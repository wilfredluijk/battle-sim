use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(name = "naval-server", about = "Naval battle game server", version)]
pub struct Config {
    /// TCP port to listen on for WebSocket connections
    #[arg(long, default_value_t = 7878)]
    pub port: u16,

    /// Simulation tick rate in Hz
    #[arg(long, default_value_t = 10)]
    pub tick_hz: u32,

    /// Per-tick deadline for collecting bot commands, in milliseconds
    #[arg(long, default_value_t = 80)]
    pub tick_deadline_ms: u64,

    /// Map size in WIDTHxHEIGHT units (e.g. 1000x1000)
    #[arg(long, default_value = "1000x1000", value_parser = parse_map_size)]
    pub map: (u32, u32),

    /// Maximum number of bots per room
    #[arg(long, default_value_t = 4)]
    pub max_bots: u32,

    /// RNG seed used to drive the deterministic simulation
    #[arg(long, default_value_t = 42)]
    pub seed: u64,

    /// Directory where replay JSONL logs are written
    #[arg(long, default_value = "./replays")]
    pub replay_dir: PathBuf,

    /// Replay an existing JSONL log instead of accepting bot connections. Spectators may
    /// still connect; the room ticks at `--tick-hz` and broadcasts as if it were live.
    #[arg(long, value_name = "FILE")]
    pub replay: Option<PathBuf>,

    /// Maximum simultaneous TCP connections from a single peer IP. Above this, additional
    /// connects from that IP are refused at accept time. Set to 0 to disable the cap.
    #[arg(long, default_value_t = 8)]
    pub max_connections_per_ip: u32,

    /// Seconds to wait for the HTTP head and the WebSocket `hello` message before forcibly
    /// dropping a half-open connection. Closes the slow-loris vector on the handshake.
    #[arg(long, default_value_t = 5)]
    pub handshake_timeout_secs: u64,

    /// Tournament mode: restrict the `/spectate` endpoint to loopback (127.0.0.1, ::1) so
    /// competing bots cannot subscribe to ground-truth world state. Bots may still join
    /// over the network.
    #[arg(long, default_value_t = false)]
    pub tournament: bool,

    /// Override the default 3000-tick match timeout. Used by integration tests so a
    /// scenario can declare "run N ticks and stop" instead of waiting for the standard
    /// match cap. The simulation is unchanged; only the end condition shifts.
    #[arg(long, value_name = "N")]
    pub max_ticks: Option<u64>,

    /// Start the match automatically once `--max-bots` bots are connected and `ready`,
    /// skipping the `room start` operator command. Designed for headless / scripted runs
    /// where there is no human at the stdin prompt.
    #[arg(long, default_value_t = false)]
    pub auto_start: bool,
}

fn parse_map_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s
        .split_once('x')
        .ok_or_else(|| format!("expected WIDTHxHEIGHT, got `{s}`"))?;
    let width: u32 = w.parse().map_err(|e| format!("invalid width `{w}`: {e}"))?;
    let height: u32 = h
        .parse()
        .map_err(|e| format!("invalid height `{h}`: {e}"))?;
    if width == 0 || height == 0 {
        return Err("map dimensions must be greater than zero".into());
    }
    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let cfg = Config::parse_from(["naval-server"]);
        assert_eq!(cfg.port, 7878);
        assert_eq!(cfg.tick_hz, 10);
        assert_eq!(cfg.tick_deadline_ms, 80);
        assert_eq!(cfg.map, (1000, 1000));
        assert_eq!(cfg.max_bots, 4);
        assert_eq!(cfg.seed, 42);
    }

    #[test]
    fn map_size_parses() {
        assert_eq!(parse_map_size("1000x1000"), Ok((1000, 1000)));
        assert_eq!(parse_map_size("640x480"), Ok((640, 480)));
        assert!(parse_map_size("1000").is_err());
        assert!(parse_map_size("0x500").is_err());
        assert!(parse_map_size("axb").is_err());
    }
}
