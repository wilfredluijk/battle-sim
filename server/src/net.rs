use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::protocol::{self, error_code, BotMsg, FireCommand, ServerMsg};
use crate::room::{BotRegistration, PendingCommand, RoomEvent, SpectatorFrame};

/// After this many protocol violations, the bot connection is closed.
const MAX_VIOLATIONS: u32 = 5;

/// Cap on the number of bytes we will buffer while reading the HTTP request head. Real
/// requests fit comfortably inside 2 KiB; anything larger is almost certainly hostile.
const MAX_HEAD_BYTES: usize = 8 * 1024;

/// Cap on a single WebSocket message/frame. Bot JSON commands are well under 1 KiB; 16 KiB
/// is generous slack without exposing the server to multi-megabyte parse DoS.
const MAX_WS_MESSAGE_BYTES: usize = 16 * 1024;

/// Tracker for live TCP connections per peer IP. Wrapped in `Arc<Mutex<..>>` so the
/// accept loop and per-connection cleanup share a view.
type IpConnTable = Arc<Mutex<HashMap<IpAddr, u32>>>;

/// RAII guard that decrements the per-IP counter on drop. Acquired right after accept;
/// dropped when the connection task ends. Skips bookkeeping when the cap is disabled.
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
        let mut guard = match table.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if let Some(entry) = guard.get_mut(&self.ip) {
            *entry = entry.saturating_sub(1);
            if *entry == 0 {
                guard.remove(&self.ip);
            }
        }
    }
}

/// Static spectator assets, embedded at compile time so the server has no runtime path
/// dependency on the `spectator/` directory.
static INDEX_HTML: &str = include_str!("../../spectator/index.html");
static RENDER_JS: &str = include_str!("../../spectator/render.js");
static STYLE_CSS: &str = include_str!("../../spectator/style.css");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Endpoint {
    Bot,
    Spectator,
}

type Ws = WebSocketStream<PrefixedStream>;
type WsSink = SplitSink<Ws, Message>;
type WsStream = SplitStream<Ws>;

pub async fn run(
    config: Config,
    room_tx: mpsc::Sender<RoomEvent>,
    spec_tx: broadcast::Sender<SpectatorFrame>,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    let addr: SocketAddr = ([0, 0, 0, 0], config.port).into();
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(%addr, error = %e, "failed to bind TCP listener");
            return;
        }
    };
    info!(
        %addr,
        max_conn_per_ip = config.max_connections_per_ip,
        handshake_timeout_secs = config.handshake_timeout_secs,
        tournament = config.tournament,
        "listener bound (HTTP /, WS /bot, WS /spectate)"
    );

    let ip_conns: IpConnTable = Arc::new(Mutex::new(HashMap::new()));
    let handshake_timeout = Duration::from_secs(config.handshake_timeout_secs.max(1));
    let per_ip_cap = config.max_connections_per_ip;
    let tournament = config.tournament;

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("net: shutdown signal received");
                break;
            }
            res = listener.accept() => {
                match res {
                    Ok((stream, peer)) => {
                        let guard = match IpConnGuard::try_acquire(&ip_conns, peer.ip(), per_ip_cap) {
                            Some(g) => g,
                            None => {
                                warn!(%peer, cap = per_ip_cap, "refusing connection: per-IP cap reached");
                                drop(stream);
                                continue;
                            }
                        };
                        let conn_shutdown = shutdown_rx.resubscribe();
                        let room_tx = room_tx.clone();
                        let spec_tx = spec_tx.clone();
                        tokio::spawn(handle_connection(
                            stream,
                            peer,
                            room_tx,
                            spec_tx,
                            conn_shutdown,
                            handshake_timeout,
                            tournament,
                            guard,
                        ));
                    }
                    Err(e) => {
                        warn!(error = %e, "accept failed");
                    }
                }
            }
        }
    }
}

