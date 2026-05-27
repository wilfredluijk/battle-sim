//! Replay log: a JSONL record of every match for re-running the simulation later.
//!
//! Format (one JSON object per line):
//!   1. Header (line 0): version, seed, tick rate, map size, bots in registration order.
//!   2. Tick records: one per tick the room produces in `Running` state, listing the
//!      commands actually applied that tick (sorted by `bot_id`, matching the order the
//!      simulation processed them).
//!   3. End record: terminal line with the final tick and winner.
//!
//! Determinism (see `CLAUDE.md`): the log captures only the *inputs* that drove the
//! simulation. Replaying = re-running the same simulation with the same seed and feeding
//! the recorded commands back through `Room::step_tick`. State is reconstructed, never
//! deserialized.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{interval, MissedTickBehavior};
use tracing::{info, warn};

use crate::protocol::{
    Contact, FireCommand, MapInfo, SensorMode, ServerMsg, SpectatorMsg, TickEvent,
};
use crate::room::{PendingCommand, Room, RoomEvent, SpectatorFrame};
use crate::sim::{PowerupId, SimConfig};

/// Bumped on any breaking change to the on-disk format. Readers reject mismatched versions
/// rather than silently misinterpreting old logs.
///
/// v2 added the `sim_config` field to the header so a replay rebuilds the simulation with
/// the exact balance parameters the live run used. v1 logs are no longer readable.
///
/// v3 added per-bot `selected_powerups` to the header and the `activate_powerup` field on
/// each `ReplayCommand`. The `SimConfig` embedded in the header gained a `powerups` field;
/// older logs that omit it are accepted by the deserializer because `PowerupConfig` has a
/// `serde(default)`, but the explicit version bump catches any other shape drift.
pub const REPLAY_FORMAT_VERSION: u32 = 3;

/// One line of the JSONL log. Internally tagged so the discriminator field (`type`) sits
/// alongside the variant payload — the on-disk shape is exactly what bot authors see when
/// they `cat` the file.
///
/// The header is boxed because it now carries a fat `SimConfig` (incl. `PowerupConfig`)
/// and `Vec<ReplayBot>` — boxing keeps the enum size proportional to the smallest variant.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReplayRecord {
    Header(Box<ReplayHeader>),
    Tick(ReplayTick),
    End(ReplayEnd),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ReplayHeader {
    pub version: u32,
    pub replay_id: String,
    pub room: String,
    pub seed: u64,
    pub tick_hz: u32,
    pub tick_deadline_ms: u64,
    pub map: MapInfo,
    pub max_bots: u32,
    /// Balance parameters the live run used. Replaying re-applies these so the simulation
    /// is bit-identical even when the match used non-default ship/weapon/sensor values.
    pub sim_config: SimConfig,
    pub bots: Vec<ReplayBot>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ReplayBot {
    pub bot_id: String,
    pub ship_id: String,
    pub name: String,
    /// Powerups this bot picked for the match (in pick order). Empty if the bot never
    /// sent `select_powerups`. Re-applied by the replay player before any tick is stepped
    /// so activation commands replay against the same loadout the live run saw.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_powerups: Vec<PowerupId>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ReplayTick {
    pub tick: u64,
    pub commands: Vec<ReplayCommand>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ReplayCommand {
    pub bot_id: String,
    pub throttle: f32,
    pub rudder: f32,
    pub sensor_mode: SensorMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fire: Option<FireCommand>,
    /// Powerup activation that drove this tick. `None` for ticks that did not activate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activate_powerup: Option<PowerupId>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ReplayEnd {
    pub tick: u64,
    pub winner: Option<String>,
}

// ---------------------------------------------------------------------------
// Replay viewer: directory listing + offline re-run capture
// ---------------------------------------------------------------------------

/// One entry in the replay directory listing (`GET /api/replays`). Built from a log's
/// header and (when present) its `end` record without re-running the simulation.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ReplaySummary {
    pub replay_id: String,
    pub room: String,
    pub seed: u64,
    pub tick_hz: u32,
    pub map: MapInfo,
    pub sim_config: SimConfig,
    /// Bot display names, in registration order.
    pub bots: Vec<String>,
    /// Final tick, if the log carries an `end` record (a match that ran to completion).
    pub final_tick: Option<u64>,
    /// Winning bot's display name. `None` for a draw, an aborted match, or a log with no
    /// `end` record.
    pub winner_name: Option<String>,
}

/// A full offline re-run of a replay: the header plus the ground-truth spectator frame at
/// every tick. `frames[t]` is the world at tick `t`, so the slider in the viewer indexes
/// straight into this vec.
#[derive(Serialize, Debug)]
pub struct CapturedReplay {
    pub header: ReplayHeader,
    pub frames: Vec<SpectatorMsg>,
    pub end: Option<ReplayEnd>,
}

/// One bot's sensor-filtered view at a single tick.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PerspectiveFrame {
    pub tick: u64,
    pub contacts: Vec<Contact>,
    pub events: Vec<TickEvent>,
}

