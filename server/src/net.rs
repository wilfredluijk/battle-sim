use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::protocol::{self, error_code, BotMsg, ServerMsg};
use crate::room::{BotRegistration, RoomEvent};

/// After this many protocol violations, the bot connection is closed.
const MAX_VIOLATIONS: u32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Endpoint {
    Bot,
    Spectator,
}

type Ws = WebSocketStream<TcpStream>;
type WsSink = SplitSink<Ws, Message>;
type WsStream = SplitStream<Ws>;

pub async fn run(
    config: Config,
    room_tx: mpsc::Sender<RoomEvent>,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    let addr: SocketAddr = ([0, 0, 0, 0], config.port).into();
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(%addr, error = %e, "failed to bind TCP listener");
            return;
        }
    };
    info!(%addr, "websocket listener bound (paths: /bot, /spectate)");

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("net: shutdown signal received");
                break;
            }
            res = listener.accept() => {
                match res {
                    Ok((stream, peer)) => {
                        let conn_shutdown = shutdown_rx.resubscribe();
                        let room_tx = room_tx.clone();
                        tokio::spawn(handle_connection(stream, peer, room_tx, conn_shutdown));
                    }
                    Err(e) => {
                        warn!(error = %e, "accept failed");
                    }
                }
            }
        }
    }
}

async fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    room_tx: mpsc::Sender<RoomEvent>,
    shutdown_rx: broadcast::Receiver<()>,
) {
    let path_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let path_slot_cb = path_slot.clone();

    // The handshake callback type returns a large `Result` (the `ErrorResponse` body
    // is owned). We never produce an `Err` here — the closure exists only to capture
    // the request path — so the size doesn't matter.
    #[allow(clippy::result_large_err)]
    let callback = move |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let path = req.uri().path().to_string();
        *path_slot_cb.lock().expect("path slot poisoned") = Some(path);
        Ok(resp)
    };

    let ws = match tokio_tungstenite::accept_hdr_async(stream, callback).await {
        Ok(ws) => ws,
        Err(e) => {
            warn!(%peer, error = %e, "websocket handshake failed");
            return;
        }
    };

    let path = path_slot
        .lock()
        .expect("path slot poisoned")
        .clone()
        .unwrap_or_default();

    let endpoint = match path.as_str() {
        "/bot" => Endpoint::Bot,
        "/spectate" => Endpoint::Spectator,
        other => {
            warn!(%peer, path = other, "unknown websocket path; closing");
            close_with_reason(ws, CloseCode::Policy, format!("unknown path `{other}`")).await;
            return;
        }
    };

    info!(%peer, ?endpoint, "websocket connected");

    match endpoint {
        Endpoint::Bot => handle_bot(peer, ws, room_tx, shutdown_rx).await,
        Endpoint::Spectator => handle_spectator(peer, ws, shutdown_rx).await,
    }

    info!(%peer, ?endpoint, "connection ended");
}

