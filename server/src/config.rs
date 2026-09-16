use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(name = "naval-server", about = "Naval battle game server", version)]
pub struct Config {
    /// Probe the local HTTP readiness endpoint and exit (container health check).
    #[arg(long, default_value_t = false)]
    pub healthcheck: bool,
    /// Protected JSON participant roster. Required unless explicitly running local development.
    #[arg(long, env = "BATTLE_BOT_CREDENTIALS_FILE")]
    pub bot_credentials_file: Option<PathBuf>,
    /// Development only: allow bots without participant credentials.
    #[arg(long, default_value_t = false)]
    pub allow_unauthenticated_bots: bool,
    /// Read administrator password from a protected file (preferred in production).
    #[arg(long, env = "BATTLE_ADMIN_PASSWORD_FILE")]
    pub admin_password_file: Option<PathBuf>,

    /// TCP port to listen on for WebSocket connections
    #[arg(long, default_value_t = 7878)]
    pub port: u16,

    /// Wall-clock tick rate in Hz (physics always advances 0.1 seconds per step)
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=1000))]
    pub tick_hz: u32,

    /// Per-tick deadline for collecting bot commands, in milliseconds
    #[arg(long, default_value_t = 80)]
    pub tick_deadline_ms: u64,

    /// Map size in WIDTHxHEIGHT units (e.g. 1000x1000)
    #[arg(long, default_value = "700x700", value_parser = parse_map_size)]
    pub map: (u32, u32),

    /// Maximum number of bots per room
    #[arg(long, default_value_t = 24, value_parser = clap::value_parser!(u32).range(1..=256))]
    pub max_bots: u32,

    /// RNG seed used to drive the deterministic simulation
    #[arg(long, default_value_t = 42)]
    pub seed: u64,

    /// Directory where replay JSONL logs are written
    #[arg(long, default_value = "./replays")]
    pub replay_dir: PathBuf,

    /// Replay an existing JSONL log instead of accepting bot connections. Spectators may
    /// still connect; playback uses the recorded tick rate and freezes on the final frame.
    #[arg(long, value_name = "FILE")]
    pub replay: Option<PathBuf>,

    /// Maximum WebSocket connections per peer IP. The production proxy also limits HTTP.
    #[arg(long, default_value_t = 25)]
    pub max_connections_per_ip: u32,

    /// WebSocket hello timeout. HTTP header timeout is configured at the production proxy.
    #[arg(long, default_value_t = 5)]
    pub handshake_timeout_secs: u64,

    /// Tournament mode: fresh secret seeds and constrained random starts per match.
    /// Administrator authentication is always required, including on loopback.
    #[arg(long, default_value_t = false)]
    pub tournament: bool,

    /// Workshop-only compatibility mode whose sensor jitter stream is reproducible from
    /// public bot/tick data. Hidden from normal help and disabled unless explicitly set.
    #[arg(
        long,
        env = "BATTLE_WORKSHOP_PUBLIC_SENSOR_STREAM",
        default_value_t = false,
        hide = true
    )]
    pub workshop_public_sensor_stream: bool,

    /// Admin password for the REST control plane. `POST /api/login` checks this value and
    /// issues a JWT. Required unless a password file is provided; never logged. Can also
    /// be supplied via the `BATTLE_ADMIN_PASSWORD` environment variable.
    #[arg(long, env = "BATTLE_ADMIN_PASSWORD", value_name = "PASSWORD")]
    pub admin_password: Option<String>,

    /// Lifetime of an issued admin JWT, in hours.
    #[arg(long, default_value_t = 12)]
    pub token_ttl_hours: u64,
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
        assert_eq!(cfg.map, (700, 700));
        assert_eq!(cfg.max_bots, 24);
        assert_eq!(cfg.seed, 42);
        assert!(!cfg.workshop_public_sensor_stream);
    }

    #[test]
    fn workshop_sensor_stream_requires_explicit_opt_in() {
        let cfg = Config::parse_from(["naval-server", "--workshop-public-sensor-stream"]);
        assert!(cfg.workshop_public_sensor_stream);
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
