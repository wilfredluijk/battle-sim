//! Phase 11.1 acceptance: server spins in-process, two scripted bots connect, the match
//! runs to completion, and a winner is declared.
//!
//! The killer bot drives forward at full throttle with active radar and fires every tick
//! at whichever ship lands in its contact list. The victim sits at its spawn with passive
//! sensors and never fires. Because both ships are placed on the §5.6 starting ring 180°
//! apart and facing the centre, the killer's straight-line course passes directly through
//! the victim — once active radar acquires the victim, every shot is a near-direct hit.

use std::time::Duration;

use clap::Parser;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use naval_server::{
    config::Config,
    net,
    room::{run_room, Room, RoomEvent, SpectatorFrame, ROOM_EVENT_BUFFER},
};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
type WsSink = SplitSink<Ws, Message>;
type WsStream = SplitStream<Ws>;

struct ServerHandle {
    port: u16,
    shutdown: broadcast::Sender<()>,
    room_tx: mpsc::Sender<RoomEvent>,
}

async fn start_server() -> ServerHandle {
    let probe = TcpListener::bind("127.0.0.1:0").await.expect("probe bind");
    let port = probe.local_addr().expect("local_addr").port();
    drop(probe);

    let mut config = Config::parse_from(["test"]);
    config.port = port;
    // 100 Hz keeps the test under ~10s in the common case while leaving plenty of headroom
    // for the tokio interval to keep up on a loaded CI runner.
    config.tick_hz = 100;
    config.tick_deadline_ms = 80;

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

async fn recv_text(stream: &mut WsStream) -> String {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(60), stream.next())
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

#[derive(Debug)]
struct BotOutcome {
    bot_id: String,
    winner: Option<String>,
    final_tick: u64,
}

/// Scripted "killer" bot: drives forward at full throttle with active radar and fires at
/// the first contact in every tick payload until `game_over`. Returns the `game_over`
/// payload so the test can assert on the winner.
async fn run_killer_bot(port: u16, name: &str) -> BotOutcome {
    let url = format!("ws://127.0.0.1:{port}/bot");
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("killer ws connect");
    let (mut sink, mut stream) = ws.split();

    let bot_id = handshake(&mut sink, &mut stream, name).await;
    drive_until_game_over(&mut sink, &mut stream, bot_id, /*shoot=*/ true).await
}

/// Scripted "victim" bot: stays put with passive sensors and never fires. Returns the
/// `game_over` payload so the test can cross-check that both sides see the same winner.
async fn run_victim_bot(port: u16, name: &str) -> BotOutcome {
    let url = format!("ws://127.0.0.1:{port}/bot");
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("victim ws connect");
    let (mut sink, mut stream) = ws.split();

    let bot_id = handshake(&mut sink, &mut stream, name).await;
    drive_until_game_over(&mut sink, &mut stream, bot_id, /*shoot=*/ false).await
}

async fn handshake(sink: &mut WsSink, stream: &mut WsStream, name: &str) -> String {
    sink.send(Message::Text(format!(
        r#"{{"type":"hello","name":"{name}","version":"1.0"}}"#
    )))
    .await
    .expect("send hello");

    let welcome: serde_json::Value =
        serde_json::from_str(&recv_text(stream).await).expect("welcome");
    assert_eq!(welcome["type"], "welcome");
    let bot_id = welcome["bot_id"]
        .as_str()
        .expect("bot_id in welcome")
        .to_string();

    sink.send(Message::Text(r#"{"type":"ready"}"#.into()))
        .await
        .expect("send ready");
    bot_id
}

async fn drive_until_game_over(
    sink: &mut WsSink,
    stream: &mut WsStream,
    bot_id: String,
    shoot: bool,
) -> BotOutcome {
    loop {
        let text = recv_text(stream).await;
        let msg: serde_json::Value = serde_json::from_str(&text).expect("server frame is JSON");
        match msg["type"].as_str() {
            Some("game_start") => continue,
            Some("tick") => {
                let tick = msg["tick"].as_u64().expect("tick number");
                let cmd = if shoot {
                    let contacts = msg["contacts"].as_array();
                    let fire = contacts
                        .and_then(|cs| cs.first())
                        .and_then(|c| {
                            let bearing = c["bearing_deg"].as_f64()?;
                            // Active contacts include a numeric `range`; passive contacts
                            // would have `range: null`, in which case we skip the shot.
                            let range = c["range"].as_f64()?;
                            Some(format!(
                                r#","fire":{{"bearing_deg":{bearing},"range":{range}}}"#
                            ))
                        })
                        .unwrap_or_default();
                    format!(
                        r#"{{"type":"command","tick":{tick},"throttle":1.0,"rudder":0.0,"sensor_mode":"active"{fire}}}"#
                    )
                } else {
                    format!(
                        r#"{{"type":"command","tick":{tick},"throttle":0.0,"rudder":0.0,"sensor_mode":"passive"}}"#
                    )
                };
                sink.send(Message::Text(cmd)).await.expect("send command");
            }
            Some("game_over") => {
                let winner = msg["winner"].as_str().map(str::to_string);
                let final_tick = msg["final_tick"].as_u64().expect("final_tick");
                let _ = sink.send(Message::Close(None)).await;
                return BotOutcome {
                    bot_id,
                    winner,
                    final_tick,
                };
            }
            Some("error") => {
                // Non-fatal during a match (e.g. cooldown_active when we spam fire). Log
                // and keep going; the test only cares that game_over eventually arrives.
                eprintln!("bot {bot_id} got error frame: {text}");
            }
            _ => {} // welcome, lobby, etc. — ignore.
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_bots_play_a_match_to_completion_and_a_winner_is_declared() {
    let ServerHandle {
        port,
        shutdown,
        room_tx,
    } = start_server().await;

    // Start both bots first; once they're both registered and ready, the operator starts
    // the match. The two tasks then play to game_over and return their final payloads.
    let killer = tokio::spawn(async move { run_killer_bot(port, "killer").await });
    let victim = tokio::spawn(async move { run_victim_bot(port, "victim").await });

    // Wait long enough for both handshakes + ready signals to land in the room.
    tokio::time::sleep(Duration::from_millis(400)).await;

    let (reply_tx, reply_rx) = oneshot::channel();
    room_tx
        .send(RoomEvent::OperatorStart {
            room: "main".into(),
            reply: reply_tx,
        })
        .await
        .expect("send operator start");
    reply_rx
        .await
        .expect("oneshot reply")
        .expect("operator start should succeed");

    // 60s wall-clock cap. Expected match length at 100 Hz: ~10s for the killer to close
    // and land four splash hits. The 3000-tick simulation timeout (30s at 100 Hz) acts as
    // a backstop inside the room.
    let outcomes = tokio::time::timeout(Duration::from_secs(60), async move {
        (
            killer.await.expect("killer task"),
            victim.await.expect("victim task"),
        )
    })
    .await
    .expect("match did not finish in 60s");

    let (killer_out, victim_out) = outcomes;

    // Both bots received the same game_over.
    assert_eq!(
        killer_out.winner, victim_out.winner,
        "killer and victim disagree on the winner",
    );
    assert_eq!(
        killer_out.final_tick, victim_out.final_tick,
        "final_tick mismatch between bots",
    );

    // A winner must be declared. The killer is firing and the victim is stationary, so
    // the killer should win — assert that explicitly to catch regressions where the
    // tiebreaker or game-over logic flips.
    let winner = killer_out
        .winner
        .as_ref()
        .expect("a winner must be declared");
    assert_eq!(
        winner, &killer_out.bot_id,
        "killer (bot_id={}) should win over victim (bot_id={}); got winner={winner}",
        killer_out.bot_id, victim_out.bot_id,
    );

    let _ = shutdown.send(());
}