async fn handle_bot(
    peer: SocketAddr,
    ws: Ws,
    room_tx: mpsc::Sender<RoomEvent>,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    let (mut sink, mut stream) = ws.split();
    let mut violations: u32 = 0;

    // Phase 1: wait for `hello`. Reject other typed messages and malformed frames as
    // protocol violations; disconnect after MAX_VIOLATIONS.
    let (name, version) = match wait_for_hello(
        peer,
        &mut sink,
        &mut stream,
        &mut shutdown_rx,
        &mut violations,
    )
    .await
    {
        Some(hello) => hello,
        None => return,
    };

    // Phase 2: register with the room and grab our outbound channel.
    let registration = match register(peer, &room_tx, name, version, &mut sink).await {
        Some(r) => r,
        None => return,
    };
    let bot_id = registration.bot_id.clone();
    let mut outbound_rx = registration.outbound;

    info!(%peer, bot_id = %bot_id, ship_id = %registration.ship_id, "bot handshake complete");

    // Phase 3: main loop — forward inbound bot messages to the room and outbound server
    // messages to the websocket.
    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!(%peer, bot_id = %bot_id, "closing bot connection (shutdown)");
                let _ = sink.send(Message::Close(None)).await;
                break;
            }
            outbound = outbound_rx.recv() => {
                let Some(msg) = outbound else {
                    debug!(%peer, bot_id = %bot_id, "room dropped outbound channel");
                    break;
                };
                if !send_server_msg(&mut sink, &msg).await {
                    break;
                }
            }
            frame = stream.next() => {
                match frame {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<BotMsg>(&text) {
                            Ok(BotMsg::Hello { .. }) => {
                                violations += 1;
                                warn!(%peer, bot_id = %bot_id, violations, "duplicate hello");
                                send_error(
                                    &mut sink,
                                    error_code::INVALID_MESSAGE,
                                    "hello already received for this connection",
                                ).await;
                                if violations >= MAX_VIOLATIONS {
                                    disconnect_for_violations(&mut sink).await;
                                    break;
                                }
                            }
                            Ok(BotMsg::Ready) => {
                                if room_tx
                                    .send(RoomEvent::BotReady { bot_id: bot_id.clone() })
                                    .await
                                    .is_err()
                                {
                                    debug!(%peer, "room channel closed; ending bot loop");
                                    break;
                                }
                            }
                            Ok(BotMsg::Command { tick, .. }) => {
                                // Phase 4.3 wires command handling. For 4.1 we acknowledge
                                // syntactic validity and drop the value.
                                debug!(%peer, bot_id = %bot_id, tick, "command received (not yet handled)");
                            }
                            Err(e) => {
                                violations += 1;
                                let code = if matches!(
                                    e.classify(),
                                    serde_json::error::Category::Syntax
                                ) {
                                    error_code::MALFORMED_JSON
                                } else {
                                    error_code::INVALID_MESSAGE
                                };
                                warn!(%peer, code, error = %e, violations, "rejected bot frame");
                                send_error(&mut sink, code, e.to_string()).await;
                                if violations >= MAX_VIOLATIONS {
                                    disconnect_for_violations(&mut sink).await;
                                    break;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        violations += 1;
                        warn!(%peer, bytes = bytes.len(), violations, "binary frame on /bot");
                        send_error(
                            &mut sink,
                            error_code::BINARY_FRAMES_UNSUPPORTED,
                            "this endpoint only accepts text JSON frames",
                        )
                        .await;
                        if violations >= MAX_VIOLATIONS {
                            disconnect_for_violations(&mut sink).await;
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = sink.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(frame))) => {
                        info!(%peer, bot_id = %bot_id, ?frame, "bot closed");
                        break;
                    }
                    Some(Ok(Message::Frame(_))) => {}
                    Some(Err(e)) => {
                        warn!(%peer, error = %e, "ws read error");
                        break;
                    }
                    None => {
                        info!(%peer, bot_id = %bot_id, "bot stream ended");
                        break;
                    }
                }
            }
        }
    }

    // Best-effort notify the room. If the channel is gone (server shutting down) the
    // room is already tearing down its bookkeeping.
    let _ = room_tx.send(RoomEvent::BotDisconnect { bot_id }).await;
}

/// Read frames until we get a valid `hello` or the connection ends. Pings are answered;
/// non-hello messages and malformed frames count as protocol violations.
async fn wait_for_hello(
    peer: SocketAddr,
    sink: &mut WsSink,
    stream: &mut WsStream,
    shutdown_rx: &mut broadcast::Receiver<()>,
    violations: &mut u32,
) -> Option<(String, String)> {
    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                let _ = sink.send(Message::Close(None)).await;
                return None;
            }
            frame = stream.next() => {
                match frame {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<BotMsg>(&text) {
                            Ok(BotMsg::Hello { name, version }) => {
                                return Some((name, version));
                            }
                            Ok(_) => {
                                *violations += 1;
                                warn!(%peer, violations = *violations, "non-hello before handshake");
                                send_error(
                                    sink,
                                    error_code::INVALID_MESSAGE,
                                    "first message must be `hello`",
                                ).await;
                                if *violations >= MAX_VIOLATIONS {
                                    disconnect_for_violations(sink).await;
                                    return None;
                                }
                            }
                            Err(e) => {
                                *violations += 1;
                                let code = if matches!(
                                    e.classify(),
                                    serde_json::error::Category::Syntax
                                ) {
                                    error_code::MALFORMED_JSON
                                } else {
                                    error_code::INVALID_MESSAGE
                                };
                                warn!(%peer, code, error = %e, violations = *violations, "rejected pre-handshake frame");
                                send_error(sink, code, e.to_string()).await;
                                if *violations >= MAX_VIOLATIONS {
                                    disconnect_for_violations(sink).await;
                                    return None;
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        *violations += 1;
                        warn!(%peer, bytes = bytes.len(), violations = *violations, "binary frame before handshake");
                        send_error(
                            sink,
                            error_code::BINARY_FRAMES_UNSUPPORTED,
                            "this endpoint only accepts text JSON frames",
                        ).await;
                        if *violations >= MAX_VIOLATIONS {
                            disconnect_for_violations(sink).await;
                            return None;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = sink.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(frame))) => {
                        info!(%peer, ?frame, "bot closed before handshake");
                        return None;
                    }
                    Some(Ok(Message::Frame(_))) => {}
                    Some(Err(e)) => {
                        warn!(%peer, error = %e, "ws read error before handshake");
                        return None;
                    }
                    None => {
                        info!(%peer, "bot stream ended before handshake");
                        return None;
                    }
                }
            }
        }
    }
}

