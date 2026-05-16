//! Phase 4.3 acceptance: a connected, ready, started bot drives through real ticks —
//! the server emits `tick` frames, the bot replies with `command`s, and the resulting
//! throttle/rudder move the ship between ticks.

use std::time::Duration;

use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

use naval_server::{
    config::Config,
    net,
    room::{run_room, Room, RoomEvent, SpectatorFrame, ROOM_EVENT_BUFFER},
};

struct ServerHandle {
    port: u16,
    shutdown: broadcast::Sender<()>,
    room_tx: mpsc::Sender<RoomEvent>,
}

async fn start_server() -> ServerHandle {
    start_server_with(50, 80).await
}

async fn start_server_with(tick_hz: u32, tick_deadline_ms: u64) -> ServerHandle {
    let probe = TcpListener::bind("127.0.0.1:0").await.expect("probe bind");
    let port = probe.local_addr().expect("local_addr").port();
    drop(probe);

    let mut config = Config::parse_from(["test"]);
    config.port = port;
    config.tick_hz = tick_hz;
    config.tick_deadline_ms = tick_deadline_ms;

    let (shutdown_tx, shutdown_rx_net) = broadcast::channel::<()>(4);
    let (room_tx, room_rx) = mpsc::channel(ROOM_EVENT_BUFFER);

    let room = Room::new(
        "main".into(),
        config.map.0 as f32,
        config.map.1 as f32,
        config.seed,
        config.tick_hz,
        config.tick_deadline_ms,
        config.max_bots,
    );
    tokio::spawn(run_room(room, room_rx, shutdown_tx.subscribe()));
    let (spec_tx, _spec_rx) = broadcast::channel::<SpectatorFrame>(8);
    tokio::spawn(net::run(
        config,
        std::sync::Arc::new("test-admin-token".to_string()),
        room_tx.clone(),
        spec_tx,
        shutdown_rx_net,
    ));

    tokio::time::sleep(Duration::from_millis(150)).await;
    ServerHandle {
        port,
        shutdown: shutdown_tx,
        room_tx,
    }
}

async fn recv_text<S>(ws: &mut S) -> String
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(2), ws.next())
            .await
            .expect("timeout waiting for frame")
            .expect("stream ended unexpectedly")
            .expect("ws read error");
        match frame {
            Message::Text(t) => return t,
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("expected text frame, got {other:?}"),
        }
    }
}

