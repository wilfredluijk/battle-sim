//! HTTP + WebSocket front end, built on `axum`.
//!
//! Three planes share one listener:
//! - **REST control plane** (`/api/*`) — admin login + room lifecycle, gated by JWT.
//! - **WebSocket streams** (`/bot`, `/spectate`) — the inherently streaming surfaces.
//! - **Static assets** (`/`, `/index.*`) — the embedded spectator UI.
//!
//! `sim/` is never imported here except for the wire-facing `SimConfig`; the room
//! translates between protocol messages and simulation commands.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{close_code, CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Json, Path as AxumPath, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::timeout;
use tracing::{debug, error, info, warn};

use crate::admin::AdminState;
use crate::auth::AuthState;
use crate::config::Config;
use crate::protocol::{self, error_code, BotMsg, FireCommand, ServerMsg};
use crate::replay;
use crate::room::{
    BotRegistration, ConfigureError, MatchReport, PendingCommand, RoomEvent, RoomSnapshot,
    SpectatorFrame, StartError,
};
use crate::sim::SimConfig;

/// After this many protocol violations, the bot connection is closed.
const MAX_VIOLATIONS: u32 = 5;

/// Cap on a single WebSocket message/frame. Bot JSON commands are well under 1 KiB; 16 KiB
/// is generous slack without exposing the server to multi-megabyte parse DoS.
const MAX_WS_MESSAGE_BYTES: usize = 16 * 1024;

/// Static spectator assets, embedded at compile time. Built from `spectator/dist/` — see
/// `spectator/vite.config.ts`.
static INDEX_HTML: &str = include_str!("../../spectator/dist/index.html");
static INDEX_JS: &str = include_str!("../../spectator/dist/index.js");
static INDEX_CSS: &str = include_str!("../../spectator/dist/index.css");

// ---------------------------------------------------------------------------
// Per-IP connection cap
// ---------------------------------------------------------------------------

type IpConnTable = Arc<Mutex<HashMap<IpAddr, u32>>>;

/// RAII guard that decrements the per-IP counter on drop. Acquired before a WebSocket
/// upgrade; dropped when the connection task ends. Skips bookkeeping when the cap is 0.
struct IpConnGuard {
    table: Option<IpConnTable>,
    ip: IpAddr,
}

impl IpConnGuard {
    fn try_acquire(table: &IpConnTable, ip: IpAddr, cap: u32) -> Option<Self> {
        if cap == 0 {
            return Some(Self { table: None, ip });
        }
        let mut guard = table.lock().expect("ip table mutex poisoned");
        let entry = guard.entry(ip).or_insert(0);
        if *entry >= cap {
            return None;
        }
        *entry += 1;
        Some(Self {
            table: Some(table.clone()),
            ip,
        })
    }
}

