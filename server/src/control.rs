use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{info, warn};

use crate::room::RoomEvent;

pub async fn run(shutdown_tx: broadcast::Sender<()>, room_tx: mpsc::Sender<RoomEvent>) {
    let (line_tx, mut line_rx) = mpsc::channel::<String>(64);

    // Stdin reads block on Windows, so push them through a dedicated thread.
    std::thread::spawn(move || {
        use std::io::BufRead;
        let stdin = std::io::stdin();
        let mut handle = stdin.lock();
        let mut buf = String::new();
        loop {
            buf.clear();
            match handle.read_line(&mut buf) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let line = buf.trim_start_matches('\u{feff}').trim().to_string();
                    if line.is_empty() {
                        continue;
                    }
                    if line_tx.blocking_send(line).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("stdin read error: {e}");
                    break;
                }
            }
        }
    });

    while let Some(line) = line_rx.recv().await {
        let mut parts = line.split_whitespace();
        let Some(cmd) = parts.next() else { continue };
        let rest: Vec<&str> = parts.collect();
        match cmd {
            "quit" | "exit" => {
                info!("control: quit received");
                let _ = shutdown_tx.send(());
                break;
            }
            "help" | "?" => {
                println!("commands:");
                println!("  quit | exit             shutdown the server");
                println!(
                    "  room start <name>       transition the named room from lobby to running"
                );
                println!("  room abort              force-end the running match (no winner)");
                println!("  room reset              return the room to lobby (only when ended)");
                println!("  room kick <bot_id>      disconnect a bot by id");
                println!("  room list               (not yet implemented)");
                println!("  seed <value>            (not yet implemented)");
                println!();
                println!("  Note: the same actions are exposed over the WS /admin endpoint;");
                println!("        see the admin token printed at server start.");
            }
            "room" => match rest.as_slice() {
                ["start", name] => {
                    handle_room_start(&room_tx, name).await;
                }
                ["abort"] => {
                    handle_room_abort(&room_tx).await;
                }
                ["reset"] => {
                    handle_room_reset(&room_tx).await;
                }
                ["kick", bot_id] => {
                    handle_room_kick(&room_tx, bot_id).await;
                }
                ["list"] => {
                    info!(args = ?rest, "control: room list (not implemented yet)");
                }
                _ => {
                    warn!("usage: room start <name> | room abort | room reset | room kick <bot_id>")
                }
            },
            "seed" => {
                info!(args = ?rest, "control: seed command (not implemented yet)");
            }
            other => {
                warn!(cmd = other, "control: unknown command (try `help`)");
            }
        }
    }

    info!("control: stdin closed");
}

async fn handle_room_start(room_tx: &mpsc::Sender<RoomEvent>, name: &str) {
    let (reply_tx, reply_rx) = oneshot::channel();
    if room_tx
        .send(RoomEvent::OperatorStart {
            room: name.to_string(),
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        warn!(room = name, "control: room channel closed; cannot start");
        return;
    }
    match reply_rx.await {
        Ok(Ok(())) => println!("room `{name}` started"),
        Ok(Err(e)) => println!("room `{name}` start refused: {}", e.as_str()),
        Err(_) => warn!(room = name, "control: room dropped start reply"),
    }
}

async fn handle_room_abort(room_tx: &mpsc::Sender<RoomEvent>) {
    let (reply_tx, reply_rx) = oneshot::channel();
    if room_tx
        .send(RoomEvent::OperatorAbort { reply: reply_tx })
        .await
        .is_err()
    {
        warn!("control: room channel closed; cannot abort");
        return;
    }
    match reply_rx.await {
        Ok(Ok(())) => println!("match aborted"),
        Ok(Err(e)) => println!("abort refused: {}", e.as_str()),
        Err(_) => warn!("control: room dropped abort reply"),
    }
}

async fn handle_room_reset(room_tx: &mpsc::Sender<RoomEvent>) {
    let (reply_tx, reply_rx) = oneshot::channel();
    if room_tx
        .send(RoomEvent::OperatorReset { reply: reply_tx })
        .await
        .is_err()
    {
        warn!("control: room channel closed; cannot reset");
        return;
    }
    match reply_rx.await {
        Ok(Ok(())) => println!("room returned to lobby"),
        Ok(Err(e)) => println!("reset refused: {}", e.as_str()),
        Err(_) => warn!("control: room dropped reset reply"),
    }
}

async fn handle_room_kick(room_tx: &mpsc::Sender<RoomEvent>, bot_id: &str) {
    let (reply_tx, reply_rx) = oneshot::channel();
    if room_tx
        .send(RoomEvent::OperatorKick {
            bot_id: bot_id.to_string(),
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        warn!(bot = bot_id, "control: room channel closed; cannot kick");
        return;
    }
    match reply_rx.await {
        Ok(Ok(())) => println!("kicked bot `{bot_id}`"),
        Ok(Err(e)) => println!("kick refused: {}", e.as_str()),
        Err(_) => warn!(bot = bot_id, "control: room dropped kick reply"),
    }
}