/// Read the HTTP request head, then dispatch: WS upgrade → `/bot` or `/spectate`,
/// plain HTTP GET → static file response.
#[allow(clippy::too_many_arguments)]
async fn handle_connection(
    mut stream: TcpStream,
    peer: SocketAddr,
    room_tx: mpsc::Sender<RoomEvent>,
    spec_tx: broadcast::Sender<SpectatorFrame>,
    shutdown_rx: broadcast::Receiver<()>,
    handshake_timeout: Duration,
    tournament: bool,
    _ip_guard: IpConnGuard,
) {
    let head_bytes = match timeout(handshake_timeout, read_http_head(&mut stream)).await {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => {
            debug!(%peer, error = %e, "failed reading HTTP head");
            return;
        }
        Err(_) => {
            warn!(%peer, "HTTP head read timed out");
            let _ = write_http_response(
                &mut stream,
                408,
                "Request Timeout",
                "text/plain; charset=utf-8",
                b"timeout",
            )
            .await;
            return;
        }
    };

    let parsed = match parse_request(&head_bytes) {
        Ok(p) => p,
        Err(e) => {
            warn!(%peer, error = %e, "malformed HTTP request");
            let _ = write_http_response(
                &mut stream,
                400,
                "Bad Request",
                "text/plain; charset=utf-8",
                b"bad request",
            )
            .await;
            return;
        }
    };

    if parsed.is_websocket_upgrade {
        let endpoint = match parsed.path.as_str() {
            "/bot" => Endpoint::Bot,
            "/spectate" => Endpoint::Spectator,
            other => {
                warn!(%peer, path = other, "unknown websocket path; closing");
                let _ = write_http_response(
                    &mut stream,
                    404,
                    "Not Found",
                    "text/plain; charset=utf-8",
                    b"unknown websocket path",
                )
                .await;
                return;
            }
        };

        // In tournament mode, `/spectate` is loopback-only. Refuse the upgrade before we
        // burn cycles on a handshake the spec view would just leak ground truth through.
        if tournament && endpoint == Endpoint::Spectator && !peer.ip().is_loopback() {
            warn!(%peer, "refusing /spectate: tournament mode allows loopback only");
            let _ = write_http_response(
                &mut stream,
                403,
                "Forbidden",
                "text/plain; charset=utf-8",
                b"spectator endpoint disabled in tournament mode",
            )
            .await;
            return;
        }

        let prefixed = PrefixedStream::new(head_bytes, stream);
        let ws_config = WebSocketConfig {
            max_message_size: Some(MAX_WS_MESSAGE_BYTES),
            max_frame_size: Some(MAX_WS_MESSAGE_BYTES),
            ..Default::default()
        };
        let ws = match timeout(
            handshake_timeout,
            tokio_tungstenite::accept_async_with_config(prefixed, Some(ws_config)),
        )
        .await
        {
            Ok(Ok(ws)) => ws,
            Ok(Err(e)) => {
                warn!(%peer, error = %e, "websocket handshake failed");
                return;
            }
            Err(_) => {
                warn!(%peer, "websocket handshake timed out");
                return;
            }
        };

        info!(%peer, ?endpoint, "websocket connected");
        match endpoint {
            Endpoint::Bot => handle_bot(peer, ws, room_tx, shutdown_rx, handshake_timeout).await,
            Endpoint::Spectator => handle_spectator(peer, ws, spec_tx, shutdown_rx).await,
        }
        info!(%peer, ?endpoint, "connection ended");
    } else {
        // Plain HTTP — static file serving.
        if !parsed.method.eq_ignore_ascii_case("GET") {
            let _ = write_http_response(
                &mut stream,
                405,
                "Method Not Allowed",
                "text/plain; charset=utf-8",
                b"method not allowed",
            )
            .await;
            return;
        }
        match resolve_static(&parsed.path) {
            Some((content_type, body)) => {
                debug!(%peer, path = %parsed.path, "serving static asset");
                let _ = write_http_response(&mut stream, 200, "OK", content_type, body).await;
            }
            None => {
                debug!(%peer, path = %parsed.path, "static asset not found");
                let _ = write_http_response(
                    &mut stream,
                    404,
                    "Not Found",
                    "text/plain; charset=utf-8",
                    b"not found",
                )
                .await;
            }
        }
    }
}