impl Drop for IpConnGuard {
    fn drop(&mut self) {
        let Some(table) = self.table.as_ref() else {
            return;
        };
        let Ok(mut guard) = table.lock() else {
            return;
        };
        if let Some(entry) = guard.get_mut(&self.ip) {
            *entry = entry.saturating_sub(1);
            if *entry == 0 {
                guard.remove(&self.ip);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shared state + entry point
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    room_tx: mpsc::Sender<RoomEvent>,
    spec_tx: broadcast::Sender<SpectatorFrame>,
    shutdown_tx: broadcast::Sender<()>,
    auth: Arc<AuthState>,
    room_name: Arc<str>,
    ip_conns: IpConnTable,
    per_ip_cap: u32,
    tournament: bool,
    hello_timeout: Duration,
    /// Directory replay JSONL logs are written to; the replay viewer reads them back.
    replay_dir: Arc<Path>,
}

/// Bind the listener and serve the HTTP/WebSocket app until `shutdown_tx` fires.
pub async fn run(
    config: Config,
    room_name: String,
    auth: Arc<AuthState>,
    room_tx: mpsc::Sender<RoomEvent>,
    spec_tx: broadcast::Sender<SpectatorFrame>,
    shutdown_tx: broadcast::Sender<()>,
) {
    let addr: SocketAddr = ([0, 0, 0, 0], config.port).into();
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            error!(%addr, error = %e, "failed to bind TCP listener");
            return;
        }
    };
    info!(
        %addr,
        max_conn_per_ip = config.max_connections_per_ip,
        tournament = config.tournament,
        "listener bound (HTTP /, REST /api, WS /bot, WS /spectate)"
    );

    let state = AppState {
        room_tx,
        spec_tx,
        shutdown_tx: shutdown_tx.clone(),
        auth,
        room_name: Arc::from(room_name),
        ip_conns: Arc::new(Mutex::new(HashMap::new())),
        per_ip_cap: config.max_connections_per_ip,
        tournament: config.tournament,
        hello_timeout: Duration::from_secs(config.handshake_timeout_secs.max(1)),
        replay_dir: Arc::from(config.replay_dir.clone()),
    };

    let app = router(state);
    let mut shutdown_rx = shutdown_tx.subscribe();
    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = shutdown_rx.recv().await;
        info!("net: shutdown signal received");
    });
    if let Err(e) = server.await {
        error!(error = %e, "http server error");
    }
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(serve_index))
        .route("/index.html", get(serve_index))
        .route("/index.js", get(serve_js))
        .route("/index.css", get(serve_css))
        .route("/api/login", post(login))
        .route("/api/room", get(get_room))
        .route("/api/room/report", get(get_report))
        .route("/api/config/schema", get(get_config_schema))
        .route("/api/room/config", put(put_config))
        .route("/api/room/start", post(post_start))
        .route("/api/room/abort", post(post_abort))
        .route("/api/room/reset", post(post_reset))
        .route("/api/room/kick", post(post_kick))
        .route("/api/replays", get(list_replays))
        .route("/api/replays/:id", get(get_replay))
        .route(
            "/api/replays/:id/perspective/:bot_id",
            get(get_replay_perspective),
        )
        .route("/bot", get(bot_ws))
        .route("/spectate", get(spectate_ws))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Static assets
// ---------------------------------------------------------------------------

fn static_response(content_type: &'static str, body: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

async fn serve_index() -> Response {
    static_response("text/html; charset=utf-8", INDEX_HTML)
}

async fn serve_js() -> Response {
    static_response("application/javascript; charset=utf-8", INDEX_JS)
}

async fn serve_css() -> Response {
    static_response("text/css; charset=utf-8", INDEX_CSS)
}

// ---------------------------------------------------------------------------
// REST: errors + helpers
// ---------------------------------------------------------------------------

/// A REST error rendered as `{ "code": ..., "message": ... }` with an HTTP status.
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "code": self.code, "message": self.message })),
        )
            .into_response()
    }
}

/// Reject the request unless it carries a valid `Authorization: Bearer <jwt>` header.
fn require_admin(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));
    match token {
        Some(t) if state.auth.verify_token(t) => Ok(()),
        _ => Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "a valid admin token is required",
        )),
    }
}

/// Send a `RoomEvent` built around a fresh oneshot and await its reply.
async fn ask_room<T>(
    state: &AppState,
    make: impl FnOnce(oneshot::Sender<T>) -> RoomEvent,
) -> Result<T, ApiError> {
    let (tx, rx) = oneshot::channel();
    if state.room_tx.send(make(tx)).await.is_err() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "room_unavailable",
            "the room is not running",
        ));
    }
    rx.await.map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "room_unavailable",
            "the room dropped the reply",
        )
    })
}

// ---------------------------------------------------------------------------
// REST: auth
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct LoginRequest {
    password: String,
}

#[derive(Serialize)]
struct LoginResponse {
    token: String,
    expires_at: u64,
}

async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    if !state.auth.verify_password(&req.password) {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "incorrect admin password",
        ));
    }
    let (token, expires_at) = state.auth.issue_token().map_err(|e| {
        warn!(error = %e, "failed to mint admin token");
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "token_error",
            "could not issue a token",
        )
    })?;
    Ok(Json(LoginResponse { token, expires_at }))
}

// ---------------------------------------------------------------------------
// REST: room state + lifecycle
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct RoomResponse {
    #[serde(flatten)]
    state: AdminState,
    config: SimConfig,
}

/// Public: current room state plus the active balance parameters. Drives both the
/// pre-match summary screen and the post-battle report.
async fn get_room(State(state): State<AppState>) -> Result<Json<RoomResponse>, ApiError> {
    let snap: RoomSnapshot = ask_room(&state, |reply| RoomEvent::QueryState { reply }).await?;
    Ok(Json(RoomResponse {
        state: snap.state,
        config: snap.config,
    }))
}

