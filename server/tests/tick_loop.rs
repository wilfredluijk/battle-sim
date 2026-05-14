//! Phase 3.3 acceptance: the room tick loop ticks at the configured rate and produces
//! monotonic, sequential tick numbers under shutdown.

use std::time::Duration;

use tokio::sync::{broadcast, mpsc};

use naval_server::room::{run_room, Room, ROOM_EVENT_BUFFER};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_room_ticks_at_configured_rate() {
    let (tx, rx) = broadcast::channel::<()>(2);
    let (_evt_tx, evt_rx) = mpsc::channel(ROOM_EVENT_BUFFER);
    let room = Room::new("test".into(), 1000.0, 1000.0, 42, 10, 80, 4);
    let handle = tokio::spawn(run_room(room, evt_rx, rx, false));

    // Let it run for ~550ms. At 10Hz with `interval` (first tick fires immediately),
    // expect ~6 ticks ±1 for OS scheduling jitter.
    tokio::time::sleep(Duration::from_millis(550)).await;
    let _ = tx.send(());

    let final_tick = handle.await.expect("room task");
    assert!(
        (4..=8).contains(&final_tick),
        "expected ~6 ticks in 550ms at 10Hz, got {final_tick}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_room_shuts_down_promptly() {
    let (tx, rx) = broadcast::channel::<()>(2);
    let (_evt_tx, evt_rx) = mpsc::channel(ROOM_EVENT_BUFFER);
    let room = Room::new("test".into(), 1000.0, 1000.0, 42, 100, 80, 4);
    let handle = tokio::spawn(run_room(room, evt_rx, rx, false));

    tokio::time::sleep(Duration::from_millis(50)).await;
    let _ = tx.send(());

    // The room must observe the shutdown within one tick period (10ms at 100Hz) plus slack.
    let res = tokio::time::timeout(Duration::from_millis(500), handle).await;
    assert!(res.is_ok(), "room did not shut down within 500ms");
}
