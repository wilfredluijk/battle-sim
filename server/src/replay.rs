//! Replay log: a JSONL record of every match for re-running the simulation later.
//!
//! Format (one JSON object per line):
//!   1. Header (line 0): version, seed, tick rate, map size, bots in registration order.
//!      Each bot also carries its `selected_powerups` and (v4+) its actual `spawn_pos` +
//!      `spawn_heading_deg`, so a replay reproduces the true starting layout even when the
//!      live run used a non-Fixed variance layout (e.g. Monte Carlo `Shuffled`).
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
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
///
/// v4 added per-bot `spawn_pos` + `spawn_heading_deg` to the header so a replay reproduces
/// the *actual* starting positions, regardless of the variance layout the live run used
/// (Monte Carlo runs use non-Fixed layouts; rebuilding via the default ring diverged).
/// Both fields are `serde(default)`, so v2/v3 logs (which lack them) still deserialize;
/// the reader treats `version <= REPLAY_FORMAT_VERSION` as loadable and falls back to the
/// rebuilt ring layout when the recorded spawns are absent/all-zero.
///
/// v5 added the `Disconnect` record: a mid-match disconnect or operator kick removes the
/// bot's ship from the world immediately, which shifts the shared RNG stream (fewer ships =
/// fewer sensor draws) and can end the match early. Without it, replay kept a ghost ship and
/// diverged from the recorded `End`. v4 and older logs simply carry no `Disconnect` records
/// (a match where nobody dropped), so they still load and replay identically.
pub const REPLAY_FORMAT_VERSION: u32 = 5;

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
    /// A bot left mid-match (transport disconnect or operator kick) while the room was
    /// `Running`. Written the instant the ship is removed. `tick` is the value of
    /// `world.tick` at that instant — i.e. the last tick the ship participated in. Replay
    /// steps the world to `tick`, then removes the ship, so every tick after it is computed
    /// (and draws RNG) without that ship, matching the live run. See `advance_to`.
    Disconnect(ReplayDisconnect),
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
    /// Actual spawn position `[x, y]` of this bot's ship at tick 0, as placed by whatever
    /// variance layout the live run used. Recorded (v4+) so the replay reproduces faithful
    /// starting positions even for non-Fixed (e.g. Monte Carlo `Shuffled`) layouts. Absent
    /// in v2/v3 logs, which deserialize this to `[0.0, 0.0]`; the rebuild then keeps the
    /// default ring layout instead of overwriting (see `rebuild_room_with_outbound`).
    #[serde(default)]
    pub spawn_pos: [f32; 2],
    /// Actual spawn heading (compass degrees) at tick 0. Defaults to `0.0` for v2/v3 logs.
    #[serde(default)]
    pub spawn_heading_deg: f32,
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
pub struct ReplayDisconnect {
    pub tick: u64,
    pub bot_id: String,
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
    ///
    /// Opens with `create_new` so an existing file is **never truncated** — if
    /// `<replay_id>.jsonl` already exists (a same-second id collision that slipped past the
    /// monotonic counter, or a stale file on disk), it retries with `_2`, `_3`, … suffixes
    /// and adopts the disambiguated id so downstream references (e.g. `game_over.replay_id`)
    /// point at the file that was actually written.
    pub fn create_file(dir: &Path, replay_id: String) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        // A handful of retries is plenty: `make_replay_id`'s per-process counter already
        // guarantees uniqueness for ids minted this run; this only guards against files left
        // on disk by a previous process.
        const MAX_SUFFIX: u32 = 1000;
        for attempt in 1..=MAX_SUFFIX {
            let candidate_id = if attempt == 1 {
                replay_id.clone()
            } else {
                format!("{replay_id}_{attempt}")
            };
            let path = dir.join(format!("{candidate_id}.jsonl"));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        sink: Box::new(BufWriter::new(file)),
                        replay_id: candidate_id,
                        path: Some(path),
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("could not find a free replay filename for `{replay_id}` after {MAX_SUFFIX} attempts"),
        ))
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

/// Per-process monotonic counter appended to every non-MC replay id so two matches started
/// within the same wall-clock second still get distinct ids (and therefore distinct files).
static REPLAY_ID_SEQ: AtomicU64 = AtomicU64::new(0);