/// Public: the most recent match report. `404` until the first match has finished.
async fn get_report(State(state): State<AppState>) -> Result<Json<MatchReport>, ApiError> {
    let report: Option<MatchReport> =
        ask_room(&state, |reply| RoomEvent::QueryReport { reply }).await?;
    report.map(Json).ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "no_report",
            "no match has finished yet",
        )
    })
}

async fn post_start(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    require_admin(&state, &headers)?;
    let room = state.room_name.to_string();
    let result: Result<(), StartError> =
        ask_room(&state, |reply| RoomEvent::OperatorStart { room, reply }).await?;
    result.map_err(|e| ApiError::new(StatusCode::CONFLICT, "start_refused", e.as_str()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn post_abort(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    require_admin(&state, &headers)?;
    let result = ask_room(&state, |reply| RoomEvent::OperatorAbort { reply }).await?;
    result.map_err(|e| ApiError::new(StatusCode::CONFLICT, "abort_refused", e.as_str()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn post_reset(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    require_admin(&state, &headers)?;
    let result = ask_room(&state, |reply| RoomEvent::OperatorReset { reply }).await?;
    result.map_err(|e| ApiError::new(StatusCode::CONFLICT, "reset_refused", e.as_str()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct KickRequest {
    bot_id: String,
}

async fn post_kick(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<KickRequest>,
) -> Result<StatusCode, ApiError> {
    require_admin(&state, &headers)?;
    let result = ask_room(&state, |reply| RoomEvent::OperatorKick {
        bot_id: req.bot_id,
        reply,
    })
    .await?;
    result.map_err(|e| ApiError::new(StatusCode::NOT_FOUND, "unknown_bot", e.as_str()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn put_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(config): Json<SimConfig>,
) -> Result<StatusCode, ApiError> {
    require_admin(&state, &headers)?;
    let result = ask_room(&state, |reply| RoomEvent::OperatorConfigure {
        config,
        reply,
    })
    .await?;
    result.map_err(|e| {
        let (status, code) = match e {
            ConfigureError::NotInLobby => (StatusCode::CONFLICT, "not_in_lobby"),
            ConfigureError::Invalid(_) => (StatusCode::BAD_REQUEST, "invalid_parameter"),
        };
        ApiError::new(status, code, e.as_str().to_string())
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Public: metadata for the pre-match parameter form — every tunable with its group,
/// default and sane bounds. The frontend renders one input per entry.
async fn get_config_schema() -> Json<serde_json::Value> {
    let d = SimConfig::default();
    let num = |key: &str, label: &str, group: &str, default: f64, min: f64, max: f64, int: bool| {
        json!({
            "key": key, "label": label, "group": group,
            "default": default, "min": min, "max": max, "integer": int,
        })
    };
    Json(json!({
        "fields": [
            num("hull_hp", "Hull HP", "ship", d.hull_hp as f64, 1.0, 100000.0, true),
            num("max_ammo", "Ammo capacity", "ship", d.max_ammo as f64, 1.0, 100000.0, true),
            num("gun_cooldown_ticks", "Gun cooldown (ticks)", "ship", d.gun_cooldown_ticks as f64, 1.0, 100000.0, true),
            num("max_forward_speed", "Max forward speed", "ship", d.max_forward_speed as f64, 0.1, 1000.0, false),
            num("max_reverse_speed", "Max reverse speed", "ship", d.max_reverse_speed as f64, 0.1, 1000.0, false),
            num("acceleration", "Acceleration", "ship", d.acceleration as f64, 0.1, 10000.0, false),
            num("turn_rate_deg_per_s", "Turn rate (deg/s)", "ship", d.turn_rate_deg_per_s as f64, 0.1, 36000.0, false),
            num("hit_radius", "Hit radius", "ship", d.hit_radius as f64, 0.1, 10000.0, false),
            num("shell_speed", "Shell speed", "weapons", d.shell_speed as f64, 0.1, 100000.0, false),
            num("max_shell_range", "Max shell range", "weapons", d.max_shell_range as f64, 0.1, 1000000.0, false),
            num("splash_radius", "Splash radius", "weapons", d.splash_radius as f64, 0.1, 1000000.0, false),
            num("max_splash_damage", "Max splash damage", "weapons", d.max_splash_damage as f64, 1.0, 100000.0, true),
            num("wall_bump_damage", "Wall bump damage", "weapons", d.wall_bump_damage as f64, 0.0, 100000.0, true),
            num("active_radar_range", "Active radar range", "sensors", d.active_radar_range as f64, 0.1, 1000000.0, false),
            num("active_radar_noise", "Active radar noise", "sensors", d.active_radar_noise as f64, 0.0, 1000000.0, false),
            num("passive_hear_active_range", "Passive hear-active range", "sensors", d.passive_hear_active_range as f64, 0.1, 1000000.0, false),
            num("passive_hear_nearby_range", "Passive hear-nearby range", "sensors", d.passive_hear_nearby_range as f64, 0.1, 1000000.0, false),
            num("passive_bearing_noise_deg", "Passive bearing noise (deg)", "sensors", d.passive_bearing_noise_deg as f64, 0.0, 180.0, false),
        ]
    }))
}

// ---------------------------------------------------------------------------
// REST: replay viewer
// ---------------------------------------------------------------------------

/// Returns `true` if `id` is a safe replay identifier — non-empty, bounded, and built from
/// a charset that cannot express a path separator or `..`. This blocks directory traversal
/// before the id is ever joined onto `replay_dir`.
fn replay_id_is_valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Reject replay access from non-loopback peers in tournament mode. Replays expose
/// ground-truth state, exactly like the live `/spectate` stream, so they get the same gate.
fn require_replay_access(state: &AppState, peer: SocketAddr) -> Result<(), ApiError> {
    if state.tournament && !peer.ip().is_loopback() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "tournament_mode",
            "replay endpoints are restricted to loopback in tournament mode",
        ));
    }
    Ok(())
}

/// Validate a replay id and resolve it to a path inside `replay_dir`.
fn resolve_replay_path(state: &AppState, id: &str) -> Result<PathBuf, ApiError> {
    if !replay_id_is_valid(id) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_replay_id",
            "replay id must be 1-128 chars of A-Z, a-z, 0-9, underscore or hyphen",
        ));
    }
    Ok(state.replay_dir.join(format!("{id}.jsonl")))
}

/// Map a [`replay::ReplayError`] onto an HTTP-shaped [`ApiError`].
fn map_replay_error(e: replay::ReplayError) -> ApiError {
    use replay::ReplayError;
    match e {
        ReplayError::Io(io_err) if io_err.kind() == std::io::ErrorKind::NotFound => {
            ApiError::new(StatusCode::NOT_FOUND, "replay_not_found", "no such replay")
        }
        ReplayError::Io(io_err) => ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "replay_io_error",
            io_err.to_string(),
        ),
        ReplayError::Header(msg) => {
            ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid_replay", msg)
        }
        ReplayError::Version(v) => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unsupported_replay_version",
            format!("replay format version {v} is not supported"),
        ),
        ReplayError::UnknownBot(id) => ApiError::new(
            StatusCode::NOT_FOUND,
            "unknown_bot",
            format!("replay has no bot `{id}`"),
        ),
    }
}

/// Public (loopback-only under tournament mode): list the replays available on disk.
async fn list_replays(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<Vec<replay::ReplaySummary>>, ApiError> {
    require_replay_access(&state, peer)?;
    let dir = state.replay_dir.clone();
    let summaries = tokio::task::spawn_blocking(move || replay::list_replays(&dir))
        .await
        .map_err(|e| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "replay_io_error",
                format!("listing task panicked: {e}"),
            )
        })?
        .map_err(|e| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "replay_io_error",
                e.to_string(),
            )
        })?;
    Ok(Json(summaries))
}

/// Public: re-run a replay and return the full ground-truth timeline.
async fn get_replay(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<replay::CapturedReplay>, ApiError> {
    require_replay_access(&state, peer)?;
    let path = resolve_replay_path(&state, &id)?;
    let captured = tokio::task::spawn_blocking(move || {
        let records = replay::read_records(&path)?;
        replay::capture_replay(records)
    })
    .await
    .map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "replay_io_error",
            format!("capture task panicked: {e}"),
        )
    })?
    .map_err(map_replay_error)?;
    Ok(Json(captured))
}

/// Public: re-run a replay from one bot's sensor perspective.
async fn get_replay_perspective(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    AxumPath((id, bot_id)): AxumPath<(String, String)>,
) -> Result<Json<replay::CapturedPerspective>, ApiError> {
    require_replay_access(&state, peer)?;
    let path = resolve_replay_path(&state, &id)?;
    let captured = tokio::task::spawn_blocking(move || {
        let records = replay::read_records(&path)?;
        replay::capture_perspective(records, &bot_id)
    })
    .await
    .map_err(|e| {
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "replay_io_error",
            format!("capture task panicked: {e}"),
        )
    })?
    .map_err(map_replay_error)?;
    Ok(Json(captured))
}

