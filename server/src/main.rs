use clap::Parser;
use tokio::sync::{broadcast, mpsc};
use tracing::info;
use tracing_subscriber::EnvFilter;

use naval_server::{
    config::Config,
    control, net,
    room::{self, Room, SpectatorFrame, ROOM_EVENT_BUFFER},
};

/// Slack in spectator-frame buffer. At 10 Hz this is 6.4s — enough for a slow client to
/// briefly stall without dropping frames; lagged clients log a `Lagged` warning rather
/// than disconnect.
const SPECTATOR_BROADCAST_BUFFER: usize = 64;

const BANNER: &str = r#"
========================================
   Naval Battle Server
========================================
"#;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,naval_server=debug")),
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

    let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(8);
    let (room_tx, room_rx) = mpsc::channel(ROOM_EVENT_BUFFER);
    let (spec_tx, _) = broadcast::channel::<SpectatorFrame>(SPECTATOR_BROADCAST_BUFFER);

    let mut main_room = Room::new(
        "main".into(),
        config.map.0 as f32,
        config.map.1 as f32,
        config.seed,
        config.tick_hz,
        config.tick_deadline_ms,
        config.max_bots,
    );
    main_room.set_spectator_broadcast(spec_tx.clone());
    let room_handle = tokio::spawn(room::run_room(main_room, room_rx, shutdown_tx.subscribe()));

    let net_handle = tokio::spawn(net::run(
        config.clone(),
        room_tx.clone(),
        spec_tx.clone(),
        shutdown_tx.subscribe(),
    ));
    let control_handle = tokio::spawn(control::run(shutdown_tx.clone(), room_tx.clone()));

    tokio::select! {
        res = tokio::signal::ctrl_c() => {
            match res {
                Ok(_) => info!("ctrl-c received, shutting down"),
                Err(e) => tracing::error!(error = %e, "ctrl-c handler failed"),
            }
        }
        res = control_handle => {
            match res {
                Ok(_) => info!("control task ended"),
                Err(e) => tracing::error!(error = %e, "control task panicked"),
            }
        }
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
