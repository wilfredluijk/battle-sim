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
                println!("  room list               (not yet implemented)");
                println!("  room abort <name>       (not yet implemented)");
                println!("  room kick <name> <bot>  (not yet implemented)");
                println!("  seed <value>            (not yet implemented)");
            }
            "room" => match rest.as_slice() {
                ["start", name] => {
                    handle_room_start(&room_tx, name).await;
                }
                ["list"] | ["abort", _] | ["kick", _, _] => {
                    info!(args = ?rest, "control: room command (not implemented yet)");
                }
                _ => warn!("usage: room start <name>"),
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
