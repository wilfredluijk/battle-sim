//! End-to-end test for Phase 2.2 acceptance: sending malformed input over `/bot`
//! results in a typed `error` reply and does not crash the server.

use std::time::Duration;

use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::Message;

use naval_server::{
    config::Config,
    net,
    room::{SpectatorFrame, ROOM_EVENT_BUFFER},
};

async fn start_server() -> (u16, broadcast::Sender<()>) {
    // Find a free port by binding to :0, then drop and let the server re-bind.
    let probe = TcpListener::bind("127.0.0.1:0").await.expect("probe bind");
    let port = probe.local_addr().expect("local_addr").port();
    drop(probe);

    let mut config = Config::parse_from(["test"]);
    config.port = port;

    let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(4);
    // No room is wired up — these tests only exercise pre-handshake validation, which
    // never reaches the room. A leaked sender keeps the channel open.
    let (room_tx, _room_rx) = mpsc::channel(ROOM_EVENT_BUFFER);
    let (spec_tx, _spec_rx) = broadcast::channel::<SpectatorFrame>(8);
    tokio::spawn(net::run(
        config,
        std::sync::Arc::new("test-admin-token".to_string()),
        room_tx,
        spec_tx,
        shutdown_rx,
    ));

    // Give the listener a moment to bind on the freed port.
    tokio::time::sleep(Duration::from_millis(150)).await;
    (port, shutdown_tx)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_object_yields_typed_error_and_does_not_crash() {
    let (port, shutdown) = start_server().await;

    let url = format!("ws://127.0.0.1:{port}/bot");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("ws connect");

    ws.send(Message::Text("{}".into())).await.expect("send");

    let text = recv_text(&mut ws).await;
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("error frame is JSON");
    assert_eq!(parsed["type"], "error", "frame: {text}");
    assert!(parsed["code"].is_string(), "missing code in {text}");
    assert!(parsed["message"].is_string(), "missing message in {text}");

    // Server is still alive: send a second bad frame, expect another error reply.
    ws.send(Message::Text("not json".into()))
        .await
        .expect("send 2");
    let text2 = recv_text(&mut ws).await;
    let parsed2: serde_json::Value = serde_json::from_str(&text2).unwrap();
    assert_eq!(parsed2["type"], "error");

    let _ = ws.close(None).await;
    let _ = shutdown.send(());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn five_violations_disconnects_with_close() {
    let (port, shutdown) = start_server().await;

    let url = format!("ws://127.0.0.1:{port}/bot");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("ws connect");

    for i in 0..10 {
        if ws
            .send(Message::Text(format!(r#"{{"bad":{i}}}"#)))
            .await
            .is_err()
        {
            break;
        }
    }

    let mut got_close = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        let res = tokio::time::timeout(Duration::from_millis(500), ws.next()).await;
        match res {
            Ok(Some(Ok(Message::Close(_)))) => {
                got_close = true;
                break;
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(_))) | Ok(None) => {
                got_close = true;
                break;
            }
            Err(_) => continue, // per-iter timeout, keep waiting
        }
    }
    assert!(
        got_close,
        "expected server to close the connection after repeated violations"
    );

    let _ = shutdown.send(());
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