/// A replay re-run captured from one bot's sensors
/// (`GET /api/replays/{id}/perspective/{bot_id}`). `frames[t]` is the bot's filtered view
/// at tick `t`; ticks with no `tick` message (tick 0 and the deciding tick) are empty.
#[derive(Serialize, Debug)]
pub struct CapturedPerspective {
    pub bot_id: String,
    pub frames: Vec<PerspectiveFrame>,
}

/// Sink for replay records. Either appends to a file on disk or to a shared in-memory
/// buffer (used by tests). Construction is fallible for the file variant; everything else
/// is infallible at the type level — write errors are reported per-call.
pub struct ReplayWriter {
    sink: Box<dyn Write + Send>,
    replay_id: String,
    /// Path on disk if this writer is file-backed. `None` for in-memory writers.
    path: Option<PathBuf>,
}

impl std::fmt::Debug for ReplayWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReplayWriter")
            .field("replay_id", &self.replay_id)
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl ReplayWriter {
    /// Open `<dir>/<replay_id>.jsonl` for writing, creating the directory tree if needed.
    pub fn create_file(dir: &Path, replay_id: String) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(format!("{replay_id}.jsonl"));
        let file = File::create(&path)?;
        Ok(Self {
            sink: Box::new(BufWriter::new(file)),
            replay_id,
            path: Some(path),
        })
    }

    /// Build a writer whose output is captured in a shared `Vec<u8>`. Returns the writer
    /// plus a clone of the buffer the caller can read from after writing finishes.
    pub fn in_memory(replay_id: String) -> (Self, Arc<Mutex<Vec<u8>>>) {
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let sink = SharedBuf(Arc::clone(&buf));
        (
            Self {
                sink: Box::new(sink),
                replay_id,
                path: None,
            },
            buf,
        )
    }

    pub fn replay_id(&self) -> &str {
        &self.replay_id
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Write a single record followed by a `\n` and flush. Flushing per-line keeps the
    /// log usable if the server crashes mid-match — at 10 Hz the cost is negligible.
    pub fn write(&mut self, record: &ReplayRecord) -> io::Result<()> {
        // Serialization is infallible for our types; treat any error as a bug.
        let json = serde_json::to_string(record).expect("ReplayRecord always serializes");
        self.sink.write_all(json.as_bytes())?;
        self.sink.write_all(b"\n")?;
        self.sink.flush()
    }
}

/// `Write` adapter over a shared `Arc<Mutex<Vec<u8>>>` so tests can grab the bytes back
/// without owning the writer.
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut guard = self
            .0
            .lock()
            .map_err(|_| io::Error::other("replay buffer poisoned"))?;
        guard.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Read a JSONL replay log into a list of records. Bails on the first malformed line —
/// partial replays are not supported. Empty lines are silently skipped to tolerate trailing
/// newlines and cosmetic padding.
pub fn read_records(path: &Path) -> io::Result<Vec<ReplayRecord>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    for (lineno, line) in reader.lines().enumerate() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let rec: ReplayRecord = serde_json::from_str(trimmed).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("malformed replay line {}: {e}", lineno + 1),
            )
        })?;
        out.push(rec);
    }
    Ok(out)
}

/// Same as `read_records` but reads from any `BufRead`. Useful for tests that keep the
/// log in memory.
pub fn read_records_from<R: BufRead>(reader: R) -> io::Result<Vec<ReplayRecord>> {
    let mut out = Vec::new();
    for (lineno, line) in reader.lines().enumerate() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let rec: ReplayRecord = serde_json::from_str(trimmed).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("malformed replay line {}: {e}", lineno + 1),
            )
        })?;
        out.push(rec);
    }
    Ok(out)
}

