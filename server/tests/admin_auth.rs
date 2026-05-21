//! End-to-end auth tests for the REST control plane: `POST /api/login` mints a JWT only
//! for the correct password, and mutating `/api/*` routes require that token.

use std::time::Duration;

use clap::Parser;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};

use naval_server::{
    auth::AuthState,
    config::Config,
    net,
    room::{run_room, Room, SpectatorFrame, ROOM_EVENT_BUFFER},
};

const ADMIN_PASSWORD: &str = "test-password";

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

    let auth = AuthState::new(ADMIN_PASSWORD.to_string(), 3600);
    tokio::spawn(net::run(
        config,
        "main".to_string(),
        auth,
        room_tx,
        spec_tx,
        shutdown_tx.clone(),
    ));
    tokio::time::sleep(Duration::from_millis(150)).await;
    ServerHandle {
        port,
        shutdown: shutdown_tx,
    }
}

/// Minimal raw-TCP HTTP/1.1 client. Returns the response status code and body.
async fn http_request(
    port: u16,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    json_body: Option<&str>,
) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("tcp connect");
    let body = json_body.unwrap_or("");
    let mut req =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    if json_body.is_some() {
        req.push_str("Content-Type: application/json\r\n");
    }
    req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    if let Some(token) = bearer {
        req.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);

    stream
        .write_all(req.as_bytes())
        .await
        .expect("send request");
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut buf)).await;
    let text = String::from_utf8_lossy(&buf).to_string();
    let status: u16 = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

/// Log in and return the bearer token.
async fn login(port: u16) -> String {
    let (status, body) = http_request(
        port,
        "POST",
        "/api/login",
        None,
        Some(&format!(r#"{{"password":"{ADMIN_PASSWORD}"}}"#)),
    )
    .await;
    assert_eq!(status, 200, "login should succeed, body: {body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("login json");
    parsed["token"].as_str().expect("token field").to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn login_rejects_wrong_password() {
    let ServerHandle { port, shutdown } = start_server().await;
    let (status, body) = http_request(
        port,
        "POST",
        "/api/login",
        None,
        Some(r#"{"password":"nope"}"#),
    )
    .await;
    assert_eq!(status, 401, "wrong password must 401, got: {body}");
    let _ = shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn login_accepts_correct_password_and_returns_token() {
    let ServerHandle { port, shutdown } = start_server().await;
    let token = login(port).await;
    assert!(!token.is_empty(), "token should not be empty");
    let _ = shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutating_route_requires_token() {
    let ServerHandle { port, shutdown } = start_server().await;
    let (status, body) = http_request(port, "POST", "/api/room/start", None, None).await;
    assert_eq!(status, 401, "start without a token must 401, got: {body}");
    let _ = shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutating_route_accepts_valid_token() {
    let ServerHandle { port, shutdown } = start_server().await;
    let token = login(port).await;
    let (status, body) = http_request(port, "POST", "/api/room/start", Some(&token), None).await;
    // A valid token gets past auth; with no bots connected the room refuses the start
    // with 409 — the point is it is no longer a 401.
    assert_ne!(status, 401, "valid token must not be rejected, got: {body}");
    assert_eq!(status, 409, "empty room should refuse start, got: {body}");
    let _ = shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn room_state_is_public() {
    let ServerHandle { port, shutdown } = start_server().await;
    let (status, body) = http_request(port, "GET", "/api/room", None, None).await;
    assert_eq!(status, 200, "GET /api/room should be public, got: {body}");
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("room json");
    assert_eq!(parsed["state"], "lobby");
    assert_eq!(parsed["room"], "main");
    assert!(parsed["config"].is_object(), "config block present");
    let _ = shutdown.send(());
}