/// Send `BotConnect` to the room and await registration. Reports failures back to the
/// bot via an `error` frame.
async fn register(
    peer: SocketAddr,
    room_tx: &mpsc::Sender<RoomEvent>,
    name: String,
    version: String,
    sink: &mut WsSink,
) -> Option<BotRegistration> {
    let (reply_tx, reply_rx) = oneshot::channel();
    if room_tx
        .send(RoomEvent::BotConnect {
            peer,
            name,
            version,
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        warn!(%peer, "room event channel closed; refusing connection");
        send_error(sink, error_code::INVALID_MESSAGE, "server is shutting down").await;
        return None;
    }
    match reply_rx.await {
        Ok(Ok(reg)) => Some(reg),
        Ok(Err(e)) => {
            warn!(%peer, reason = e.as_str(), "room rejected join");
            send_error(sink, error_code::INVALID_MESSAGE, e.as_str()).await;
            None
        }
        Err(_) => {
            warn!(%peer, "room dropped registration reply");
            None
        }
    }
}

async fn handle_spectator(peer: SocketAddr, ws: Ws, mut shutdown_rx: broadcast::Receiver<()>) {
    let (mut sink, mut stream) = ws.split();

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!(%peer, "closing spectator connection (shutdown)");
                let _ = sink.send(Message::Close(None)).await;
                break;
            }
            frame = stream.next() => {
                match frame {
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = sink.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Close(frame))) => {
                        info!(%peer, ?frame, "spectator closed");
                        break;
                    }
                    Some(Ok(_)) => {
                        // Spectators are read-only — silently drop anything they send.
                    }
                    Some(Err(e)) => {
                        warn!(%peer, error = %e, "ws read error");
                        break;
                    }
                    None => {
                        info!(%peer, "spectator stream ended");
                        break;
                    }
                }
            }
        }
    }
}

async fn send_error(sink: &mut WsSink, code: &str, message: impl Into<String>) {
    send_server_msg(sink, &protocol::error_msg(code, message)).await;
}

/// Returns `false` if the underlying socket failed; the caller should treat that as a
/// terminal condition for the connection.
async fn send_server_msg(sink: &mut WsSink, msg: &ServerMsg) -> bool {
    let payload = serde_json::to_string(msg).expect("ServerMsg always serializes");
    match sink.send(Message::Text(payload)).await {
        Ok(()) => true,
        Err(e) => {
            debug!(error = %e, "failed to send server frame");
            false
        }
    }
}

async fn disconnect_for_violations(sink: &mut WsSink) {
    send_error(
        sink,
        error_code::TOO_MANY_VIOLATIONS,
        format!("disconnecting after {MAX_VIOLATIONS} violations"),
    )
    .await;
    let _ = sink
        .send(Message::Close(Some(CloseFrame {
            code: CloseCode::Policy,
            reason: "too many protocol violations".into(),
        })))
        .await;
}

async fn close_with_reason(ws: Ws, code: CloseCode, reason: String) {
    let (mut sink, _) = ws.split();
    let _ = sink
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await;
}