// ---------------------------------------------------------------------------
// WebSocket: upgrade handlers
// ---------------------------------------------------------------------------

async fn bot_ws(
    ws: WebSocketUpgrade,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Response {
    let guard = match IpConnGuard::try_acquire(&state.ip_conns, peer.ip(), state.per_ip_cap) {
        Some(g) => g,
        None => {
            warn!(%peer, "refusing /bot: per-IP connection cap reached");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "per-IP connection cap reached",
            )
                .into_response();
        }
    };
    let hello_timeout = state.hello_timeout;
    let shutdown_rx = state.shutdown_tx.subscribe();
    let room_tx = state.room_tx.clone();
    let ws = ws
        .max_message_size(MAX_WS_MESSAGE_BYTES)
        .max_frame_size(MAX_WS_MESSAGE_BYTES);
    ws.on_upgrade(move |socket| async move {
        let _guard = guard;
        info!(%peer, "bot websocket connected");
        handle_bot(peer, socket, room_tx, shutdown_rx, hello_timeout).await;
        info!(%peer, "bot connection ended");
    })
}

async fn spectate_ws(
    ws: WebSocketUpgrade,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Response {
    // Tournament mode keeps ground-truth spectator state on loopback only.
    if state.tournament && !peer.ip().is_loopback() {
        warn!(%peer, "refusing /spectate: tournament mode allows loopback only");
        return (
            StatusCode::FORBIDDEN,
            "spectator endpoint disabled in tournament mode",
        )
            .into_response();
    }
    let guard = match IpConnGuard::try_acquire(&state.ip_conns, peer.ip(), state.per_ip_cap) {
        Some(g) => g,
        None => {
            warn!(%peer, "refusing /spectate: per-IP connection cap reached");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "per-IP connection cap reached",
            )
                .into_response();
        }
    };
    let spec_tx = state.spec_tx.clone();
    let shutdown_rx = state.shutdown_tx.subscribe();
    let ws = ws
        .max_message_size(MAX_WS_MESSAGE_BYTES)
        .max_frame_size(MAX_WS_MESSAGE_BYTES);
    ws.on_upgrade(move |socket| async move {
        let _guard = guard;
        info!(%peer, "spectator websocket connected");
        handle_spectator(peer, socket, spec_tx, shutdown_rx).await;
        info!(%peer, "spectator connection ended");
    })
}

