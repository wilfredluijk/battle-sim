//! End-to-end auth tests for the `/admin` WebSocket: bad tokens get HTTP 401, good
//! tokens get past the upgrade and receive an initial state snapshot.

use std::time::Duration;

use clap::Parser;
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::Message;

use naval_server::{
    config::Config,
    net,
    room::{run_room, Room, SpectatorFrame, ROOM_EVENT_BUFFER},
};

const ADMIN_TOKEN: &str = "test-admin-token-xyz";

struct ServerHandle {
    port: u16,
    shutdown: broadcast::Sender<()>,
}

async fn start_server() -> ServerHandle {
    let probe = TcpListener::bind("127.0.0.1:0").await.expect("probe bind");
    let port = probe.local_addr().expect("local_addr").port();
    drop(probe);

    let mut config = Config::parse_from(["test"]);
    config.port = port;
    config.tick_hz = 50;

    let (shutdown_tx, shutdown_rx_net) = broadcast::channel::<()>(4);
    let (room_tx, room_rx) = mpsc::channel(ROOM_EVENT_BUFFER);

    let mut room = Room::new(
        "main".into(),
        config.map.0 as f32,
        config.map.1 as f32,
        config.seed,
        config.tick_hz,
        config.tick_deadline_ms,
        config.max_bots,
    );
    let (admin_tx, _admin_rx) = broadcast::channel(8);
    room.set_admin_broadcast(admin_tx.clone());
    tokio::spawn(run_room(room, room_rx, shutdown_tx.subscribe()));
    let (spec_tx, _spec_rx) = broadcast::channel::<SpectatorFrame>(8);

    tokio::spawn(net::run(
        config,
        std::sync::Arc::new(ADMIN_TOKEN.to_string()),
        room_tx,
        spec_tx,
        shutdown_rx_net,
    ));
    tokio::time::sleep(Duration::from_millis(150)).await;
    ServerHandle {
        port,
        shutdown: shutdown_tx,
    }
}

/// Raw HTTP upgrade request — lets us inspect a 401 response that never gets to the
/// WS handshake. The `tokio_tungstenite::connect_async` path would swallow the body.
async fn raw_get_upgrade(port: u16, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("tcp connect");
    let req = format!(
        "GET {path} HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\
         \r\n"
    );
    stream
        .write_all(req.as_bytes())
        .await
        .expect("send request");
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut buf)).await;
    let text = String::from_utf8_lossy(&buf).to_string();
    let status: u16 = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, text)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admin_endpoint_rejects_missing_token() {
    let ServerHandle { port, shutdown } = start_server().await;
    let (status, body) = raw_get_upgrade(port, "/admin").await;
    assert_eq!(status, 401, "missing token must 401, got: {body}");
    let _ = shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admin_endpoint_rejects_wrong_token() {
    let ServerHandle { port, shutdown } = start_server().await;
    let (status, body) = raw_get_upgrade(port, "/admin?token=wrong").await;
    assert_eq!(status, 401, "bad token must 401, got: {body}");
    let _ = shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admin_endpoint_accepts_good_token_and_pushes_state() {
    let ServerHandle { port, shutdown } = start_server().await;
    let url = format!("ws://127.0.0.1:{port}/admin?token={ADMIN_TOKEN}");
    let (mut ws, response) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("ws connect");
    assert_eq!(response.status(), 101, "websocket upgrade succeeded");

    // First frame must be a state snapshot of the room.
    let frame = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("ws read error");
    let text = match frame {
        Message::Text(t) => t,
        other => panic!("expected text frame, got {other:?}"),
    };
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(parsed["type"], "state");
    assert_eq!(parsed["room"], "main");
    assert_eq!(parsed["state"], "lobby");
    let _ = shutdown.send(());
}