/// Generate a replay identifier of the form `match_<room>_<unix_secs>_<seq>`. The unix
/// timestamp is a wall-clock read, so this MUST NOT be called inside the simulation — it's
/// strictly for naming the file we're about to write. `<seq>` is a per-process monotonic
/// counter: 1-second timestamp resolution alone collides when a match is started, aborted,
/// reset, and restarted inside one second (trivial via the REST API), and `create_file`
/// would then truncate the earlier match's log. The counter makes ids unique regardless.
pub fn make_replay_id(room: &str) -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let seq = REPLAY_ID_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("match_{room}_{secs}_{seq}")
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

    // Reuse the recorded ids verbatim. The room's live registration mints ids from a
    // monotonic `next_index` that lobby churn advances, so a recorded match's bots are not
    // guaranteed to start at `b_1` or be contiguous (e.g. `b_12`, `b_14`). Re-deriving ids
    // on rebuild drifted from the header ("header expected `b_12`, room assigned `b_1`");
    // `register_replay_bot` forces the recorded pair instead. Registering in numeric-id
    // order keeps `next_index`, logs, and the `outbound` vec tidy — correctness no longer
    // depends on it, since the ids are no longer derived from registration order.
    let mut ordered: Vec<&ReplayBot> = header.bots.iter().collect();
    ordered.sort_by_key(|b| bot_id_seq(&b.bot_id));

    let mut outbound = Vec::with_capacity(header.bots.len());
    for bot in ordered {
        let reg = room
            .register_replay_bot(
                bot.bot_id.clone(),
                bot.ship_id.clone(),
                SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
                bot.name.clone(),
                "replay",
            )
            .map_err(|e| {
                ReplayError::Header(format!("replay bot register failed: {}", e.as_str()))
            })?;
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

    // `OperatorStart` placed every ship on the default ring (`default_ring_layout`). For
    // logs that recorded explicit spawn state (v4+), overwrite each ship's position and
    // heading with the recorded values so the replay starts from the *actual* layout the
    // live run used — Monte Carlo runs use non-Fixed layouts that the ring can't reproduce.
    //
    // Guard against old logs: v2/v3 lack the spawn fields, so they deserialize to
    // `[0.0, 0.0]` / `0.0`. Only overwrite when the header is v4+ AND at least one bot
    // carries a non-zero spawn (a real spawn is never exactly the origin on any map),
    // leaving pre-v4 replays on the rebuilt ring as before.
    let has_recorded_spawns = header.version >= 4
        && header
            .bots
            .iter()
            .any(|b| b.spawn_pos != [0.0, 0.0] || b.spawn_heading_deg != 0.0);
    if has_recorded_spawns {
        for bot in &header.bots {
            if let Some(ship) = room.world.ships.get_mut(&bot.ship_id) {
                ship.pos = glam::Vec2::new(bot.spawn_pos[0], bot.spawn_pos[1]);
                ship.heading_deg = bot.spawn_heading_deg;
            }
        }
    }

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

/// Advance `room` to exactly `target_tick`, arranging for `commands` to be consumed by the
/// step that *produces* `target_tick` and no earlier.
///
/// F-01: `step_tick` unconditionally drains each bot's `pending_command` at the top of every
/// step — the `PendingCommand.tick` field does not gate consumption. The writer omits ticks
/// with no commands (see `write_replay_tick`), so consecutive records can straddle a tick
/// gap (e.g. a record for tick 100 followed by one for tick 105 when every bot fell silent
/// in between). Injecting first and then stepping the whole gap would consume `target_tick`'s
/// commands on the *first* of those steps — up to N-1 ticks early, diverging live from
/// replay for any match where a bot skipped a tick. So we step up to `target_tick - 1` with
/// nothing queued, then inject, then step exactly once.
///
/// `after_step` runs after each `step_tick`; spectator/perspective capture drain their frame
/// buffers there, and the live player passes a no-op. All three replay drivers funnel their
/// tick records through this helper so their timing semantics can't drift apart again.
fn advance_and_inject(
    room: &mut Room,
    target_tick: u64,
    commands: &[ReplayCommand],
    mut after_step: impl FnMut(&mut Room),
) {
    // Step every empty tick that precedes the one the commands belong to. The `+ 1` form
    // avoids the underflow a bare `target_tick - 1` would hit on a corrupt `tick: 0` record.
    while room.world.tick + 1 < target_tick {
        room.step_tick();
        after_step(room);
    }
    for cmd in commands {
        room.inject_replay_command(
            &cmd.bot_id,
            PendingCommand {
                tick: target_tick,
                throttle: cmd.throttle,
                rudder: cmd.rudder,
                fire: cmd.fire,
                sensor_mode: cmd.sensor_mode,
                activate_powerup: cmd.activate_powerup,
            },
        );
    }
    // Produce `target_tick`, consuming the freshly injected commands. Guarded so a stray
    // record whose tick we've already reached (duplicate or corrupt) never steps backwards
    // or re-consumes an already-applied command.
    if room.world.tick < target_tick {
        room.step_tick();
        after_step(room);
    }
}

/// Step `room` forward until `world.tick == target_tick`, running `after_step` after each
/// step. Shared by the `End` and `Disconnect` record handlers (which advance the world but
/// inject no commands) across all three replay drivers, so their timing stays in lockstep.
fn advance_to(room: &mut Room, target_tick: u64, mut after_step: impl FnMut(&mut Room)) {
    while room.world.tick < target_tick {
        room.step_tick();
        after_step(room);
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
    // Accept any version up to and including the current one. Old logs (v2/v3) lack the
    // v4 spawn fields, which deserialize to defaults and trigger the ring-layout fallback
    // on rebuild. Newer-than-current logs are still rejected — we can't know their shape.
    if header.version > REPLAY_FORMAT_VERSION {
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
                        advance_and_inject(&mut room, rec.tick, &rec.commands, |_| {});
                    }
                    Some(ReplayRecord::Disconnect(rec)) => {
                        advance_to(&mut room, rec.tick, |_| {});
                        room.remove_bot_and_ship(&rec.bot_id);
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
    // Accept any version up to and including the current one. Old logs (v2/v3) lack the
    // v4 spawn fields, which deserialize to defaults and trigger the ring-layout fallback
    // on rebuild. Newer-than-current logs are still rejected — we can't know their shape.
    if header.version > REPLAY_FORMAT_VERSION {
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
                advance_and_inject(&mut room, rec.tick, &rec.commands, |_| {
                    drain_spectator_frames(&mut spec_rx, &mut frames);
                });
            }
            ReplayRecord::Disconnect(rec) => {
                advance_to(&mut room, rec.tick, |_| {
                    drain_spectator_frames(&mut spec_rx, &mut frames);
                });
                room.remove_bot_and_ship(&rec.bot_id);
            }
            ReplayRecord::End(rec) => {
                advance_to(&mut room, rec.tick, |_| {
                    drain_spectator_frames(&mut spec_rx, &mut frames);
                });
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
    // Accept any version up to and including the current one. Old logs (v2/v3) lack the
    // v4 spawn fields, which deserialize to defaults and trigger the ring-layout fallback
    // on rebuild. Newer-than-current logs are still rejected — we can't know their shape.
    if header.version > REPLAY_FORMAT_VERSION {
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
                advance_and_inject(&mut room, rec.tick, &rec.commands, |_| {
                    drain_perspective(&mut outbound, bot_id, &mut views);
                });
            }
            ReplayRecord::Disconnect(rec) => {
                advance_to(&mut room, rec.tick, |_| {
                    drain_perspective(&mut outbound, bot_id, &mut views);
                });
                room.remove_bot_and_ship(&rec.bot_id);
            }
            ReplayRecord::End(rec) => {
                advance_to(&mut room, rec.tick, |_| {
                    drain_perspective(&mut outbound, bot_id, &mut views);
                });
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
                    spawn_pos: [300.0, 500.0],
                    spawn_heading_deg: 90.0,
                },
                ReplayBot {
                    bot_id: "b_2".into(),
                    ship_id: "s_2".into(),
                    name: "bob".into(),
                    selected_powerups: vec![],
                    spawn_pos: [700.0, 500.0],
                    spawn_heading_deg: 270.0,
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
    fn make_replay_id_is_unique_within_the_same_second() {
        // F-10: two ids minted back-to-back (same wall-clock second) must differ, so the
        // second match's log can't truncate the first's.
        let a = make_replay_id("main");
        let b = make_replay_id("main");
        assert_ne!(a, b, "consecutive replay ids collided: {a}");
    }

    #[test]
    fn create_file_never_truncates_an_existing_log() {
        // F-10: opening a writer for an id whose file already exists must not clobber it —
        // it lands on a suffixed filename instead, and the original bytes survive.
        let dir = std::env::temp_dir().join(format!(
            "battle_sim_replay_test_{}",
            make_replay_id("collide")
        ));
        let id = "match_collide_fixed".to_string();

        let mut first = ReplayWriter::create_file(&dir, id.clone()).expect("first writer");
        first
            .write(&ReplayRecord::End(ReplayEnd {
                tick: 7,
                winner: Some("b_1".into()),
            }))
            .expect("write first");
        let first_path = first.path().expect("file-backed").to_path_buf();
        drop(first);

        // A second writer for the *same* id must not reuse the same file.
        let second = ReplayWriter::create_file(&dir, id.clone()).expect("second writer");
        assert_ne!(
            second.replay_id(),
            id,
            "second writer reused the colliding id"
        );
        assert_ne!(
            second.path().expect("file-backed"),
            first_path,
            "second writer opened the same file (would truncate)"
        );

        // The first log is intact.
        let first_bytes = fs::read_to_string(&first_path).expect("read first log");
        assert!(
            first_bytes.contains("\"tick\":7"),
            "first replay log was truncated: {first_bytes:?}"
        );

        let _ = fs::remove_dir_all(&dir);
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
                spawn_pos: [0.0, 0.0],
                spawn_heading_deg: 0.0,
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

    #[test]
    fn rebuild_reuses_recorded_ids_with_offset_and_gaps() {
        // Lobby churn in the live run advances the room's monotonic `next_index` and leaves
        // gaps, so a recorded match's bots need not start at `b_1` or be contiguous. Rebuild
        // must reuse the recorded ids verbatim rather than re-mint from `b_1` (which drifted:
        // "header expected `b_12`, room assigned `b_1`").
        let mut header = sample_header();
        header.max_bots = 16;
        header.bots = vec![
            ReplayBot {
                bot_id: "b_12".into(),
                ship_id: "s_12".into(),
                name: "alice".into(),
                selected_powerups: vec![],
                spawn_pos: [300.0, 500.0],
                spawn_heading_deg: 90.0,
            },
            ReplayBot {
                bot_id: "b_14".into(),
                ship_id: "s_14".into(),
                name: "bob".into(),
                selected_powerups: vec![],
                spawn_pos: [700.0, 500.0],
                spawn_heading_deg: 270.0,
            },
        ];

        let room = rebuild_room_from_header(&header).expect("rebuild reuses recorded ids");
        assert_eq!(room.bot_count(), 2);
        // Ships carry the recorded ids and their owning bot ids — not freshly minted ones.
        let s12 = room
            .world
            .ships
            .get("s_12")
            .expect("recorded ship s_12 present");
        assert_eq!(s12.bot_id, "b_12");
        let s14 = room
            .world
            .ships
            .get("s_14")
            .expect("recorded ship s_14 present");
        assert_eq!(s14.bot_id, "b_14");
        assert!(
            !room.world.ships.contains_key("s_1"),
            "rebuild must not re-mint ids from b_1"
        );
    }

    #[test]
    fn spawn_state_round_trips_through_serialization() {
        // The v4 header's per-bot spawn fields must survive a JSONL write/read cycle.
        let header = sample_header();
        let line =
            serde_json::to_string(&ReplayRecord::Header(Box::new(header.clone()))).expect("ser");
        let records = read_records_from(Cursor::new(line)).expect("read");
        let ReplayRecord::Header(parsed) = &records[0] else {
            panic!("expected header record");
        };
        assert_eq!(parsed.bots[0].spawn_pos, [300.0, 500.0]);
        assert_eq!(parsed.bots[0].spawn_heading_deg, 90.0);
        assert_eq!(parsed.bots[1].spawn_pos, [700.0, 500.0]);
        assert_eq!(parsed.bots[1].spawn_heading_deg, 270.0);
    }

    #[test]
    fn rebuild_applies_recorded_non_ring_spawns() {
        // A v4 header records explicit spawn positions that are *not* on the default ring.
        // Rebuild must place ships at exactly those positions — this is what makes a
        // non-Fixed (Monte Carlo `Shuffled`) layout replay faithfully instead of diverging
        // back onto the ring. `sample_header` records s_1 at (300,500)/90° and s_2 at
        // (700,500)/270°; the default ring for 2 bots on a 1000×1000 map would instead place
        // them at (900,500) and (100,500).
        let header = sample_header();
        let room = rebuild_room_from_header(&header).expect("rebuild");

        let s1 = room.world.ships.get("s_1").expect("ship s_1");
        let s2 = room.world.ships.get("s_2").expect("ship s_2");
        assert_eq!([s1.pos.x, s1.pos.y], [300.0, 500.0]);
        assert_eq!(s1.heading_deg, 90.0);
        assert_eq!([s2.pos.x, s2.pos.y], [700.0, 500.0]);
        assert_eq!(s2.heading_deg, 270.0);
        // Genuinely off the ring (would be x≈900 or x≈100 under the old ring rebuild).
        assert!(
            (s1.pos.x - 900.0).abs() > 1.0 && (s1.pos.x - 100.0).abs() > 1.0,
            "spawn fell back onto the default ring: {:?}",
            s1.pos
        );
    }

    #[test]
    fn old_log_without_spawns_keeps_ring_layout() {
        // Pre-v4 logs lack spawn fields, which deserialize to [0,0]/0. Rebuild must NOT
        // overwrite the rebuilt ring with the origin; it keeps the default ring layout.
        let mut header = sample_header();
        header.version = 3;
        for b in &mut header.bots {
            b.spawn_pos = [0.0, 0.0];
            b.spawn_heading_deg = 0.0;
        }
        let room = rebuild_room_from_header(&header).expect("rebuild old log");
        let s1 = room.world.ships.get("s_1").expect("ship s_1");
        // Default ring: 2 bots at (900,500) / (100,500) on a 1000×1000 map.
        let on_ring = ((s1.pos.x - 900.0).abs() < 1e-3 || (s1.pos.x - 100.0).abs() < 1e-3)
            && (s1.pos.y - 500.0).abs() < 1e-3;
        assert!(on_ring, "old log should keep ring layout, got {:?}", s1.pos);
    }
}
