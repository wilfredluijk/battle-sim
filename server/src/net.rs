use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::protocol::{self, error_code, BotMsg, ServerMsg};
use crate::room::{BotRegistration, PendingCommand, RoomEvent, SpectatorFrame};

/// After this many protocol violations, the bot connection is closed.
const MAX_VIOLATIONS: u32 = 5;

/// Cap on the number of bytes we will buffer while reading the HTTP request head. Real
/// requests fit comfortably inside 2 KiB; anything larger is almost certainly hostile.
const MAX_HEAD_BYTES: usize = 8 * 1024;

/// Static spectator assets, embedded at compile time so the server has no runtime path
/// dependency on the `spectator/` directory.
static INDEX_HTML: &str = include_str!("../../spectator/index.html");
static RENDER_JS: &str = include_str!("../../spectator/render.js");
static STYLE_CSS: &str = include_str!("../../spectator/style.css");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Endpoint {
    Bot,
    Spectator,
}

type Ws = WebSocketStream<PrefixedStream>;
type WsSink = SplitSink<Ws, Message>;
type WsStream = SplitStream<Ws>;

pub async fn run(
    config: Config,
    room_tx: mpsc::Sender<RoomEvent>,
    spec_tx: broadcast::Sender<SpectatorFrame>,
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
    info!(
        %addr,
        "listener bound (HTTP /, WS /bot, WS /spectate)"
    );

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
                        let spec_tx = spec_tx.clone();
                        tokio::spawn(handle_connection(stream, peer, room_tx, spec_tx, conn_shutdown));
                    }
                    Err(e) => {
                        warn!(error = %e, "accept failed");
                    }
                }
            }
        }
    }
}

/// Read the HTTP request head, then dispatch: WS upgrade → `/bot` or `/spectate`,
/// plain HTTP GET → static file response.
async fn handle_connection(
    mut stream: TcpStream,
    peer: SocketAddr,
    room_tx: mpsc::Sender<RoomEvent>,
    spec_tx: broadcast::Sender<SpectatorFrame>,
    shutdown_rx: broadcast::Receiver<()>,
) {
    let head_bytes = match read_http_head(&mut stream).await {
        Ok(b) => b,
        Err(e) => {
            debug!(%peer, error = %e, "failed reading HTTP head");
            return;
        }
    };

    let parsed = match parse_request(&head_bytes) {
        Ok(p) => p,
        Err(e) => {
            warn!(%peer, error = %e, "malformed HTTP request");
            let _ = write_http_response(
                &mut stream,
                400,
                "Bad Request",
                "text/plain; charset=utf-8",
                b"bad request",
            )
            .await;
            return;
        }
    };

    if parsed.is_websocket_upgrade {
        let prefixed = PrefixedStream::new(head_bytes, stream);
        let ws = match tokio_tungstenite::accept_async(prefixed).await {
            Ok(ws) => ws,
            Err(e) => {
                warn!(%peer, error = %e, "websocket handshake failed");
                return;
            }
        };

        let endpoint = match parsed.path.as_str() {
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
            Endpoint::Spectator => handle_spectator(peer, ws, spec_tx, shutdown_rx).await,
        }
        info!(%peer, ?endpoint, "connection ended");
    } else {
        // Plain HTTP — static file serving.
        if !parsed.method.eq_ignore_ascii_case("GET") {
            let _ = write_http_response(
                &mut stream,
                405,
                "Method Not Allowed",
                "text/plain; charset=utf-8",
                b"method not allowed",
            )
            .await;
            return;
        }
        match resolve_static(&parsed.path) {
            Some((content_type, body)) => {
                debug!(%peer, path = %parsed.path, "serving static asset");
                let _ = write_http_response(&mut stream, 200, "OK", content_type, body).await;
            }
            None => {
                debug!(%peer, path = %parsed.path, "static asset not found");
                let _ = write_http_response(
                    &mut stream,
                    404,
                    "Not Found",
                    "text/plain; charset=utf-8",
                    b"not found",
                )
                .await;
            }
        }
    }
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
                            Ok(BotMsg::Command {
                                tick,
                                throttle,
                                rudder,
                                fire,
                                sensor_mode,
                            }) => {
                                let command = PendingCommand {
                                    tick,
                                    throttle,
                                    rudder,
                                    sensor_mode,
                                    fire,
                                };
                                if room_tx
                                    .send(RoomEvent::BotCommand {
                                        bot_id: bot_id.clone(),
                                        command,
                                    })
                                    .await
                                    .is_err()
                                {
                                    debug!(%peer, "room channel closed; ending bot loop");
                                    break;
                                }
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

async fn handle_spectator(
    peer: SocketAddr,
    ws: Ws,
    spec_tx: broadcast::Sender<SpectatorFrame>,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    let (mut sink, mut stream) = ws.split();
    let mut spec_rx = spec_tx.subscribe();
    info!(%peer, subscribers = spec_tx.receiver_count(), "spectator subscribed");

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!(%peer, "closing spectator connection (shutdown)");
                let _ = sink.send(Message::Close(None)).await;
                break;
            }
            recv = spec_rx.recv() => {
                match recv {
                    Ok(frame) => {
                        // The Arc avoids re-allocating per subscriber, but the WS sink
                        // still needs an owned `String` per send.
                        if sink.send(Message::Text((*frame).clone())).await.is_err() {
                            debug!(%peer, "spectator sink closed");
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(%peer, skipped, "spectator lagging; dropped frames");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!(%peer, "spectator broadcast closed");
                        break;
                    }
                }
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

// ---------------------------------------------------------------------------
// HTTP head reading and dispatch
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ParsedRequest {
    method: String,
    path: String,
    is_websocket_upgrade: bool,
}

/// Read raw bytes from the stream until we see `\r\n\r\n` (end of HTTP request head),
/// then return the buffered bytes verbatim. The buffer is replayed to the WS handshake
/// via `PrefixedStream` so tungstenite can re-parse the request itself.
async fn read_http_head(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 512];
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed before HTTP head was complete",
            ));
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return Ok(buf);
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP head exceeded size limit",
            ));
        }
    }
}