type WsSink = SplitSink<WebSocket, Message>;
type WsStream = SplitStream<WebSocket>;

// ---------------------------------------------------------------------------
// WebSocket: /bot
// ---------------------------------------------------------------------------

async fn handle_bot(
    peer: SocketAddr,
    ws: WebSocket,
    room_tx: mpsc::Sender<RoomEvent>,
    mut shutdown_rx: broadcast::Receiver<()>,
    hello_timeout: Duration,
) {
    let (mut sink, mut stream) = ws.split();
    let mut violations: u32 = 0;

    // Phase 1: wait for `hello`. A bot that never sends it is dropped after the timeout.
    let hello_fut = wait_for_hello(
        peer,
        &mut sink,
        &mut stream,
        &mut shutdown_rx,
        &mut violations,
    );
    let (name, version) = match timeout(hello_timeout, hello_fut).await {
        Ok(Some(hello)) => hello,
        Ok(None) => return,
        Err(_) => {
            warn!(%peer, "bot did not send `hello` within timeout; dropping");
            send_error(
                &mut sink,
                error_code::HANDSHAKE_TIMEOUT,
                format!(
                    "hello not received within {}s; first frame must be \
                     {{\"type\":\"hello\",\"name\":\"...\",\"version\":\"...\"}}",
                    hello_timeout.as_secs()
                ),
            )
            .await;
            let _ = sink
                .send(Message::Close(Some(CloseFrame {
                    code: close_code::POLICY,
                    reason: "handshake timeout".into(),
                })))
                .await;
            return;
        }
    };

    if let Err(reason) = protocol::validate_bot_name(&name) {
        warn!(%peer, name = %name, %reason, "rejecting invalid bot name");
        send_error(&mut sink, error_code::INVALID_NAME, reason).await;
        let _ = sink
            .send(Message::Close(Some(CloseFrame {
                code: close_code::POLICY,
                reason: "invalid name".into(),
            })))
            .await;
        return;
    }

    // Phase 2: register with the room.
    let registration = match register(peer, &room_tx, name, version, &mut sink).await {
        Some(r) => r,
        None => return,
    };
    let bot_id = registration.bot_id.clone();
    let mut outbound_rx = registration.outbound;
    info!(%peer, bot_id = %bot_id, ship_id = %registration.ship_id, "bot handshake complete");

    // Phase 3: forward inbound bot messages to the room and outbound frames to the socket.
    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!(%peer, bot_id = %bot_id, "closing bot connection (shutdown)");
                let _ = sink.send(Message::Close(None)).await;
                break;
            }
            outbound = outbound_rx.recv() => {
                let Some(msg) = outbound else {
                    debug!(%peer, bot_id = %bot_id, "room dropped outbound channel");
                    break;
                };
                if !send_server_msg(&mut sink, &msg).await {
                    break;
                }
            }
            frame = stream.next() => {
                match frame {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<BotMsg>(&text) {
                            Ok(BotMsg::Hello { .. }) => {
                                violations += 1;
                                warn!(%peer, bot_id = %bot_id, violations, "duplicate hello");
                                send_error(
                                    &mut sink,
                                    error_code::INVALID_MESSAGE,
                                    "hello already received for this connection",
                                ).await;
                                if violations >= MAX_VIOLATIONS {
                                    disconnect_for_violations(&mut sink).await;
                                    break;
                                }
                            }
                            Ok(BotMsg::Ready) => {
                                if room_tx
                                    .send(RoomEvent::BotReady { bot_id: bot_id.clone() })
                                    .await
                                    .is_err()
                                {
                                    debug!(%peer, "room channel closed; ending bot loop");
                                    break;
                                }
                            }
                            Ok(BotMsg::Command {
                                tick,
                                throttle,
                                rudder,
                                fire,
                                sensor_mode,
                                activate_powerup,
                            }) => {
                                if let Err(reason) =
                                    validate_command_floats(throttle, rudder, fire.as_ref())
                                {
                                    violations += 1;
                                    warn!(
                                        %peer,
                                        bot_id = %bot_id,
                                        violations,
                                        %reason,
                                        "rejecting command with non-finite float",
                                    );
                                    send_error(&mut sink, error_code::NON_FINITE_VALUE, reason)
                                        .await;
                                    if violations >= MAX_VIOLATIONS {
                                        disconnect_for_violations(&mut sink).await;
                                        break;
                                    }
                                    continue;
                                }
                                let command = PendingCommand {
                                    tick,
                                    throttle,
                                    rudder,
                                    sensor_mode,
                                    fire,
                                    activate_powerup,
                                };
                                if room_tx
                                    .send(RoomEvent::BotCommand {
                                        bot_id: bot_id.clone(),
                                        command,
                                    })
                                    .await
                                    .is_err()
                                {
                                    debug!(%peer, "room channel closed; ending bot loop");
                                    break;
                                }
                            }
                            Ok(BotMsg::SelectPowerups { powerups }) => {
                                if room_tx
                                    .send(RoomEvent::BotSelectPowerups {
                                        bot_id: bot_id.clone(),
                                        powerups,
                                    })
                                    .await
                                    .is_err()
                                {
                                    debug!(%peer, "room channel closed; ending bot loop");
                                    break;
                                }
                            }
                            Err(e) => {
                                violations += 1;
                                let code = if matches!(
                                    e.classify(),
                                    serde_json::error::Category::Syntax
                                ) {
                                    error_code::MALFORMED_JSON
                                } else {
                                    error_code::INVALID_MESSAGE
                                };
                                warn!(%peer, code, error = %e, violations, "rejected bot frame");
                                send_error(&mut sink, code, e.to_string()).await;
                                if violations >= MAX_VIOLATIONS {
                                    disconnect_for_violations(&mut sink).await;
                                    break;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        violations += 1;
                        warn!(%peer, bytes = bytes.len(), violations, "binary frame on /bot");
                        send_error(
                            &mut sink,
                            error_code::BINARY_FRAMES_UNSUPPORTED,
                            "/bot only accepts text JSON frames (binary frames are rejected)",
                        )
                        .await;
                        if violations >= MAX_VIOLATIONS {
                            disconnect_for_violations(&mut sink).await;
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(frame))) => {
                        info!(%peer, bot_id = %bot_id, ?frame, "bot closed");
                        break;
                    }
                    Some(Err(e)) => {
                        warn!(%peer, error = %e, "ws read error");
                        break;
                    }
                    None => {
                        info!(%peer, bot_id = %bot_id, "bot stream ended");
                        break;
                    }
                }
            }
        }
    }

    let _ = room_tx.send(RoomEvent::BotDisconnect { bot_id }).await;
}