/// Generate a replay identifier of the form `match_<room>_<unix_secs>`. The unix timestamp
/// is a wall-clock read, so this MUST NOT be called inside the simulation — it's strictly
/// for naming the file we're about to write.
pub fn make_replay_id(room: &str) -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("match_{room}_{secs}")
}

/// A bot's id paired with the receiver for the frames the room sends it.
type BotOutbound = (String, mpsc::Receiver<ServerMsg>);

/// Registration sequence number behind a `b_<n>` bot id. The room assigns ids
/// monotonically as bots register, so this is the canonical ordering for replay bots — and
/// the order the header's `bots` array is expected to be in. Ids that don't match the
/// pattern sort last; that's defensive only — the room always emits `b_<n>`.
pub(crate) fn bot_id_seq(bot_id: &str) -> u64 {
    bot_id
        .strip_prefix("b_")
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(u64::MAX)
}

/// Reconstruct a `Room` from a `ReplayHeader`, returning each bot's outbound receiver
/// alongside it. The bots are pre-registered with the same ids the live run assigned, then
/// marked ready and started — so the resulting Room is in `Running` state at tick 0, ready
/// to accept injected commands.
///
/// The receivers carry the `welcome` / `game_start` / `tick` frames the room sends to
/// bots. Perspective capture drains them to recover each bot's sensor view; `run_replay`
/// and the determinism test discard them via [`rebuild_room_from_header`].
fn rebuild_room_with_outbound(
    header: &ReplayHeader,
) -> Result<(Room, Vec<BotOutbound>), ReplayError> {
    let mut room = Room::new(
        header.room.clone(),
        header.map.width as f32,
        header.map.height as f32,
        header.seed,
        header.tick_hz,
        header.tick_deadline_ms,
        header.max_bots.max(header.bots.len() as u32),
    );
    // Apply the recorded balance parameters before any bot registers or the match starts,
    // so `welcome` payloads and physics use the exact values the live run did.
    room.world.config = header.sim_config;

    // The header's `bots` array may be in any order: logs written before the
    // registration-order fix serialized it lexicographically by `BotId`, which interleaves
    // `b_10` ahead of `b_2`. The room assigns ids sequentially as bots register, so
    // register in numeric-id order — the only order in which the ids the room assigns line
    // up with the ones the header recorded.
    let mut ordered: Vec<&ReplayBot> = header.bots.iter().collect();
    ordered.sort_by_key(|b| bot_id_seq(&b.bot_id));

    let mut outbound = Vec::with_capacity(header.bots.len());
    for bot in ordered {
        let (reply_tx, mut reply_rx) = oneshot::channel();
        room.handle_event(RoomEvent::BotConnect {
            peer: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            name: bot.name.clone(),
            version: "replay".into(),
            reply: reply_tx,
        });
        // `handle_event` is synchronous and fills the reply before returning, so try_recv
        // always succeeds here.
        let reg = reply_rx
            .try_recv()
            .map_err(|_| ReplayError::Header("room dropped registration reply".into()))?
            .map_err(|e| ReplayError::Header(format!("bot register failed: {}", e.as_str())))?;
        if reg.bot_id != bot.bot_id {
            return Err(ReplayError::Header(format!(
                "bot id drift on rebuild: header expected `{}`, room assigned `{}`",
                bot.bot_id, reg.bot_id
            )));
        }
        if reg.ship_id != bot.ship_id {
            return Err(ReplayError::Header(format!(
                "ship id drift on rebuild: header expected `{}`, room assigned `{}`",
                bot.ship_id, reg.ship_id
            )));
        }
        outbound.push((reg.bot_id.clone(), reg.outbound));
        // Re-apply the recorded loadout, if any, before `BotReady`. This way the room's
        // `start_match` mirrors the same selections onto the ship as the live run did.
        if !bot.selected_powerups.is_empty() {
            room.handle_event(RoomEvent::BotSelectPowerups {
                bot_id: reg.bot_id.clone(),
                powerups: bot.selected_powerups.clone(),
            });
        }
        room.handle_event(RoomEvent::BotReady { bot_id: reg.bot_id });
    }

    let (start_tx, mut start_rx) = oneshot::channel();
    room.handle_event(RoomEvent::OperatorStart {
        room: header.room.clone(),
        reply: start_tx,
    });
    start_rx
        .try_recv()
        .map_err(|_| ReplayError::Header("room dropped start reply".into()))?
        .map_err(|e| ReplayError::Header(format!("start refused: {}", e.as_str())))?;

    Ok((room, outbound))
}