async fn handle_bot(
    peer: SocketAddr,
    ws: Ws,
    room_tx: mpsc::Sender<RoomEvent>,
    mut shutdown_rx: broadcast::Receiver<()>,
    hello_timeout: Duration,
) {
    let (mut sink, mut stream) = ws.split();
    let mut violations: u32 = 0;

    // Phase 1: wait for `hello`. Reject other typed messages and malformed frames as
    // protocol violations; disconnect after MAX_VIOLATIONS. A bot that connects but never
    // sends `hello` is dropped after `hello_timeout`.
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
                "hello not received within handshake timeout",
            )
            .await;
            let _ = sink
                .send(Message::Close(Some(CloseFrame {
                    code: CloseCode::Policy,
                    reason: "handshake timeout".into(),
                })))
                .await;
            return;
        }
    };

    // Validate the name charset/length before we ever hand it to the room. Saves a
    // round-trip and keeps the violation accounting consistent with malformed messages.
    if let Err(reason) = protocol::validate_bot_name(&name) {
        warn!(%peer, name = %name, %reason, "rejecting invalid bot name");
        send_error(&mut sink, error_code::INVALID_NAME, reason).await;
        let _ = sink
            .send(Message::Close(Some(CloseFrame {
                code: CloseCode::Policy,
                reason: "invalid name".into(),
            })))
            .await;
        return;
    }

    // Phase 2: register with the room and grab our outbound channel.
    let registration = match register(peer, &room_tx, name, version, &mut sink).await {
        Some(r) => r,
        None => return,
    };
    let bot_id = registration.bot_id.clone();
    let mut outbound_rx = registration.outbound;

    info!(%peer, bot_id = %bot_id, ship_id = %registration.ship_id, "bot handshake complete");

    // Phase 3: main loop — forward inbound bot messages to the room and outbound server
    // messages to the websocket.
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
                                    send_error(
                                        &mut sink,
                                        error_code::NON_FINITE_VALUE,
                                        reason,
                                    )
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
                            "this endpoint only accepts text JSON frames",
                        )
                        .await;
                        if violations >= MAX_VIOLATIONS {
                            disconnect_for_violations(&mut sink).await;
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = sink.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(frame))) => {
                        info!(%peer, bot_id = %bot_id, ?frame, "bot closed");
                        break;
                    }
                    Some(Ok(Message::Frame(_))) => {}
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

    // Best-effort notify the room. If the channel is gone (server shutting down) the
    // room is already tearing down its bookkeeping.
    let _ = room_tx.send(RoomEvent::BotDisconnect { bot_id }).await;
}

/// Read frames until we get a valid `hello` or the connection ends. Pings are answered;
/// non-hello messages and malformed frames count as protocol violations.
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
                                    "first message must be `hello`",
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
                            "this endpoint only accepts text JSON frames",
                        ).await;
                        if *violations >= MAX_VIOLATIONS {
                            disconnect_for_violations(sink).await;
                            return None;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = sink.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(frame))) => {
                        info!(%peer, ?frame, "bot closed before handshake");
                        return None;
                    }
                    Some(Ok(Message::Frame(_))) => {}
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

/// Send `BotConnect` to the room and await registration. Reports failures back to the
/// bot via an `error` frame.
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

async fn handle_spectator(
    peer: SocketAddr,
    ws: Ws,
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
                        // The Arc avoids re-allocating per subscriber, but the WS sink
                        // still needs an owned `String` per send.
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
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = sink.send(Message::Pong(payload)).await;
                    }
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

/// Reject `command` frames containing `NaN` / `Inf`. Without this, NaN propagates into
/// `physics::step_world` (NaN positions, broken distance checks) and is a determinism
/// hazard as well as a DoS sink for the JSON parser if combined with huge payloads.
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

/// Returns `false` if the underlying socket failed; the caller should treat that as a
/// terminal condition for the connection.
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
            code: CloseCode::Policy,
            reason: "too many protocol violations".into(),
        })))
        .await;
}

// ---------------------------------------------------------------------------
// HTTP head reading and dispatch
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ParsedRequest {
    method: String,
    path: String,
    is_websocket_upgrade: bool,
}

/// Read raw bytes from the stream until we see `\r\n\r\n` (end of HTTP request head),
/// then return the buffered bytes verbatim. The buffer is replayed to the WS handshake
/// via `PrefixedStream` so tungstenite can re-parse the request itself.
async fn read_http_head(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 512];
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed before HTTP head was complete",
            ));
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return Ok(buf);
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP head exceeded size limit",
            ));
        }
    }
}