async fn recv_typed<S>(ws: &mut S, expected_type: &str) -> serde_json::Value
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let text = recv_text(ws).await;
        let v: serde_json::Value =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("bad json: {text} ({e})"));
        if v["type"] == expected_type {
            return v;
        }
        // Stray welcome/game_start/etc. — keep reading until we hit the type we want.
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_throttle_command_moves_the_ship_each_tick() {
    let ServerHandle {
        port,
        shutdown,
        room_tx,
    } = start_server().await;
    let url = format!("ws://127.0.0.1:{port}/bot");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("ws connect");

    ws.send(Message::Text(
        r#"{"type":"hello","name":"runner","version":"1.0"}"#.into(),
    ))
    .await
    .expect("send hello");
    let _welcome = recv_typed(&mut ws, "welcome").await;

    ws.send(Message::Text(r#"{"type":"ready"}"#.into()))
        .await
        .expect("send ready");
    tokio::time::sleep(Duration::from_millis(80)).await;

    let (reply_tx, reply_rx) = oneshot::channel();
    room_tx
        .send(RoomEvent::OperatorStart {
            room: "main".into(),
            reply: reply_tx,
        })
        .await
        .expect("send start");
    reply_rx.await.expect("oneshot").expect("start ok");

    let game_start = recv_typed(&mut ws, "game_start").await;
    let start_pos = (
        game_start["starting_position"][0].as_f64().unwrap() as f32,
        game_start["starting_position"][1].as_f64().unwrap() as f32,
    );

    // Drive 5 ticks at full throttle. After each tick, send a matching command back.
    let mut last_pos = start_pos;
    let mut moved_at_least_once = false;
    for _ in 0..5 {
        let tick_msg = recv_typed(&mut ws, "tick").await;
        let tick = tick_msg["tick"].as_u64().expect("tick number");
        let pos = (
            tick_msg["self"]["pos"][0].as_f64().unwrap() as f32,
            tick_msg["self"]["pos"][1].as_f64().unwrap() as f32,
        );
        if (pos.0 - last_pos.0).abs() > 1e-3 || (pos.1 - last_pos.1).abs() > 1e-3 {
            moved_at_least_once = true;
        }
        last_pos = pos;

        let cmd = format!(
            r#"{{"type":"command","tick":{tick},"throttle":1.0,"rudder":0.0,"sensor_mode":"passive"}}"#
        );
        ws.send(Message::Text(cmd)).await.expect("send command");
    }

    assert!(
        moved_at_least_once,
        "ship should have moved between ticks under full throttle (start={start_pos:?}, last={last_pos:?})"
    );

    let _ = ws.close(None).await;
    let _ = shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_command_yields_error_and_keeps_bot_alive() {
    // Slow tick (5Hz = 200ms) + tight deadline (10ms) so a tiny sleep makes us late.
    let ServerHandle {
        port,
        shutdown,
        room_tx,
    } = start_server_with(5, 10).await;
    let url = format!("ws://127.0.0.1:{port}/bot");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("ws connect");

    ws.send(Message::Text(
        r#"{"type":"hello","name":"slow","version":"1.0"}"#.into(),
    ))
    .await
    .expect("send hello");
    let _ = recv_typed(&mut ws, "welcome").await;
    ws.send(Message::Text(r#"{"type":"ready"}"#.into()))
        .await
        .expect("send ready");
    tokio::time::sleep(Duration::from_millis(80)).await;

    let (reply_tx, reply_rx) = oneshot::channel();
    room_tx
        .send(RoomEvent::OperatorStart {
            room: "main".into(),
            reply: reply_tx,
        })
        .await
        .expect("send start");
    reply_rx.await.expect("oneshot").expect("start ok");
    let _ = recv_typed(&mut ws, "game_start").await;

    let first = recv_typed(&mut ws, "tick").await;
    let tick = first["tick"].as_u64().unwrap();

    // Sleep well past the 10ms deadline before responding.
    tokio::time::sleep(Duration::from_millis(120)).await;

    let cmd = format!(
        r#"{{"type":"command","tick":{tick},"throttle":1.0,"rudder":1.0,"sensor_mode":"active"}}"#
    );
    ws.send(Message::Text(cmd))
        .await
        .expect("send late command");

    // Expect a `late_command` error frame on this stream.
    let mut got_late = false;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(800);
    while tokio::time::Instant::now() < deadline {
        let res = tokio::time::timeout(Duration::from_millis(400), ws.next()).await;
        let Ok(Some(Ok(Message::Text(text)))) = res else {
            continue;
        };
        let v: serde_json::Value = serde_json::from_str(&text).expect("frame");
        if v["type"] == "error" && v["code"] == "late_command" {
            got_late = true;
            break;
        }
    }
    assert!(got_late, "expected a late_command error frame");

    // Bot is still alive: subsequent ticks still come, and the previous-tick controls
    // (defaulted to 0 from game_start) persist — the late command did NOT take effect.
    let next = recv_typed(&mut ws, "tick").await;
    let throttle = next["self"]["throttle"].as_f64().unwrap() as f32;
    let rudder = next["self"]["rudder"].as_f64().unwrap() as f32;
    assert_eq!(throttle, 0.0, "late command must not have applied throttle");
    assert_eq!(rudder, 0.0, "late command must not have applied rudder");

    let _ = ws.close(None).await;
    let _ = shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_bot_keeps_previous_controls() {
    let ServerHandle {
        port,
        shutdown,
        room_tx,
    } = start_server().await;
    let url = format!("ws://127.0.0.1:{port}/bot");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("ws connect");

    ws.send(Message::Text(
        r#"{"type":"hello","name":"idle","version":"1.0"}"#.into(),
    ))
    .await
    .expect("send hello");
    let _ = recv_typed(&mut ws, "welcome").await;

    ws.send(Message::Text(r#"{"type":"ready"}"#.into()))
        .await
        .expect("send ready");
    tokio::time::sleep(Duration::from_millis(80)).await;

    let (reply_tx, reply_rx) = oneshot::channel();
    room_tx
        .send(RoomEvent::OperatorStart {
            room: "main".into(),
            reply: reply_tx,
        })
        .await
        .expect("send start");
    reply_rx.await.expect("oneshot").expect("start ok");
    let _ = recv_typed(&mut ws, "game_start").await;

    // Send one command, then go silent and verify throttle/rudder persist in subsequent
    // tick frames. Because tick frames may already be in flight when the command lands,
    // skip frames until we observe the commanded controls, then assert they stick for
    // several follow-up frames.
    let first = recv_typed(&mut ws, "tick").await;
    let tick = first["tick"].as_u64().unwrap();
    let cmd = format!(
        r#"{{"type":"command","tick":{tick},"throttle":0.6,"rudder":-0.2,"sensor_mode":"passive"}}"#
    );
    ws.send(Message::Text(cmd)).await.expect("send command");

    let mut applied = false;
    for _ in 0..20 {
        let frame = recv_typed(&mut ws, "tick").await;
        let throttle = frame["self"]["throttle"].as_f64().unwrap() as f32;
        let rudder = frame["self"]["rudder"].as_f64().unwrap() as f32;
        if (throttle - 0.6).abs() < 1e-4 && (rudder + 0.2).abs() < 1e-4 {
            applied = true;
            break;
        }
    }
    assert!(
        applied,
        "command should have been applied within a few ticks"
    );

    for _ in 0..3 {
        let frame = recv_typed(&mut ws, "tick").await;
        let throttle = frame["self"]["throttle"].as_f64().unwrap() as f32;
        let rudder = frame["self"]["rudder"].as_f64().unwrap() as f32;
        assert!(
            (throttle - 0.6).abs() < 1e-4,
            "throttle persistence broken: {throttle}"
        );
        assert!(
            (rudder + 0.2).abs() < 1e-4,
            "rudder persistence broken: {rudder}"
        );
    }

    let _ = ws.close(None).await;
    let _ = shutdown.send(());
}
