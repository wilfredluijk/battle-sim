use tokio::sync::{broadcast, mpsc};
use tracing::{info, warn};

pub async fn run(shutdown_tx: broadcast::Sender<()>) {
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
                println!("commands: quit | room create|list|start|abort|kick ... | seed <value>");
            }
            "room" => {
                info!(args = ?rest, "control: room command (not implemented yet)");
            }
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