fn parse_request(buf: &[u8]) -> Result<ParsedRequest, String> {
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut req = httparse::Request::new(&mut headers);
    match req.parse(buf).map_err(|e| e.to_string())? {
        httparse::Status::Complete(_) => {}
        httparse::Status::Partial => return Err("partial request".into()),
    }
    let method = req.method.unwrap_or("").to_string();
    let path = req.path.unwrap_or("/").to_string();
    let upgrade_to_ws = req.headers.iter().any(|h| {
        h.name.eq_ignore_ascii_case("Upgrade") && h.value.eq_ignore_ascii_case(b"websocket")
    });
    let connection_upgrade = req.headers.iter().any(|h| {
        if !h.name.eq_ignore_ascii_case("Connection") {
            return false;
        }
        let value = std::str::from_utf8(h.value).unwrap_or("");
        value
            .split(',')
            .any(|p| p.trim().eq_ignore_ascii_case("upgrade"))
    });
    Ok(ParsedRequest {
        method,
        path,
        is_websocket_upgrade: upgrade_to_ws && connection_upgrade,
    })
}

/// Map a request path to an embedded asset. Anything outside this small whitelist 404s,
/// so directory traversal isn't a concern.
fn resolve_static(path: &str) -> Option<(&'static str, &'static [u8])> {
    let path_only = path.split('?').next().unwrap_or(path);
    match path_only {
        "/" | "/index.html" => Some(("text/html; charset=utf-8", INDEX_HTML.as_bytes())),
        "/render.js" => Some((
            "application/javascript; charset=utf-8",
            RENDER_JS.as_bytes(),
        )),
        "/style.css" => Some(("text/css; charset=utf-8", STYLE_CSS.as_bytes())),
        _ => None,
    }
}

async fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    status_text: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: {content_type}\r\nContent-Length: {len}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        len = body.len(),
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// PrefixedStream: replay the buffered HTTP head before delegating to the TCP stream
// ---------------------------------------------------------------------------

/// `AsyncRead + AsyncWrite` adapter that emits a fixed prefix of buffered bytes before
/// reading from the wrapped `TcpStream`. We use this so the WebSocket handshake sees
/// the same request bytes we already consumed for path-routing.
pub struct PrefixedStream {
    prefix: Vec<u8>,
    cursor: usize,
    inner: TcpStream,
}

impl PrefixedStream {
    pub fn new(prefix: Vec<u8>, inner: TcpStream) -> Self {
        Self {
            prefix,
            cursor: 0,
            inner,
        }
    }
}

impl AsyncRead for PrefixedStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        if me.cursor < me.prefix.len() {
            let remaining = &me.prefix[me.cursor..];
            let n = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..n]);
            me.cursor += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut me.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for PrefixedStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(bytes: &[u8]) -> ParsedRequest {
        parse_request(bytes).expect("parse")
    }

    #[test]
    fn parses_plain_get() {
        let r = req(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(r.method, "GET");
        assert_eq!(r.path, "/");
        assert!(!r.is_websocket_upgrade);
    }

    #[test]
    fn detects_websocket_upgrade() {
        let r = req(b"GET /bot HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: x\r\nSec-WebSocket-Version: 13\r\n\r\n");
        assert!(r.is_websocket_upgrade);
        assert_eq!(r.path, "/bot");
    }

    #[test]
    fn handles_compound_connection_header() {
        // Some clients send `Connection: keep-alive, Upgrade`.
        let r = req(b"GET /spectate HTTP/1.1\r\nUpgrade: websocket\r\nConnection: keep-alive, Upgrade\r\nSec-WebSocket-Key: x\r\nSec-WebSocket-Version: 13\r\n\r\n");
        assert!(r.is_websocket_upgrade);
        assert_eq!(r.path, "/spectate");
    }

    #[test]
    fn static_routes() {
        assert!(resolve_static("/").is_some());
        assert!(resolve_static("/index.html").is_some());
        assert!(resolve_static("/render.js").is_some());
        assert!(resolve_static("/style.css").is_some());
        assert!(resolve_static("/etc/passwd").is_none());
        // Query strings are stripped before matching.
        assert!(resolve_static("/?cachebust=1").is_some());
    }
}
