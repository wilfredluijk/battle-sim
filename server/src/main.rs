use clap::Parser;
use tokio::sync::{broadcast, mpsc};
use tracing::info;
use tracing_subscriber::EnvFilter;

use naval_server::{
    auth::{self, AuthState},
    config::Config,
    net, replay,
    room::{self, Room, SpectatorFrame, ROOM_EVENT_BUFFER},
};

/// Slack in spectator-frame buffer. At 10 Hz this is 6.4s — enough for a slow client to
/// briefly stall without dropping frames; lagged clients log a `Lagged` warning rather
/// than disconnect.
const SPECTATOR_BROADCAST_BUFFER: usize = 64;

/// The single room this server hosts. Lifecycle and parameters are driven over `/api/*`.
const ROOM_NAME: &str = "main";

const BANNER: &str = r#"
========================================
   Naval Battle Server
========================================
"#;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::parse();
    println!("{BANNER}");
    info!(
        port = config.port,
        tick_hz = config.tick_hz,
        tick_deadline_ms = config.tick_deadline_ms,
        seed = config.seed,
        max_bots = config.max_bots,
        map_w = config.map.0,
        map_h = config.map.1,
        replay_dir = %config.replay_dir.display(),
        "starting naval-server"
    );

    // Resolve the admin password: explicit override wins, otherwise generate one. The
    // value is logged once so a local operator can copy it into the web UI's login form.
    let admin_password = config
        .admin_password
        .clone()
        .unwrap_or_else(auth::generate_admin_password);
    info!(
        admin_password = %admin_password,
        "admin password (POST /api/login — random each start unless --admin-password / BATTLE_ADMIN_PASSWORD is set)"
    );
    let auth = AuthState::new(
        admin_password,
        config.token_ttl_hours.saturating_mul(3600).max(60),
    );

    let (shutdown_tx, _) = broadcast::channel::<()>(8);
    let (room_tx, room_rx) = mpsc::channel(ROOM_EVENT_BUFFER);
    let (spec_tx, _) = broadcast::channel::<SpectatorFrame>(SPECTATOR_BROADCAST_BUFFER);

    let replay_path = config.replay.clone();

    let room_handle = if let Some(path) = replay_path.as_ref() {
        // Replay mode: drive a Room from a recorded JSONL log instead of accepting bot
        // connections. The room_rx is dropped immediately so any /bot connections that
        // sneak through fail-fast on registration.
        info!(path = %path.display(), "starting in replay mode");
        drop(room_rx);
        let path = path.clone();
        let spec_tx = spec_tx.clone();
        let shutdown_rx = shutdown_tx.subscribe();
        tokio::spawn(async move {
            if let Err(e) = replay::run_replay(path, spec_tx, shutdown_rx).await {
                tracing::error!(error = %e, "replay failed");
            }
            0u64
        })
    } else {
        let mut main_room = Room::new(
            ROOM_NAME.into(),
            config.map.0 as f32,
            config.map.1 as f32,
            config.seed,
            config.tick_hz,
            config.tick_deadline_ms,
            config.max_bots,
        );
        main_room.set_spectator_broadcast(spec_tx.clone());
        main_room.set_replay_dir(config.replay_dir.clone());
        tokio::spawn(room::run_room(main_room, room_rx, shutdown_tx.subscribe()))
    };

    let net_handle = tokio::spawn(net::run(
        config.clone(),
        ROOM_NAME.to_string(),
        auth,
        room_tx.clone(),
        spec_tx.clone(),
        shutdown_tx.clone(),
    ));

    match tokio::signal::ctrl_c().await {
        Ok(_) => info!("ctrl-c received, shutting down"),
        Err(e) => tracing::error!(error = %e, "ctrl-c handler failed"),
    }

    let _ = shutdown_tx.send(());
    drop(room_tx);

    if let Err(e) = net_handle.await {
        tracing::error!(error = %e, "net task panicked");
    }
    if let Err(e) = room_handle.await {
        tracing::error!(error = %e, "room task panicked");
    }

    info!("naval-server stopped");
}