/// Read frames until a valid `hello` arrives or the connection ends.
async fn wait_for_hello(
    peer: SocketAddr,
    sink: &mut WsSink,
    stream: &mut WsStream,
    shutdown_rx: &mut broadcast::Receiver<()>,
    violations: &mut u32,
) -> Option<(String, String)> {
    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                let _ = sink.send(Message::Close(None)).await;
                return None;
            }
            frame = stream.next() => {
                match frame {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<BotMsg>(&text) {
                            Ok(BotMsg::Hello { name, version }) => {
                                return Some((name, version));
                            }
                            Ok(_) => {
                                *violations += 1;
                                warn!(%peer, violations = *violations, "non-hello before handshake");
                                send_error(
                                    sink,
                                    error_code::INVALID_MESSAGE,
                                    "first message must be `hello`: \
                                     {\"type\":\"hello\",\"name\":\"...\",\"version\":\"...\"}",
                                ).await;
                                if *violations >= MAX_VIOLATIONS {
                                    disconnect_for_violations(sink).await;
                                    return None;
                                }
                            }
                            Err(e) => {
                                *violations += 1;
                                let code = if matches!(
                                    e.classify(),
                                    serde_json::error::Category::Syntax
                                ) {
                                    error_code::MALFORMED_JSON
                                } else {
                                    error_code::INVALID_MESSAGE
                                };
                                warn!(%peer, code, error = %e, violations = *violations, "rejected pre-handshake frame");
                                send_error(sink, code, e.to_string()).await;
                                if *violations >= MAX_VIOLATIONS {
                                    disconnect_for_violations(sink).await;
                                    return None;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        *violations += 1;
                        warn!(%peer, bytes = bytes.len(), violations = *violations, "binary frame before handshake");
                        send_error(
                            sink,
                            error_code::BINARY_FRAMES_UNSUPPORTED,
                            "/bot only accepts text JSON frames (binary frames are rejected)",
                        ).await;
                        if *violations >= MAX_VIOLATIONS {
                            disconnect_for_violations(sink).await;
                            return None;
                        }
                    }
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(frame))) => {
                        info!(%peer, ?frame, "bot closed before handshake");
                        return None;
                    }
                    Some(Err(e)) => {
                        warn!(%peer, error = %e, "ws read error before handshake");
                        return None;
                    }
                    None => {
                        info!(%peer, "bot stream ended before handshake");
                        return None;
                    }
                }
            }
        }
    }
}

