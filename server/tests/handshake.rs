//! Phase 4.1 acceptance: a client connects, sends `hello`, receives `welcome` with
//! assigned IDs, sends `ready`, and receives no further messages until game start.

use std::time::Duration;

use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::Message;

use naval_server::{
    config::Config,
    net,
    room::{run_room, Room, RoomEvent, SpectatorFrame, ROOM_EVENT_BUFFER},
};
use tokio::sync::oneshot;

struct ServerHandle {
    port: u16,
    shutdown: broadcast::Sender<()>,
    room_tx: mpsc::Sender<RoomEvent>,
}

/// Spin a server + room pair on a free port. Returns the port, a shutdown sender that
/// terminates both tasks when fired, and a handle to the room's event channel so tests
/// can simulate operator commands.
async fn start_server() -> ServerHandle {
    let probe = TcpListener::bind("127.0.0.1:0").await.expect("probe bind");
    let port = probe.local_addr().expect("local_addr").port();
    drop(probe);

    let mut config = Config::parse_from(["test"]);
    config.port = port;
    config.tick_hz = 50; // fast loop so shutdown observation is snappy

    let (shutdown_tx, _) = broadcast::channel::<()>(4);
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
        "main".to_string(),
        naval_server::auth::AuthState::new("test-password".to_string(), 3600),
        room_tx.clone(),
        spec_tx,
        shutdown_tx.clone(),
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hello_yields_welcome_and_ready_is_silent() {
    let ServerHandle {
        port,
        shutdown,
        room_tx: _,
    } = start_server().await;
    let url = format!("ws://127.0.0.1:{port}/bot");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("ws connect");

    ws.send(Message::Text(
        r#"{"type":"hello","name":"captain_kirk","version":"1.0"}"#.into(),
    ))
    .await
    .expect("send hello");

    let text = recv_text(&mut ws).await;
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("welcome is JSON");
    assert_eq!(parsed["type"], "welcome", "frame: {text}");
    assert!(parsed["bot_id"].is_string(), "bot_id missing in {text}");
    assert!(parsed["ship_id"].is_string(), "ship_id missing in {text}");
    let bot_id = parsed["bot_id"].as_str().unwrap().to_string();
    let ship_id = parsed["ship_id"].as_str().unwrap().to_string();
    assert!(bot_id.starts_with("b_"), "bot_id={bot_id}");
    assert!(ship_id.starts_with("s_"), "ship_id={ship_id}");
    assert_eq!(parsed["map"]["width"], 700);
    assert_eq!(parsed["map"]["height"], 700);
    assert_eq!(parsed["tick_hz"], 50);

    // After ready, the server should stay silent until game_start (Phase 4.2).
    ws.send(Message::Text(r#"{"type":"ready"}"#.into()))
        .await
        .expect("send ready");

    let res = tokio::time::timeout(Duration::from_millis(400), ws.next()).await;
    match res {
        Err(_) => {} // timeout: no further messages, as required
        Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => {}
        Ok(other) => panic!("unexpected frame after ready: {other:?}"),
    }

    let _ = ws.close(None).await;
    let _ = shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_bots_get_distinct_ids() {
    let ServerHandle {
        port,
        shutdown,
        room_tx: _,
    } = start_server().await;
    let url = format!("ws://127.0.0.1:{port}/bot");

    let (mut ws_a, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("ws a connect");
    ws_a.send(Message::Text(
        r#"{"type":"hello","name":"a","version":"1"}"#.into(),
    ))
    .await
    .expect("send hello a");
    let a: serde_json::Value =
        serde_json::from_str(&recv_text(&mut ws_a).await).expect("welcome a");

    let (mut ws_b, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("ws b connect");
    ws_b.send(Message::Text(
        r#"{"type":"hello","name":"b","version":"1"}"#.into(),
    ))
    .await
    .expect("send hello b");
    let b: serde_json::Value =
        serde_json::from_str(&recv_text(&mut ws_b).await).expect("welcome b");

    assert_ne!(a["bot_id"], b["bot_id"], "bots must get distinct IDs");
    assert_ne!(a["ship_id"], b["ship_id"], "bots must get distinct ships");

    let _ = ws_a.close(None).await;
    let _ = ws_b.close(None).await;
    let _ = shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn non_hello_first_frame_is_rejected() {
    let ServerHandle {
        port,
        shutdown,
        room_tx: _,
    } = start_server().await;
    let url = format!("ws://127.0.0.1:{port}/bot");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("ws connect");

    // Send `ready` before `hello` — server should reply with an error frame.
    ws.send(Message::Text(r#"{"type":"ready"}"#.into()))
        .await
        .expect("send");

    let text = recv_text(&mut ws).await;
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("error is JSON");
    assert_eq!(parsed["type"], "error", "frame: {text}");
    assert_eq!(parsed["code"], "invalid_message");

    let _ = ws.close(None).await;
    let _ = shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn operator_start_delivers_game_start_to_ready_bot() {
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
        r#"{"type":"hello","name":"only_bot","version":"1.0"}"#.into(),
    ))
    .await
    .expect("send hello");

    // Drain `welcome`.
    let welcome: serde_json::Value =
        serde_json::from_str(&recv_text(&mut ws).await).expect("welcome");
    assert_eq!(welcome["type"], "welcome");

    ws.send(Message::Text(r#"{"type":"ready"}"#.into()))
        .await
        .expect("send ready");

    // Give the room a moment to record the ready flag.
    tokio::time::sleep(Duration::from_millis(80)).await;

    // Simulate the operator typing `room start main`.
    let (reply_tx, reply_rx) = oneshot::channel();
    room_tx
        .send(RoomEvent::OperatorStart {
            room: "main".into(),
            reply: reply_tx,
        })
        .await
        .expect("send start");
    reply_rx
        .await
        .expect("oneshot")
        .expect("start should succeed");

    // Bot should receive `game_start`.
    let game_start: serde_json::Value =
        serde_json::from_str(&recv_text(&mut ws).await).expect("game_start");
    assert_eq!(game_start["type"], "game_start", "frame: {game_start}");
    assert_eq!(game_start["tick"], 0);
    assert!(game_start["starting_position"].is_array());
    let pos = &game_start["starting_position"];
    assert!(pos[0].is_number() && pos[1].is_number());
    assert!(game_start["starting_heading_deg"].is_number());

    let _ = ws.close(None).await;
    let _ = shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn operator_start_refused_when_no_bots_ready() {
    let ServerHandle {
        port: _,
        shutdown,
        room_tx,
    } = start_server().await;

    let (reply_tx, reply_rx) = oneshot::channel();
    room_tx
        .send(RoomEvent::OperatorStart {
            room: "main".into(),
            reply: reply_tx,
        })
        .await
        .expect("send start");
    let result = reply_rx.await.expect("oneshot");
    assert!(result.is_err(), "start with no bots should fail");

    let _ = shutdown.send(());
}
