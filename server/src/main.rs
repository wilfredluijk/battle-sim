use clap::Parser;
use tokio::sync::{broadcast, mpsc};
use tracing::info;
use tracing_subscriber::EnvFilter;

use naval_server::{
    auth::AuthState,
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
    if config.healthcheck {
        use std::io::{Read, Write};
        let result = (|| -> std::io::Result<bool> {
            let mut socket = std::net::TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], config.port)),
                std::time::Duration::from_secs(1),
            )?;
            socket.set_read_timeout(Some(std::time::Duration::from_secs(2)))?;
            socket.set_write_timeout(Some(std::time::Duration::from_secs(1)))?;
            socket.write_all(
                b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            )?;
            let mut response = [0; 32];
            socket.read_exact(&mut response)?;
            Ok(response.starts_with(b"HTTP/1.1 200"))
        })();
        std::process::exit(if matches!(result, Ok(true)) { 0 } else { 1 });
    }
    println!("{BANNER}");
    info!(
        port = config.port,
        tick_hz = config.tick_hz,
        tick_deadline_ms = config.tick_deadline_ms,
        max_bots = config.max_bots,
        map_w = config.map.0,
        map_h = config.map.1,
        replay_dir = %config.replay_dir.display(),
        workshop_public_sensor_stream = config.workshop_public_sensor_stream,
        "starting naval-server"
    );

    if config.tick_deadline_ms == 0 || config.tick_deadline_ms >= 1000 / u64::from(config.tick_hz) {
        panic!("tick deadline must be positive and strictly less than the tick interval");
    }
    let participants = naval_server::participants::Participants::new(
        config.bot_credentials_file.clone(),
        config.allow_unauthenticated_bots,
    );
    if !config.allow_unauthenticated_bots && config.replay.is_none() {
        participants
            .read()
            .expect("valid protected participant roster required");
    }
    let admin_password = if let Some(path) = &config.admin_password_file {
        std::fs::read_to_string(path)
            .expect("read admin password file")
            .trim()
            .to_owned()
    } else {
        config
            .admin_password
            .clone()
            .expect("set BATTLE_ADMIN_PASSWORD_FILE or BATTLE_ADMIN_PASSWORD")
    };
    assert!(
        admin_password.len() >= 16 && admin_password != "change-me",
        "use an admin password with at least 16 characters"
    );
    let auth = AuthState::new(
        admin_password,
        config.token_ttl_hours.saturating_mul(3600).max(60),
    );

    let (shutdown_tx, _) = broadcast::channel::<()>(8);
    let (room_tx, room_rx) = mpsc::channel(ROOM_EVENT_BUFFER);
    let (spec_tx, _) = broadcast::channel::<SpectatorFrame>(SPECTATOR_BROADCAST_BUFFER);

    let replay_path = config.replay.clone();

    let mut room_handle = if let Some(path) = replay_path.as_ref() {
        // Replay mode: drive a Room from a recorded JSONL log instead of accepting bot
        // connections. Keep the read-only control plane available for the spectator.
        info!(path = %path.display(), "starting in replay mode");
        let path = path.clone();
        let spec_tx = spec_tx.clone();
        let shutdown_rx = shutdown_tx.subscribe();
        tokio::spawn(async move {
            if let Err(e) =
                replay::run_replay_with_events(path, spec_tx, shutdown_rx, Some(room_rx)).await
            {
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
        main_room.tournament = config.tournament;
        main_room.workshop_public_sensor_stream = config.workshop_public_sensor_stream;
        main_room.set_spectator_broadcast(spec_tx.clone());
        main_room.set_replay_dir(config.replay_dir.clone());
        tokio::spawn(room::run_room(main_room, room_rx, shutdown_tx.subscribe()))
    };

    let mut net_handle = tokio::spawn(net::run(
        config.clone(),
        ROOM_NAME.to_string(),
        auth,
        room_tx.clone(),
        spec_tx.clone(),
        shutdown_tx.clone(),
    ));

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM handler");
    let mut task_failed = false;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => info!("SIGINT received"),
        _ = sigterm.recv() => info!("SIGTERM received"),
        result = &mut room_handle => { tracing::error!(?result, "room task exited unexpectedly"); task_failed = true; },
        result = &mut net_handle => { tracing::error!(?result, "network task exited unexpectedly"); task_failed = true; },
    }
    let _ = shutdown_tx.send(());
    drop(room_tx);
    if !task_failed {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(8), async {
            let _ = net_handle.await;
            let _ = room_handle.await;
        })
        .await;
    }
    info!("naval-server stopped");
    if task_failed {
        std::process::exit(1);
    }
}