fn parse_request(buf: &[u8]) -> Result<ParsedRequest, String> {
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut req = httparse::Request::new(&mut headers);
    match req.parse(buf).map_err(|e| e.to_string())? {
        httparse::Status::Complete(_) => {}
        httparse::Status::Partial => return Err("partial request".into()),
    }
    let method = req.method.unwrap_or("").to_string();
    let path = req.path.unwrap_or("/").to_string();
    let upgrade_to_ws = req.headers.iter().any(|h| {
        h.name.eq_ignore_ascii_case("Upgrade") && h.value.eq_ignore_ascii_case(b"websocket")
    });
    let connection_upgrade = req.headers.iter().any(|h| {
        if !h.name.eq_ignore_ascii_case("Connection") {
            return false;
        }
        let value = std::str::from_utf8(h.value).unwrap_or("");
        value
            .split(',')
            .any(|p| p.trim().eq_ignore_ascii_case("upgrade"))
    });
    Ok(ParsedRequest {
        method,
        path,
        is_websocket_upgrade: upgrade_to_ws && connection_upgrade,
    })
}

/// Map a request path to an embedded asset. Anything outside this small whitelist 404s,
/// so directory traversal isn't a concern.
fn resolve_static(path: &str) -> Option<(&'static str, &'static [u8])> {
    let path_only = path.split('?').next().unwrap_or(path);
    match path_only {
        "/" | "/index.html" => Some(("text/html; charset=utf-8", INDEX_HTML.as_bytes())),
        "/render.js" => Some((
            "application/javascript; charset=utf-8",
            RENDER_JS.as_bytes(),
        )),
        "/style.css" => Some(("text/css; charset=utf-8", STYLE_CSS.as_bytes())),
        _ => None,
    }
}

async fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    status_text: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: {content_type}\r\nContent-Length: {len}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        len = body.len(),
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// PrefixedStream: replay the buffered HTTP head before delegating to the TCP stream
// ---------------------------------------------------------------------------

/// `AsyncRead + AsyncWrite` adapter that emits a fixed prefix of buffered bytes before
/// reading from the wrapped `TcpStream`. We use this so the WebSocket handshake sees
/// the same request bytes we already consumed for path-routing.
pub struct PrefixedStream {
    prefix: Vec<u8>,
    cursor: usize,
    inner: TcpStream,
}

impl PrefixedStream {
    pub fn new(prefix: Vec<u8>, inner: TcpStream) -> Self {
        Self {
            prefix,
            cursor: 0,
            inner,
        }
    }
}

impl AsyncRead for PrefixedStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        if me.cursor < me.prefix.len() {
            let remaining = &me.prefix[me.cursor..];
            let n = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..n]);
            me.cursor += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut me.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for PrefixedStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(bytes: &[u8]) -> ParsedRequest {
        parse_request(bytes).expect("parse")
    }

    #[test]
    fn parses_plain_get() {
        let r = req(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert_eq!(r.method, "GET");
        assert_eq!(r.path, "/");
        assert!(!r.is_websocket_upgrade);
    }

    #[test]
    fn detects_websocket_upgrade() {
        let r = req(b"GET /bot HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: x\r\nSec-WebSocket-Version: 13\r\n\r\n");
        assert!(r.is_websocket_upgrade);
        assert_eq!(r.path, "/bot");
    }

    #[test]
    fn handles_compound_connection_header() {
        // Some clients send `Connection: keep-alive, Upgrade`.
        let r = req(b"GET /spectate HTTP/1.1\r\nUpgrade: websocket\r\nConnection: keep-alive, Upgrade\r\nSec-WebSocket-Key: x\r\nSec-WebSocket-Version: 13\r\n\r\n");
        assert!(r.is_websocket_upgrade);
        assert_eq!(r.path, "/spectate");
    }

    #[test]
    fn static_routes() {
        assert!(resolve_static("/").is_some());
        assert!(resolve_static("/index.html").is_some());
        assert!(resolve_static("/render.js").is_some());
        assert!(resolve_static("/style.css").is_some());
        assert!(resolve_static("/etc/passwd").is_none());
        // Query strings are stripped before matching.
        assert!(resolve_static("/?cachebust=1").is_some());
    }
}