/// Send `BotConnect` to the room and await registration.
async fn register(
    peer: SocketAddr,
    room_tx: &mpsc::Sender<RoomEvent>,
    name: String,
    version: String,
    sink: &mut WsSink,
) -> Option<BotRegistration> {
    let (reply_tx, reply_rx) = oneshot::channel();
    if room_tx
        .send(RoomEvent::BotConnect {
            peer,
            name,
            version,
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        warn!(%peer, "room event channel closed; refusing connection");
        send_error(sink, error_code::INVALID_MESSAGE, "server is shutting down").await;
        return None;
    }
    match reply_rx.await {
        Ok(Ok(reg)) => Some(reg),
        Ok(Err(e)) => {
            warn!(%peer, reason = e.as_str(), "room rejected join");
            send_error(sink, error_code::INVALID_MESSAGE, e.as_str()).await;
            None
        }
        Err(_) => {
            warn!(%peer, "room dropped registration reply");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// WebSocket: /spectate
// ---------------------------------------------------------------------------

async fn handle_spectator(
    peer: SocketAddr,
    ws: WebSocket,
    spec_tx: broadcast::Sender<SpectatorFrame>,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    let (mut sink, mut stream) = ws.split();
    let mut spec_rx = spec_tx.subscribe();
    info!(%peer, subscribers = spec_tx.receiver_count(), "spectator subscribed");

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!(%peer, "closing spectator connection (shutdown)");
                let _ = sink.send(Message::Close(None)).await;
                break;
            }
            recv = spec_rx.recv() => {
                match recv {
                    Ok(frame) => {
                        if sink.send(Message::Text((*frame).clone())).await.is_err() {
                            debug!(%peer, "spectator sink closed");
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(%peer, skipped, "spectator lagging; dropped frames");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!(%peer, "spectator broadcast closed");
                        break;
                    }
                }
            }
            frame = stream.next() => {
                match frame {
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(frame))) => {
                        info!(%peer, ?frame, "spectator closed");
                        break;
                    }
                    Some(Ok(_)) => {
                        // Spectators are read-only — silently drop anything they send.
                    }
                    Some(Err(e)) => {
                        warn!(%peer, error = %e, "ws read error");
                        break;
                    }
                    None => {
                        info!(%peer, "spectator stream ended");
                        break;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shared frame helpers
// ---------------------------------------------------------------------------

/// Reject `command` frames containing `NaN` / `Inf` before they reach the simulation.
fn validate_command_floats(
    throttle: f32,
    rudder: f32,
    fire: Option<&FireCommand>,
) -> Result<(), &'static str> {
    if !throttle.is_finite() {
        return Err("throttle must be finite");
    }
    if !rudder.is_finite() {
        return Err("rudder must be finite");
    }
    if let Some(f) = fire {
        if !f.bearing_deg.is_finite() {
            return Err("fire.bearing_deg must be finite");
        }
        if !f.range.is_finite() {
            return Err("fire.range must be finite");
        }
    }
    Ok(())
}

async fn send_error(sink: &mut WsSink, code: &str, message: impl Into<String>) {
    send_server_msg(sink, &protocol::error_msg(code, message)).await;
}

/// Returns `false` if the socket failed; the caller should treat that as terminal.
async fn send_server_msg(sink: &mut WsSink, msg: &ServerMsg) -> bool {
    let payload = serde_json::to_string(msg).expect("ServerMsg always serializes");
    match sink.send(Message::Text(payload)).await {
        Ok(()) => true,
        Err(e) => {
            debug!(error = %e, "failed to send server frame");
            false
        }
    }
}

async fn disconnect_for_violations(sink: &mut WsSink) {
    send_error(
        sink,
        error_code::TOO_MANY_VIOLATIONS,
        format!("disconnecting after {MAX_VIOLATIONS} violations"),
    )
    .await;
    let _ = sink
        .send(Message::Close(Some(CloseFrame {
            code: close_code::POLICY,
            reason: "too many protocol violations".into(),
        })))
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_finite_command_floats() {
        assert!(validate_command_floats(f32::NAN, 0.0, None).is_err());
        assert!(validate_command_floats(0.0, f32::INFINITY, None).is_err());
        assert!(validate_command_floats(0.5, -0.5, None).is_ok());
        let bad_fire = FireCommand {
            bearing_deg: f32::NAN,
            range: 100.0,
        };
        assert!(validate_command_floats(0.0, 0.0, Some(&bad_fire)).is_err());
    }

    #[test]
    fn ip_conn_guard_enforces_cap() {
        let table: IpConnTable = Arc::new(Mutex::new(HashMap::new()));
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        let g1 = IpConnGuard::try_acquire(&table, ip, 2);
        let g2 = IpConnGuard::try_acquire(&table, ip, 2);
        assert!(g1.is_some() && g2.is_some());
        assert!(IpConnGuard::try_acquire(&table, ip, 2).is_none());
        drop(g1);
        assert!(IpConnGuard::try_acquire(&table, ip, 2).is_some());
        drop(g2);
    }

    #[test]
    fn replay_id_validation_blocks_traversal() {
        assert!(replay_id_is_valid("match_main_1700000000"));
        assert!(replay_id_is_valid("abc-DEF_123"));
        // Anything that could escape `replay_dir` or name another file is rejected.
        assert!(!replay_id_is_valid(""));
        assert!(!replay_id_is_valid("../etc/passwd"));
        assert!(!replay_id_is_valid("foo/bar"));
        assert!(!replay_id_is_valid("foo.jsonl"));
        assert!(!replay_id_is_valid(".."));
        assert!(!replay_id_is_valid(&"x".repeat(129)));
    }
}