/// Reconstruct a `Room` from a `ReplayHeader`. Used by the live replay player
/// (`run_replay`), `capture_replay`, and the determinism test.
///
/// The per-bot outbound receivers are dropped: those callers never read the bot frames.
/// The senders' `try_send`s then either buffer or fail silently, both fine in replay mode.
pub fn rebuild_room_from_header(header: &ReplayHeader) -> Result<Room, ReplayError> {
    rebuild_room_with_outbound(header).map(|(room, _outbound)| room)
}

/// Errors that can stop a replay run before it finishes.
#[derive(Debug)]
pub enum ReplayError {
    Io(io::Error),
    /// The header was missing, malformed, or referenced bots the rebuilt room couldn't
    /// reproduce.
    Header(String),
    Version(u32),
    /// A perspective capture named a bot id absent from the replay header.
    UnknownBot(String),
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayError::Io(e) => write!(f, "io error: {e}"),
            ReplayError::Header(msg) => write!(f, "replay header error: {msg}"),
            ReplayError::Version(v) => write!(
                f,
                "unsupported replay format version {v}; expected {}",
                REPLAY_FORMAT_VERSION
            ),
            ReplayError::UnknownBot(id) => {
                write!(f, "replay has no bot with id `{id}`")
            }
        }
    }
}

impl std::error::Error for ReplayError {}

impl From<io::Error> for ReplayError {
    fn from(e: io::Error) -> Self {
        ReplayError::Io(e)
    }
}

