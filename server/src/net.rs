use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::protocol::{self, error_code, BotMsg};

/// After this many protocol violations, the bot connection is closed.
const MAX_VIOLATIONS: u32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Endpoint {
    Bot,
    Spectator,
}

type Ws = WebSocketStream<TcpStream>;

pub async fn run(config: Config, mut shutdown_rx: broadcast::Receiver<()>) {
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
                        tokio::spawn(handle_connection(stream, peer, conn_shutdown));
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
    shutdown_rx: broadcast::Receiver<()>,
) {
    let path_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let path_slot_cb = path_slot.clone();

    let callback =
        move |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
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
        Endpoint::Bot => handle_bot(peer, ws, shutdown_rx).await,
        Endpoint::Spectator => handle_spectator(peer, ws, shutdown_rx).await,
    }

    info!(%peer, ?endpoint, "connection ended");
}

async fn handle_bot(peer: SocketAddr, ws: Ws, mut shutdown_rx: broadcast::Receiver<()>) {
    let (mut sink, mut stream) = ws.split();
    let mut violations: u32 = 0;

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!(%peer, "closing bot connection (shutdown)");
                let _ = sink.send(Message::Close(None)).await;
                break;
            }
            frame = stream.next() => {
                match frame {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<BotMsg>(&text) {
                            Ok(msg) => {
                                debug!(%peer, ?msg, "bot message");
                                // Phase 4 will hand this to the room.
                            }
                            Err(e) => {
                                let code = if matches!(e.classify(), serde_json::error::Category::Syntax) {
                                    error_code::MALFORMED_JSON
                                } else {
                                    error_code::INVALID_MESSAGE
                                };
                                violations += 1;
                                warn!(%peer, code, error = %e, violations, "rejected bot frame");
                                send_error(&mut sink, code, e.to_string()).await;
                                if violations >= MAX_VIOLATIONS {
                                    send_error(
                                        &mut sink,
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
                            let _ = sink.send(Message::Close(None)).await;
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = sink.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(frame))) => {
                        info!(%peer, ?frame, "bot closed");
                        break;
                    }
                    Some(Ok(Message::Frame(_))) => {}
                    Some(Err(e)) => {
                        warn!(%peer, error = %e, "ws read error");
                        break;
                    }
                    None => {
                        info!(%peer, "bot stream ended");
                        break;
                    }
                }
            }
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

async fn send_error<S>(sink: &mut S, code: &str, message: impl Into<String>)
where
    S: SinkExt<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::fmt::Display,
{
    let payload = serde_json::to_string(&protocol::error_msg(code, message))
        .expect("ServerMsg::Error always serializes");
    if let Err(e) = sink.send(Message::Text(payload)).await {
        debug!(error = %e, "failed to send error frame");
    }
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