/// Drive a replay end-to-end: read the file, rebuild the room, then tick at the recorded
/// `tick_hz` injecting commands and broadcasting spectator frames. Returns when the log
/// is exhausted, the shutdown channel fires, or an error occurs.
pub async fn run_replay(
    path: PathBuf,
    spec_tx: broadcast::Sender<SpectatorFrame>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<(), ReplayError> {
    let records = tokio::task::spawn_blocking(move || read_records(&path))
        .await
        .map_err(|e| ReplayError::Io(io::Error::other(format!("blocking read panicked: {e}"))))??;

    let mut iter = records.into_iter();
    let header = match iter.next() {
        Some(ReplayRecord::Header(h)) => *h,
        Some(_) => {
            return Err(ReplayError::Header(
                "replay log does not begin with a header record".into(),
            ))
        }
        None => return Err(ReplayError::Header("replay log is empty".into())),
    };
    if header.version != REPLAY_FORMAT_VERSION {
        return Err(ReplayError::Version(header.version));
    }

    let tick_hz = header.tick_hz.max(1);
    let mut room = tokio::task::spawn_blocking(move || rebuild_room_from_header(&header))
        .await
        .map_err(|e| ReplayError::Header(format!("rebuild room blocking task panicked: {e}")))??;
    room.set_spectator_broadcast(spec_tx);

    let period = Duration::from_secs_f64(1.0 / f64::from(tick_hz));
    let mut ticker = interval(period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    info!(tick_hz, "replay running");

    loop {
        tokio::select! {
            biased;
            _ = shutdown_rx.recv() => {
                info!("replay: shutdown signal received");
                break;
            }
            _ = ticker.tick() => {
                let next = iter.next();
                match next {
                    Some(ReplayRecord::Tick(rec)) => {
                        for cmd in rec.commands {
                            let pending = PendingCommand {
                                tick: rec.tick,
                                throttle: cmd.throttle,
                                rudder: cmd.rudder,
                                fire: cmd.fire,
                                sensor_mode: cmd.sensor_mode,
                                activate_powerup: cmd.activate_powerup,
                            };
                            room.inject_replay_command(&cmd.bot_id, pending);
                        }
                        // Step until we reach the recorded tick. Empty-command ticks were
                        // not written by the writer (it skips records with no commands),
                        // so we may need to advance multiple ticks per record.
                        while room.world.tick < rec.tick {
                            room.step_tick();
                        }
                    }
                    Some(ReplayRecord::End(end)) => {
                        // Run remaining ticks (if any) so the spectator sees the final
                        // state at the same world.tick the live run reached.
                        while room.world.tick < end.tick {
                            room.step_tick();
                        }
                        info!(final_tick = end.tick, winner = ?end.winner, "replay finished");
                        break;
                    }
                    Some(ReplayRecord::Header(_)) => {
                        warn!("replay: stray header record mid-stream, ignored");
                    }
                    None => {
                        info!("replay: log exhausted with no end record");
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Re-run a replay end to end, capturing the ground-truth spectator frame at every tick.
/// `records` must begin with a `Header`. Backs `GET /api/replays/{id}`.
///
/// Determinism: this drives the exact `inject_replay_command` + `step_tick` path the
/// determinism test covers, so the captured frames are bit-faithful to the live run.
pub fn capture_replay(records: Vec<ReplayRecord>) -> Result<CapturedReplay, ReplayError> {
    let mut iter = records.into_iter();
    let header = match iter.next() {
        Some(ReplayRecord::Header(h)) => *h,
        Some(_) => {
            return Err(ReplayError::Header(
                "replay log does not begin with a header record".into(),
            ))
        }
        None => return Err(ReplayError::Header("replay log is empty".into())),
    };
    if header.version != REPLAY_FORMAT_VERSION {
        return Err(ReplayError::Version(header.version));
    }

    let mut room = rebuild_room_from_header(&header)?;
    // A single local receiver keeps `broadcast_spectator_world` from skipping serialization
    // (it no-ops when nobody is subscribed). One frame is produced per `step_tick`; we
    // drain it immediately so the buffer never backs up.
    let (spec_tx, mut spec_rx) = broadcast::channel::<SpectatorFrame>(16);
    room.set_spectator_broadcast(spec_tx);

    let mut frames: Vec<SpectatorMsg> = Vec::new();
    // Tick 0: the starting layout, before any command is applied.
    frames.push(room.spectator_world_snapshot());

    let mut end: Option<ReplayEnd> = None;
    for record in iter {
        match record {
            ReplayRecord::Tick(rec) => {
                for cmd in rec.commands {
                    room.inject_replay_command(
                        &cmd.bot_id,
                        PendingCommand {
                            tick: rec.tick,
                            throttle: cmd.throttle,
                            rudder: cmd.rudder,
                            fire: cmd.fire,
                            sensor_mode: cmd.sensor_mode,
                            activate_powerup: cmd.activate_powerup,
                        },
                    );
                }
                while room.world.tick < rec.tick {
                    room.step_tick();
                    drain_spectator_frames(&mut spec_rx, &mut frames);
                }
            }
            ReplayRecord::End(rec) => {
                while room.world.tick < rec.tick {
                    room.step_tick();
                    drain_spectator_frames(&mut spec_rx, &mut frames);
                }
                end = Some(rec);
                break;
            }
            ReplayRecord::Header(_) => {
                warn!("replay capture: stray header record mid-stream, ignored");
            }
        }
    }

    Ok(CapturedReplay {
        header,
        frames,
        end,
    })
}

/// Drain every pending broadcast frame into `frames`, parsing each JSON string back into a
/// typed `SpectatorMsg`.
fn drain_spectator_frames(
    rx: &mut broadcast::Receiver<SpectatorFrame>,
    frames: &mut Vec<SpectatorMsg>,
) {
    while let Ok(frame) = rx.try_recv() {
        match serde_json::from_str::<SpectatorMsg>(frame.as_str()) {
            Ok(msg) => frames.push(msg),
            Err(e) => warn!(error = %e, "replay capture: failed to parse spectator frame"),
        }
    }
}

/// Re-run a replay capturing one bot's sensor-filtered view at every tick. `bot_id` must
/// name a bot present in the header. Backs `GET /api/replays/{id}/perspective/{bot_id}`.
pub fn capture_perspective(
    records: Vec<ReplayRecord>,
    bot_id: &str,
) -> Result<CapturedPerspective, ReplayError> {
    let mut iter = records.into_iter();
    let header = match iter.next() {
        Some(ReplayRecord::Header(h)) => *h,
        Some(_) => {
            return Err(ReplayError::Header(
                "replay log does not begin with a header record".into(),
            ))
        }
        None => return Err(ReplayError::Header("replay log is empty".into())),
    };
    if header.version != REPLAY_FORMAT_VERSION {
        return Err(ReplayError::Version(header.version));
    }
    if !header.bots.iter().any(|b| b.bot_id == bot_id) {
        return Err(ReplayError::UnknownBot(bot_id.to_string()));
    }

    let (mut room, mut outbound) = rebuild_room_with_outbound(&header)?;

    // Per-tick filtered views keyed by tick. The registration `welcome` / `game_start`
    // frames already queued on the channels are drained and discarded here.
    let mut views: BTreeMap<u64, PerspectiveFrame> = BTreeMap::new();
    drain_perspective(&mut outbound, bot_id, &mut views);

    for record in iter {
        match record {
            ReplayRecord::Tick(rec) => {
                for cmd in rec.commands {
                    room.inject_replay_command(
                        &cmd.bot_id,
                        PendingCommand {
                            tick: rec.tick,
                            throttle: cmd.throttle,
                            rudder: cmd.rudder,
                            fire: cmd.fire,
                            sensor_mode: cmd.sensor_mode,
                            activate_powerup: cmd.activate_powerup,
                        },
                    );
                }
                while room.world.tick < rec.tick {
                    room.step_tick();
                    drain_perspective(&mut outbound, bot_id, &mut views);
                }
            }
            ReplayRecord::End(rec) => {
                while room.world.tick < rec.tick {
                    room.step_tick();
                    drain_perspective(&mut outbound, bot_id, &mut views);
                }
                break;
            }
            ReplayRecord::Header(_) => {
                warn!("perspective capture: stray header record mid-stream, ignored");
            }
        }
    }

    // Densify: one frame per tick 0..=final_tick. Ticks where the bot received no `tick`
    // message (tick 0, and the deciding tick) get empty contacts/events.
    let final_tick = room.world.tick;
    let frames: Vec<PerspectiveFrame> = (0..=final_tick)
        .map(|t| {
            views.remove(&t).unwrap_or(PerspectiveFrame {
                tick: t,
                contacts: Vec::new(),
                events: Vec::new(),
            })
        })
        .collect();

    Ok(CapturedPerspective {
        bot_id: bot_id.to_string(),
        frames,
    })
}

/// Drain each bot's outbound channel, recording the target bot's `tick` frames into
/// `views`. Other bots' frames are discarded — draining still keeps their buffers from
/// filling and dropping frames mid-run.
fn drain_perspective(
    outbound: &mut [BotOutbound],
    bot_id: &str,
    views: &mut BTreeMap<u64, PerspectiveFrame>,
) {
    for (id, rx) in outbound.iter_mut() {
        while let Ok(msg) = rx.try_recv() {
            if id != bot_id {
                continue;
            }
            if let ServerMsg::Tick {
                tick,
                contacts,
                events,
                ..
            } = msg
            {
                views.insert(
                    tick,
                    PerspectiveFrame {
                        tick,
                        contacts,
                        events,
                    },
                );
            }
        }
    }
}

/// List every readable replay in `dir`, newest first. A missing directory yields an empty
/// list; individual unreadable files are skipped with a warning rather than failing the
/// whole listing. Backs `GET /api/replays`.
pub fn list_replays(dir: &Path) -> io::Result<Vec<ReplaySummary>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        match read_replay_summary(&path) {
            Ok(summary) => out.push(summary),
            Err(e) => warn!(path = %path.display(), error = %e, "skipping unreadable replay"),
        }
    }
    // Replay ids embed a unix timestamp (`match_<room>_<secs>`), so a descending sort by
    // id puts the most recent match first.
    out.sort_by(|a, b| b.replay_id.cmp(&a.replay_id));
    Ok(out)
}

/// Build a [`ReplaySummary`] from a log's header and last record without re-running the
/// simulation — only the first and last lines are parsed.
fn read_replay_summary(path: &Path) -> io::Result<ReplaySummary> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut header: Option<ReplayHeader> = None;
    let mut last_line: Option<String> = None;
    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if header.is_none() {
            match serde_json::from_str::<ReplayRecord>(trimmed) {
                Ok(ReplayRecord::Header(h)) => header = Some(*h),
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "replay log does not begin with a header record",
                    ))
                }
                Err(e) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("malformed replay header: {e}"),
                    ))
                }
            }
        } else {
            last_line = Some(trimmed.to_string());
        }
    }
    let header =
        header.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "replay log is empty"))?;

    let end: Option<ReplayEnd> =
        last_line.and_then(|l| match serde_json::from_str::<ReplayRecord>(&l) {
            Ok(ReplayRecord::End(e)) => Some(e),
            _ => None,
        });
    let winner_name = end
        .as_ref()
        .and_then(|e| e.winner.as_ref())
        .and_then(|wid| {
            header
                .bots
                .iter()
                .find(|b| &b.bot_id == wid)
                .map(|b| b.name.clone())
        });

    Ok(ReplaySummary {
        replay_id: header.replay_id.clone(),
        room: header.room.clone(),
        seed: header.seed,
        tick_hz: header.tick_hz,
        map: header.map,
        sim_config: header.sim_config,
        bots: {
            // Present names in registration order even for logs written before the writer
            // emitted `bots` in that order (see `bot_id_seq`).
            let mut bots = header.bots.clone();
            bots.sort_by_key(|b| bot_id_seq(&b.bot_id));
            bots.into_iter().map(|b| b.name).collect()
        },
        final_tick: end.as_ref().map(|e| e.tick),
        winner_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sample_header() -> ReplayHeader {
        ReplayHeader {
            version: REPLAY_FORMAT_VERSION,
            replay_id: "match_test_42".into(),
            room: "test".into(),
            seed: 42,
            tick_hz: 10,
            tick_deadline_ms: 80,
            map: MapInfo {
                width: 1000,
                height: 1000,
            },
            max_bots: 4,
            sim_config: SimConfig::default(),
            bots: vec![
                ReplayBot {
                    bot_id: "b_1".into(),
                    ship_id: "s_1".into(),
                    name: "alice".into(),
                    selected_powerups: vec![PowerupId::Overdrive, PowerupId::RapidFire],
                },
                ReplayBot {
                    bot_id: "b_2".into(),
                    ship_id: "s_2".into(),
                    name: "bob".into(),
                    selected_powerups: vec![],
                },
            ],
        }
    }

    fn sample_tick() -> ReplayTick {
        ReplayTick {
            tick: 7,
            commands: vec![
                ReplayCommand {
                    bot_id: "b_1".into(),
                    throttle: 1.0,
                    rudder: -0.25,
                    sensor_mode: SensorMode::Active,
                    fire: None,
                    activate_powerup: Some(PowerupId::Overdrive),
                },
                ReplayCommand {
                    bot_id: "b_2".into(),
                    throttle: -0.5,
                    rudder: 0.0,
                    sensor_mode: SensorMode::Passive,
                    fire: Some(FireCommand {
                        bearing_deg: 90.0,
                        range: 200.0,
                    }),
                    activate_powerup: None,
                },
            ],
        }
    }

    #[test]
    fn record_roundtrips() {
        for rec in [
            ReplayRecord::Header(Box::new(sample_header())),
            ReplayRecord::Tick(sample_tick()),
            ReplayRecord::End(ReplayEnd {
                tick: 1843,
                winner: Some("b_1".into()),
            }),
            ReplayRecord::End(ReplayEnd {
                tick: 3000,
                winner: None,
            }),
        ] {
            let json = serde_json::to_string(&rec).expect("serialize");
            let parsed: ReplayRecord = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(rec, parsed, "roundtrip mismatch: {json}");
        }
    }

    #[test]
    fn writer_emits_jsonl() {
        let (mut writer, buf) = ReplayWriter::in_memory("match_test_0".into());
        writer
            .write(&ReplayRecord::Header(Box::new(sample_header())))
            .expect("write header");
        writer
            .write(&ReplayRecord::Tick(sample_tick()))
            .expect("write tick");
        writer
            .write(&ReplayRecord::End(ReplayEnd {
                tick: 9,
                winner: None,
            }))
            .expect("write end");

        let bytes = buf.lock().unwrap().clone();
        let text = String::from_utf8(bytes).expect("utf-8");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        // Each line must be a valid `ReplayRecord` with the discriminator field intact.
        for line in &lines {
            assert!(line.contains("\"type\":"), "missing type tag: {line}");
            let _: ReplayRecord = serde_json::from_str(line).expect("each line parses");
        }
    }

    #[test]
    fn read_records_from_skips_blank_lines() {
        let text = format!(
            "{}\n\n{}\n",
            serde_json::to_string(&ReplayRecord::Header(Box::new(sample_header()))).unwrap(),
            serde_json::to_string(&ReplayRecord::End(ReplayEnd {
                tick: 1,
                winner: None
            }))
            .unwrap()
        );
        let records = read_records_from(Cursor::new(text)).expect("read");
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn read_records_rejects_garbage() {
        let text = "{\"type\":\"header\"}\nnot-json\n";
        let err = read_records_from(Cursor::new(text)).expect_err("should error");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn make_replay_id_starts_with_match_room() {
        let id = make_replay_id("main");
        assert!(id.starts_with("match_main_"), "got {id}");
    }

    #[test]
    fn capture_replay_yields_one_frame_per_tick() {
        let records = vec![
            ReplayRecord::Header(Box::new(sample_header())),
            ReplayRecord::Tick(sample_tick()),
            ReplayRecord::End(ReplayEnd {
                tick: 10,
                winner: None,
            }),
        ];
        let captured = capture_replay(records).expect("capture");

        // Tick 0 (starting layout) through tick 10 inclusive → 11 frames.
        assert_eq!(captured.frames.len(), 11);
        for (i, frame) in captured.frames.iter().enumerate() {
            let SpectatorMsg::World { tick, .. } = frame;
            assert_eq!(*tick, i as u64, "frame {i} carries the wrong tick");
        }
        assert_eq!(captured.end.expect("end record").tick, 10);
    }

    #[test]
    fn capture_perspective_densifies_to_one_frame_per_tick() {
        let records = vec![
            ReplayRecord::Header(Box::new(sample_header())),
            ReplayRecord::Tick(sample_tick()),
            ReplayRecord::End(ReplayEnd {
                tick: 10,
                winner: None,
            }),
        ];
        let captured = capture_perspective(records, "b_1").expect("capture");

        assert_eq!(captured.bot_id, "b_1");
        assert_eq!(captured.frames.len(), 11);
        for (i, frame) in captured.frames.iter().enumerate() {
            assert_eq!(frame.tick, i as u64, "frame {i} carries the wrong tick");
        }
    }

    #[test]
    fn capture_perspective_rejects_unknown_bot() {
        let records = vec![
            ReplayRecord::Header(Box::new(sample_header())),
            ReplayRecord::End(ReplayEnd {
                tick: 1,
                winner: None,
            }),
        ];
        let err = capture_perspective(records, "b_404").expect_err("unknown bot");
        assert!(matches!(err, ReplayError::UnknownBot(_)), "got {err:?}");
    }

    #[test]
    fn list_replays_missing_dir_is_empty() {
        let listed = list_replays(Path::new("/nonexistent-replay-dir-xyz")).expect("list");
        assert!(listed.is_empty());
    }

    #[test]
    fn bot_id_seq_orders_numerically() {
        // Lexicographic string order would put `b_10` before `b_2`; `bot_id_seq` must not.
        assert!(bot_id_seq("b_2") < bot_id_seq("b_10"));
        assert!(bot_id_seq("b_10") < bot_id_seq("b_11"));
        // Malformed ids sort last rather than panicking.
        assert_eq!(bot_id_seq("garbage"), u64::MAX);
        assert_eq!(bot_id_seq("b_"), u64::MAX);
    }

    #[test]
    fn rebuild_tolerates_lexicographic_header() {
        // A header whose `bots` array is in lexicographic `BotId` order — exactly what
        // logs written before the registration-order fix contain. `b_10`/`b_11`
        // interleave ahead of `b_2`, so a naive in-order rebuild assigns mismatched ids
        // and fails with "bot id drift on rebuild".
        let mut header = sample_header();
        header.max_bots = 11;
        let mut bots: Vec<ReplayBot> = (1..=11)
            .map(|n| ReplayBot {
                bot_id: format!("b_{n}"),
                ship_id: format!("s_{n}"),
                name: format!("bot-{n}"),
                selected_powerups: vec![],
            })
            .collect();
        bots.sort_by(|a, b| a.bot_id.cmp(&b.bot_id));
        // Sanity-check the array really is in the pathological order before rebuilding.
        assert_eq!(
            bots[1].bot_id, "b_10",
            "test setup: expected lexicographic order"
        );
        header.bots = bots;

        let room = rebuild_room_from_header(&header).expect("rebuild tolerates header order");
        assert_eq!(room.bot_count(), 11);
    }
}
