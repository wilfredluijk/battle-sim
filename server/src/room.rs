//! Room: a single match. Owns the world, the RNG, and the tick loop.
//!
//! The room is the bridge between the protocol (`net.rs`) and the deterministic simulation
//! (`sim/`). It receives `RoomEvent`s over an mpsc channel, mutates the world, and replies
//! to bots via per-connection mpsc senders. Bot lifecycle (Phase 4.1) lives here; per-tick
//! command exchange lands in Phase 4.3.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::Vec2;
use rand::SeedableRng;
use rand_pcg::Pcg64;
use serde::Serialize;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{interval, MissedTickBehavior};
use tracing::{debug, info, warn};

use crate::admin::{AdminBotInfo, AdminServerMsg, AdminState};
use crate::monte_carlo::{self, McConfig, McState, McStatus};
use crate::protocol::{
    self, error_code, Contact as ProtocolContact, ContactKind as ProtocolContactKind, FireCommand,
    MapInfo, PowerupStatus, SelfState, SensorMode, ServerMsg, ShipSpecs, SpectatorDecoy,
    SpectatorEvent, SpectatorMsg, SpectatorShell, SpectatorShip, SpectatorSmokeCloud, TickEvent,
};
use crate::replay::{
    self, ReplayBot, ReplayCommand, ReplayEnd, ReplayHeader, ReplayRecord, ReplayTick,
    ReplayWriter, REPLAY_FORMAT_VERSION,
};
use crate::sim::combat::{self, CombatEvent, FireError};
use crate::sim::powerups::{self, ActivationError, PowerupId};
use crate::sim::sensors::{self, Contact as SimContact, ContactKind as SimContactKind};
use crate::sim::{physics, BotId, Ship, ShipId, SimConfig, World};

/// A pre-serialized spectator frame, broadcast once per tick to every `/spectate`
/// connection. Wrapped in `Arc` so subscribers share the underlying allocation rather
/// than copying the JSON.
pub type SpectatorFrame = Arc<String>;

/// Channel buffer for outbound messages to a single bot. Sized for a few ticks of slack —
/// the bot consumes one message per tick under normal operation.
const BOT_OUTBOUND_BUFFER: usize = 32;

/// Channel buffer for inbound `RoomEvent`s. One event per bot action; tens of bots tops.
pub const ROOM_EVENT_BUFFER: usize = 256;

/// Radius of the §5.6 starting circle. Bots are placed evenly around the map center,
/// all facing inward.
const STARTING_RING_RADIUS: f32 = 400.0;

/// Hard match timeout per §5.5. After this many ticks the room ends regardless of how
/// many ships are alive; the highest-HP survivor (tie-break: highest remaining ammo) wins.
const MATCH_TIMEOUT_TICKS: u64 = 3000;

/// Ticks the room stays in `Ended` before auto-returning to `Lobby` after a match. At the
/// default `tick_hz = 10` this is ~2 seconds — long enough for the spectator UI to show
/// the final frame and the winner banner, short enough that the operator doesn't have to
/// click "Reset" between every match. Bots see this gap as silence between the `game_over`
/// frame and the next `lobby` frame.
pub const POST_GAME_LOBBY_TICKS: u64 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomState {
    Lobby,
    Running,
    Ended,
}

/// Per-bot state tracked by the room.
#[derive(Debug)]
#[allow(dead_code)] // `name`/`peer` are read by Phase 7 (spectator) / Phase 11 (kick).
struct BotEntry {
    bot_id: BotId,
    ship_id: ShipId,
    name: String,
    peer: SocketAddr,
    outbound: mpsc::Sender<ServerMsg>,
    ready: bool,
    /// Latest queued command from this bot. Drained at the top of each tick and applied
    /// in `BotId` order. `None` means the previous tick's controls persist (per §4.1.3).
    pending_command: Option<PendingCommand>,
    /// Last commanded sensor mode. Persists across ticks until the bot changes it.
    sensor_mode: SensorMode,
    /// Tick of the most recent fire-cooldown error sent to this bot. Used to suppress
    /// duplicate cooldown/no-ammo error frames inside the same tick — a bot spamming
    /// `fire` would otherwise flood its own outbound buffer.
    last_fire_error_tick: Option<u64>,
    /// World ticks at which we accepted a command from this bot, used to surface a
    /// rolling commands-per-second figure on the spectator feed. Entries older than
    /// `tick_hz` ticks are trimmed at push time. Observability only — not part of any
    /// simulation state.
    command_ticks: VecDeque<u64>,
    /// Powerups picked by this bot, in pick order (length 0..=2). Set by
    /// `select_powerups` while in `Lobby`. Mirrored onto the bot's ship at `start_match`
    /// so the simulation can read it without going through the room.
    selected_powerups: Vec<PowerupId>,
}

/// A command waiting to be applied at the next tick. Lifted from `BotMsg::Command` —
/// keeping a separate type lets the room own its data without dragging the protocol
/// enum into long-lived state.
#[derive(Debug, Clone, Copy)]
pub struct PendingCommand {
    pub tick: u64,
    pub throttle: f32,
    pub rudder: f32,
    pub sensor_mode: SensorMode,
    pub fire: Option<FireCommand>,
    pub activate_powerup: Option<PowerupId>,
}

/// What the room hands back to a connection task after a successful `BotConnect`.
#[derive(Debug)]
pub struct BotRegistration {
    pub bot_id: BotId,
    pub ship_id: ShipId,
    /// Receiver for messages the room wants delivered to this bot.
    pub outbound: mpsc::Receiver<ServerMsg>,
}

/// Reasons the room can refuse a `BotConnect`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinError {
    NotInLobby,
    RoomFull,
    DuplicateName,
    InvalidName,
}

impl JoinError {
    pub fn as_str(&self) -> &'static str {
        match self {
            JoinError::NotInLobby => "room is not accepting bots (already running or ended)",
            JoinError::RoomFull => "room is full",
            JoinError::DuplicateName => "another bot is already registered with that name",
            JoinError::InvalidName => "bot name is invalid",
        }
    }
}

/// Reasons the operator's `room start` request can be refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    UnknownRoom,
    NotInLobby,
    NoBots,
    NotAllReady,
}

impl StartError {
    pub fn as_str(&self) -> &'static str {
        match self {
            StartError::UnknownRoom => "no room with that name",
            StartError::NotInLobby => "room is not in lobby state",
            StartError::NoBots => "no bots connected",
            StartError::NotAllReady => "not all bots are ready",
        }
    }
}

/// Reasons an `OperatorAbort` request can be refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbortError {
    NotRunning,
}

impl AbortError {
    pub fn as_str(&self) -> &'static str {
        match self {
            AbortError::NotRunning => "room is not running",
        }
    }
}

/// Reasons an `OperatorReset` request can be refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResetError {
    NotEnded,
}

impl ResetError {
    pub fn as_str(&self) -> &'static str {
        match self {
            ResetError::NotEnded => "room is not in ended state",
        }
    }
}

/// Reasons an `OperatorKick` request can be refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KickError {
    UnknownBot,
}

impl KickError {
    pub fn as_str(&self) -> &'static str {
        match self {
            KickError::UnknownBot => "no bot with that id",
        }
    }
}

/// Reasons a `StartMonteCarlo` request can be refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McStartError {
    /// MC requires a Lobby state to begin (same prerequisite as a single match start).
    NotInLobby,
    /// At least two ready bots are required for a meaningful Monte Carlo run.
    InsufficientBots,
    /// Another run is already active; stop it first.
    AlreadyRunning,
    Invalid(String),
}

impl McStartError {
    pub fn as_str(&self) -> &str {
        match self {
            McStartError::NotInLobby => "monte carlo runs can only be started from the lobby",
            McStartError::InsufficientBots => {
                "at least two ready bots are required for a monte carlo run"
            }
            McStartError::AlreadyRunning => "a monte carlo run is already in progress",
            McStartError::Invalid(msg) => msg,
        }
    }
}

/// Reasons a `StopMonteCarlo` request can be refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McStopError {
    NotRunning,
}

impl McStopError {
    pub fn as_str(&self) -> &'static str {
        match self {
            McStopError::NotRunning => "no monte carlo run is in progress",
        }
    }
}

/// Reasons an `OperatorConfigure` request can be refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigureError {
    /// Parameters can only be changed while the room is in `Lobby`.
    NotInLobby,
    /// A parameter failed validation; the string is a human-readable reason.
    Invalid(String),
}

impl ConfigureError {
    pub fn as_str(&self) -> &str {
        match self {
            ConfigureError::NotInLobby => "parameters can only be changed before the match starts",
            ConfigureError::Invalid(reason) => reason,
        }
    }
}

/// Events the room consumes from connection tasks and the operator. The room is
/// single-threaded with respect to its own state; this channel serializes all mutations.
#[derive(Debug)]
pub enum RoomEvent {
    BotConnect {
        peer: SocketAddr,
        name: String,
        version: String,
        reply: oneshot::Sender<Result<BotRegistration, JoinError>>,
    },
    BotReady {
        bot_id: BotId,
    },
    /// Bot declared its powerup loadout for the match. Only honoured in `Lobby`. The
    /// room validates the list (exactly 2, distinct, all known) and on success replaces
    /// the bot's previous selection; on failure it sends a typed `error` frame to the
    /// bot and leaves the previous selection unchanged.
    BotSelectPowerups {
        bot_id: BotId,
        powerups: Vec<PowerupId>,
    },
    BotCommand {
        bot_id: BotId,
        command: PendingCommand,
    },
    BotDisconnect {
        bot_id: BotId,
    },
    /// Operator-issued `room start <name>`. Replies with `Ok(())` if the room
    /// transitioned to `Running`, otherwise the reason it could not.
    OperatorStart {
        room: String,
        reply: oneshot::Sender<Result<(), StartError>>,
    },
    /// Operator-issued abort. Forces an ongoing match to end with no winner. The room
    /// then auto-returns to `Lobby` after the usual post-game pause.
    OperatorAbort {
        reply: oneshot::Sender<Result<(), AbortError>>,
    },
    /// Operator-issued reset. Only valid in `Ended`; cuts the post-game pause short and
    /// returns the room to `Lobby` immediately.
    OperatorReset {
        reply: oneshot::Sender<Result<(), ResetError>>,
    },
    /// Operator-issued kick. Removes a bot from the room. The bot's connection task
    /// observes its outbound channel close and exits cleanly.
    OperatorKick {
        bot_id: BotId,
        reply: oneshot::Sender<Result<(), KickError>>,
    },
    /// Admin client subscribed to room-state pushes. The room replies with a fresh
    /// `broadcast::Receiver` and immediately publishes the current snapshot so the new
    /// receiver's first frame is the state.
    AdminSubscribe {
        reply: oneshot::Sender<broadcast::Receiver<AdminServerMsg>>,
    },
    /// Admin REST request for a one-shot snapshot of the current room state. Used by
    /// `GET /api/room` — no subscription, just the current `AdminState`.
    QueryState {
        reply: oneshot::Sender<RoomSnapshot>,
    },
    /// Admin REST request for the most recent [`MatchReport`]. Used by
    /// `GET /api/room/report`. `None` until the first match has finished.
    QueryReport {
        reply: oneshot::Sender<Option<MatchReport>>,
    },
    /// Operator-issued parameter change from `PUT /api/room/config`. Only valid in
    /// `Lobby`; replaces the match's `SimConfig` after validation.
    OperatorConfigure {
        config: SimConfig,
        reply: oneshot::Sender<Result<(), ConfigureError>>,
    },
    /// Start a Monte Carlo batch run. Validates the config, snapshots the run state, and
    /// kicks off the first match immediately. Subsequent matches are chained from inside
    /// `step_tick`.
    StartMonteCarlo {
        config: McConfig,
        reply: oneshot::Sender<Result<String, McStartError>>,
    },
    /// Stop the active Monte Carlo run. The currently in-flight match finishes naturally
    /// (or is aborted, depending on `force_abort`); no further matches are queued.
    StopMonteCarlo {
        force_abort: bool,
        reply: oneshot::Sender<Result<(), McStopError>>,
    },
    /// Snapshot the active or most-recent MC run for `GET /api/montecarlo/status`.
    QueryMonteCarloStatus {
        reply: oneshot::Sender<McStatus>,
    },
}

/// One-shot snapshot returned by `RoomEvent::QueryState`: the admin-facing room state plus
/// the current balance parameters so the pre-match UI can populate its form.
#[derive(Debug, Clone)]
pub struct RoomSnapshot {
    pub state: AdminState,
    pub config: SimConfig,
}

/// Per-bot statistics accumulated over a single match. Reset at `start_match`, frozen into
/// a [`MatchReport`] when the match ends. Counters are bumped inside `step_tick`, so they
/// are deterministic — a replay rebuilds identical figures.
#[derive(Debug, Clone, Default)]
struct BotStats {
    shots_fired: u32,
    hits_landed: u32,
    damage_dealt: u32,
    damage_taken: u32,
    kills: u32,
}

/// Post-match summary surfaced to the spectator UI via `GET /api/room/report`. Built once
/// when a match ends (naturally or by abort) and kept until the next match starts.
#[derive(Debug, Clone, Serialize)]
pub struct MatchReport {
    pub room: String,
    /// Identifier of the replay log for this match, if one was written.
    pub replay_id: Option<String>,
    /// `"winner"`, `"draw"`, or `"aborted"`.
    pub outcome: String,
    pub winner: Option<BotId>,
    pub winner_name: Option<String>,
    pub duration_ticks: u64,
    pub duration_seconds: f32,
    pub bots: Vec<BotReport>,
}

/// One bot's row in a [`MatchReport`].
#[derive(Debug, Clone, Serialize)]
pub struct BotReport {
    pub bot_id: BotId,
    pub name: String,
    pub shots_fired: u32,
    pub hits_landed: u32,
    /// `hits_landed / shots_fired`, in `[0, 1]`; `0.0` when the bot never fired.
    pub accuracy: f32,
    pub damage_dealt: u32,
    pub damage_taken: u32,
    pub kills: u32,
    pub final_hp: u32,
    pub survived: bool,
}

#[derive(Debug)]
pub struct Room {
    pub name: String,
    pub world: World,
    pub state: RoomState,
    pub rng: Pcg64,
    /// Original seed used to construct `rng`. Stashed so the replay header can record it
    /// (the Pcg64 itself doesn't expose its seed).
    pub seed: u64,
    pub tick_hz: u32,
    pub tick_deadline_ms: u64,
    pub max_bots: u32,
    bots: BTreeMap<BotId, BotEntry>,
    next_index: u32,
    /// Wall-clock time at which the most recent `tick` frame was broadcast. Commands
    /// arriving more than `tick_deadline_ms` after this are rejected as `late_command`.
    /// `None` until the room sends its first `tick` frame after entering `Running`.
    tick_send_time: Option<Instant>,
    /// Set of `ShipId`s that were in `Active` sensor mode during the previous tick. The
    /// current tick's passive listeners use this snapshot — sensor mode changes
    /// propagate with one tick of delay (§5.3 "who pinged last tick" logic).
    previous_active_pingers: BTreeSet<ShipId>,
    /// Number of bots present at `game_start`. The "≤1 alive ⇒ match over" rule from §5.5
    /// only applies when at least two ships started; a 1-bot sandbox/test room would
    /// otherwise terminate on its first tick.
    starting_bot_count: u32,
    /// Optional broadcast sender for spectator `world` frames. `None` in unit tests; the
    /// runtime in `main.rs` always wires a real channel. Send failures (no subscribers)
    /// are ignored — the simulation never blocks on the spectator UI.
    spectator_tx: Option<broadcast::Sender<SpectatorFrame>>,
    /// Directory where replay JSONL files should be written. When set, `start_match`
    /// opens a fresh writer and emits the header line; subsequent ticks append. `None`
    /// in unit tests and in `--replay` mode.
    replay_dir: Option<PathBuf>,
    /// Active replay log writer. Open between `start_match` and `broadcast_game_over`.
    /// In `--replay` mode this stays `None` — replay playback never writes a new log.
    replay_writer: Option<ReplayWriter>,
    /// Identifier for the current match. Generated at `start_match` time and reused both
    /// for the replay filename and the `replay_id` field of `game_over` so a player who
    /// just lost can find their log without grepping.
    replay_id: Option<String>,
    /// `world.tick` at which the most recent match ended. Drives the deterministic
    /// `Ended → Lobby` countdown in `step_tick`. `None` while the room has never run, or
    /// after a successful transition back to `Lobby`.
    end_tick: Option<u64>,
    /// Winner of the most recent match, or `None` for a draw / aborted match. Surfaced to
    /// admin clients via `AdminState.last_winner` so the UI can show the result during the
    /// post-game pause and on Lobby afterwards.
    last_winner: Option<BotId>,
    /// Optional broadcast sender for admin state pushes. `None` in unit tests; the
    /// runtime in `main.rs` wires a real channel. Receivers are added via
    /// `RoomEvent::AdminSubscribe`.
    admin_tx: Option<broadcast::Sender<AdminServerMsg>>,
    /// Per-bot statistics for the in-progress match, keyed by `BotId`. Reset to a fresh
    /// entry per bot at `start_match`; folded into `last_report` when the match ends.
    match_stats: BTreeMap<BotId, BotStats>,
    /// Report from the most recently finished match. Survives the `Ended → Lobby`
    /// transition so the post-battle screen can show it; replaced at the next
    /// `start_match`. `None` until the first match has finished.
    last_report: Option<MatchReport>,
    /// In-flight Monte Carlo run. `Some` while a batch is running; `None` otherwise.
    /// The fields drive lockstep pacing, spectator throttling, and the
    /// auto-chain-to-next-match path inside `step_tick`.
    mc_run: Option<McState>,
    /// Status snapshot of the most recently finished Monte Carlo run, surfaced via
    /// `GET /api/montecarlo/status` after the run ends.
    mc_last_status: Option<McStatus>,
}

impl Room {
    pub fn new(
        name: String,
        width: f32,
        height: f32,
        seed: u64,
        tick_hz: u32,
        tick_deadline_ms: u64,
        max_bots: u32,
    ) -> Self {
        Self {
            name,
            world: World::new(width, height, crate::sim::SimConfig::default()),
            state: RoomState::Lobby,
            rng: Pcg64::seed_from_u64(seed),
            seed,
            tick_hz,
            tick_deadline_ms,
            max_bots,
            bots: BTreeMap::new(),
            next_index: 1,
            tick_send_time: None,
            previous_active_pingers: BTreeSet::new(),
            starting_bot_count: 0,
            spectator_tx: None,
            replay_dir: None,
            replay_writer: None,
            replay_id: None,
            end_tick: None,
            last_winner: None,
            admin_tx: None,
            match_stats: BTreeMap::new(),
            last_report: None,
            mc_run: None,
            mc_last_status: None,
        }
    }

    /// Wire an admin broadcast channel. `AdminSubscribe` events return clones of this
    /// sender's receiver; lifecycle transitions publish through it. Call once at
    /// construction time.
    pub fn set_admin_broadcast(&mut self, tx: broadcast::Sender<AdminServerMsg>) {
        self.admin_tx = Some(tx);
    }

    /// Wire a spectator broadcast channel. Subsequent `step_tick` calls will publish a
    /// `world` frame to every subscriber. Call this once at construction time.
    pub fn set_spectator_broadcast(&mut self, tx: broadcast::Sender<SpectatorFrame>) {
        self.spectator_tx = Some(tx);
    }

    /// Configure the directory where replay logs are written. Call before `start_match` —
    /// the writer is opened on the lobby→running transition.
    pub fn set_replay_dir(&mut self, dir: PathBuf) {
        self.replay_dir = Some(dir);
    }

    /// Inject a pre-built `ReplayWriter` instead of opening one from `replay_dir`. Used
    /// by tests to capture replay bytes in memory.
    pub fn set_replay_writer(&mut self, writer: ReplayWriter) {
        self.replay_id = Some(writer.replay_id().to_string());
        self.replay_writer = Some(writer);
    }

    /// Take ownership of the active writer. Used by tests after a match to read back the
    /// log; under normal operation the writer is dropped on `broadcast_game_over`.
    pub fn take_replay_writer(&mut self) -> Option<ReplayWriter> {
        self.replay_writer.take()
    }

    /// Bypass the late-command path and queue `command` for `bot_id` as if it had arrived
    /// in time. Replay playback uses this to inject recorded commands without tripping the
    /// deadline check; live mode goes through `RoomEvent::BotCommand` instead.
    pub fn inject_replay_command(&mut self, bot_id: &BotId, command: PendingCommand) {
        let world_tick = self.world.tick;
        let window = u64::from(self.tick_hz);
        if let Some(entry) = self.bots.get_mut(bot_id) {
            entry.pending_command = Some(command);
            record_command_tick(&mut entry.command_ticks, world_tick, window);
        } else {
            warn!(room = %self.name, bot = %bot_id, "replay command for unknown bot, dropped");
        }
    }

    /// Number of bots currently registered (regardless of `ready` state).
    pub fn bot_count(&self) -> usize {
        self.bots.len()
    }

    /// True when at least one bot is registered and every registered bot is `ready`.
    pub fn all_ready(&self) -> bool {
        !self.bots.is_empty() && self.bots.values().all(|b| b.ready)
    }

    /// Advance the simulation by one fixed timestep and bump the tick counter.
    /// Only steps physics in `Running` state; in `Lobby` / `Ended` the world is frozen.
    ///
    /// Order, per the determinism contract in `CLAUDE.md`:
    /// 1. Apply queued commands (throttle/rudder/sensor_mode + fire) in `BotId` order.
    /// 2. Step physics (movement + cooldown decrement).
    /// 3. Step shells (flight + splash damage + death flips).
    /// 4. Bump tick counter.
    /// 5. Check the §5.5 end conditions; if the match is over, broadcast `game_over`
    ///    and return — no `tick` frame this tick.
    /// 6. Compute per-bot sensor contacts and build/send `tick` frames including
    ///    sensor-filtered combat events.
    /// 7. Snapshot the now-current `Active` pingers for use by next tick's passives.
    pub fn step_tick(&mut self) {
        // Post-game pause: after a match ends the room stays in `Ended` for
        // `POST_GAME_LOBBY_TICKS` so the spectator UI can show the final frame and bots
        // can react to `game_over`. Once the gap elapses, transition back to `Lobby` and
        // notify bots so they can rearm for the next match. Monte Carlo mode skips the
        // pause entirely and immediately starts the next match below.
        if self.state == RoomState::Ended && self.mc_run.is_none() {
            if let Some(end_tick) = self.end_tick {
                if self.world.tick.saturating_sub(end_tick) >= POST_GAME_LOBBY_TICKS {
                    self.transition_to_lobby();
                    self.publish_admin_state();
                }
            }
        }

        if self.state != RoomState::Running {
            self.world.tick = self.world.tick.saturating_add(1);
            // Spectators still see the lobby/ended state — full ground truth, no events.
            self.broadcast_spectator_world(&[], &[]);
            return;
        }

        let bot_ids: Vec<BotId> = self.bots.keys().cloned().collect();

        // Snapshot of commands actually applied this tick, in BotId order. Written to the
        // replay log after the tick counter is bumped so the on-disk tick number matches
        // the post-step world state.
        let mut applied_commands: Vec<ReplayCommand> = Vec::new();
        // Powerup activations that succeeded this tick. Surfaced to bots / spectators
        // via `TickEvent::PowerupActivated` / `SpectatorEvent::PowerupActivated`.
        let mut powerup_activations: Vec<(ShipId, PowerupId)> = Vec::new();

        // 1. Drain pending commands and apply them in BotId order. Fire processed after
        //    throttle/rudder so a successful shot is reflected in this tick's cooldown.
        for bot_id in &bot_ids {
            let cmd = match self.bots.get_mut(bot_id) {
                Some(entry) => entry.pending_command.take(),
                None => continue,
            };
            let Some(cmd) = cmd else { continue };

            let ship_id = {
                let entry = self.bots.get_mut(bot_id).expect("present");
                entry.sensor_mode = cmd.sensor_mode;
                entry.ship_id.clone()
            };
            if let Some(ship) = self.world.ships.get_mut(&ship_id) {
                ship.throttle = cmd.throttle.clamp(-1.0, 1.0);
                ship.rudder = cmd.rudder.clamp(-1.0, 1.0);
            }
            if let Some(fire_cmd) = cmd.fire {
                match combat::fire(
                    &mut self.world,
                    &ship_id,
                    fire_cmd.bearing_deg,
                    fire_cmd.range,
                ) {
                    Ok(()) => {
                        if let Some(stats) = self.match_stats.get_mut(bot_id) {
                            stats.shots_fired += 1;
                        }
                    }
                    Err(err) => self.send_fire_error(bot_id, err),
                }
            }
            if let Some(powerup) = cmd.activate_powerup {
                match powerups::activate(&mut self.world, &ship_id, powerup, &mut self.rng) {
                    Ok(()) => {
                        powerup_activations.push((ship_id.clone(), powerup));
                        info!(
                            room = %self.name,
                            bot = %bot_id,
                            powerup = powerup.as_str(),
                            tick = self.world.tick,
                            "powerup activated",
                        );
                    }
                    Err(err) => self.send_activation_error(bot_id, powerup, err),
                }
            }
            // Record the raw command (un-clamped) so a replay re-applies the exact same
            // input the live run saw. Clamping happens deterministically inside step_tick,
            // so the post-clamp ship state will match.
            if self.replay_writer.is_some() {
                applied_commands.push(ReplayCommand {
                    bot_id: bot_id.clone(),
                    throttle: cmd.throttle,
                    rudder: cmd.rudder,
                    sensor_mode: cmd.sensor_mode,
                    fire: cmd.fire,
                    activate_powerup: cmd.activate_powerup,
                });
            }
        }

        // 2 + 3. Movement, then shell flight & splashes.
        physics::step_world(&mut self.world);
        let combat_events = combat::step_shells(&mut self.world);
        // 3.5: powerup maintenance — repair regen, smoke/decoy GC. Runs *before* the
        // tick-counter bump so effects that expire at tick `t` stop applying once
        // `world.tick == t`. (Maintenance reads `world.tick`, so doing it here is correct
        // — the bump comes next.)
        powerups::step_tick_maintenance(&mut self.world);

        // 4. Bump the tick counter so the outbound frames carry the new tick number.
        self.world.tick = self.world.tick.saturating_add(1);

        // Persist the commands that drove this tick. Writing here (post-bump) means the
        // recorded `tick` field equals the world tick the commands produced, which is the
        // tick the bots received next time around.
        self.write_replay_tick(applied_commands);

        // Spectator broadcast: full ground truth + every combat event + powerup
        // activations. Done before the end-of-match check so the deciding tick (with its
        // death events) is visible.
        self.broadcast_spectator_world(&combat_events, &powerup_activations);

        // Fold this tick's combat into the per-bot match statistics. Runs before the
        // end-of-match check so the deciding tick's hits and kills are counted.
        self.accumulate_combat_stats(&combat_events);

        // 5. End-of-match check. Broadcasting `game_over` and returning early means dead
        //    and surviving bots all hear about the outcome via the same message; no final
        //    `tick` frame is sent for the deciding tick.
        if let Some(winner) = self.match_outcome() {
            self.state = RoomState::Ended;
            self.end_tick = Some(self.world.tick);
            self.last_winner = winner.clone();
            self.last_report = Some(self.build_match_report(winner.clone(), false));
            let duration_ticks = self.world.tick;
            let replay_id_for_match = self.replay_id.clone();
            self.broadcast_game_over(winner.clone());
            // Monte Carlo: record this match's outcome and immediately chain to the next
            // match (or finalize the run if this was the last one). Skips the regular
            // POST_GAME_LOBBY_TICKS pause — the batch is the whole point of this mode.
            if self.mc_run.is_some() {
                self.mc_record_match_end(winner, duration_ticks, replay_id_for_match);
                self.mc_advance_after_match();
            }
            self.publish_admin_state();
            return;
        }

        // 6. Per-bot sensor view + filtered combat events.
        for bot_id in &bot_ids {
            // Look up the bot + ship without holding any borrow on self past the call
            // site — we'll need `&mut self.rng` and `&self.world` together.
            let (ship_id, sensor_mode, viewer_pos) = {
                let Some(entry) = self.bots.get(bot_id) else {
                    continue;
                };
                let Some(ship) = self.world.ships.get(&entry.ship_id) else {
                    continue;
                };
                (entry.ship_id.clone(), entry.sensor_mode, ship.pos)
            };

            let sim_contacts = match sensor_mode {
                SensorMode::Active => {
                    sensors::active_contacts(&ship_id, viewer_pos, &self.world, &mut self.rng)
                }
                SensorMode::Passive => sensors::passive_contacts(
                    &ship_id,
                    viewer_pos,
                    &self.world,
                    &self.previous_active_pingers,
                    &mut self.rng,
                ),
            };
            // If this bot has a pending counter-battery trace, splice a synthetic precise
            // contact in *before* the natural ones so it's near the front of the list.
            // Done in-place rather than in `sensors::active_contacts` so the trace works
            // regardless of the bot's current sensor mode.
            let mut contacts: Vec<ProtocolContact> = sim_contacts
                .into_iter()
                .enumerate()
                .map(|(i, c)| translate_contact(i, c))
                .collect();
            self.consume_counter_battery_reveal(&ship_id, viewer_pos, &mut contacts);

            let mut events = filter_events_for_bot(
                &ship_id,
                viewer_pos,
                sensor_mode,
                &self.world.config,
                &combat_events,
            );
            // Activation events: always show your own; for others, only if the activating
            // ship would currently be a contact for the viewer.
            for (acting_ship_id, powerup) in &powerup_activations {
                let visible = acting_ship_id == &ship_id
                    || self.is_ship_visible_to(&ship_id, viewer_pos, sensor_mode, acting_ship_id);
                if visible {
                    events.push(TickEvent::PowerupActivated {
                        ship_id: acting_ship_id.clone(),
                        powerup: *powerup,
                    });
                }
            }

            let entry = self.bots.get(bot_id).expect("bot still present");
            let ship = self.world.ships.get(&ship_id).expect("ship still present");
            let world_tick = self.world.tick;
            let powerup_status: Vec<PowerupStatus> = ship
                .powerups
                .selected
                .iter()
                .map(|id| PowerupStatus {
                    id: *id,
                    used: ship.powerups.used.contains(id),
                    active_ticks_left: ship.powerups.ticks_remaining(*id, world_tick),
                })
                .collect();
            let tick_msg = ServerMsg::Tick {
                tick: world_tick,
                deadline_ms: self.tick_deadline_ms,
                self_state: SelfState {
                    pos: [ship.pos.x, ship.pos.y],
                    heading_deg: ship.heading_deg,
                    speed: ship.speed,
                    hp: ship.hp,
                    ammo: ship.ammo,
                    rudder: ship.rudder,
                    throttle: ship.throttle,
                    selected_powerups: ship.powerups.selected.clone(),
                    powerup_status,
                },
                contacts,
                events,
            };
            if let Err(e) = entry.outbound.try_send(tick_msg) {
                debug!(
                    room = %self.name,
                    bot = %bot_id,
                    error = %e,
                    "tick frame dropped (slow bot or closed channel)"
                );
            }
        }

        // 7. Snapshot who pinged this tick so next tick's passive listeners can hear them.
        self.previous_active_pingers = self
            .bots
            .values()
            .filter(|b| b.sensor_mode == SensorMode::Active)
            .map(|b| b.ship_id.clone())
            .collect();

        // Record the deadline reference *after* the broadcast so the bot's allotted
        // window starts when it could actually have received the frame.
        self.tick_send_time = Some(Instant::now());
    }

    /// Returns `Some(winner)` if the match should end this tick, where `winner` is the
    /// `BotId` of the winning bot (or `None` for a draw). Returns `None` if the match
    /// continues.
    ///
    /// End conditions per §5.5:
    /// - At most one ship alive → that ship's bot wins (or draw if none alive).
    /// - `world.tick >= MATCH_TIMEOUT_TICKS` → highest HP wins; tiebreak by highest
    ///   remaining ammo (== lowest used).
    fn match_outcome(&self) -> Option<Option<BotId>> {
        let alive: Vec<&Ship> = self.world.ships.values().filter(|s| s.alive).collect();
        // The "last ship standing" rule only fires when at least two bots actually
        // started; otherwise a 1-bot sandbox would end on tick 1.
        if self.starting_bot_count >= 2 && alive.len() <= 1 {
            return Some(alive.first().map(|s| s.bot_id.clone()));
        }
        if self.world.tick >= MATCH_TIMEOUT_TICKS {
            // BTreeMap iteration is BotId-stable, so `max_by_key` deterministically
            // resolves further ties by BotId order (later wins).
            let winner = alive
                .iter()
                .max_by_key(|s| (s.hp, s.ammo))
                .map(|s| s.bot_id.clone());
            return Some(winner);
        }
        None
    }

    /// Resolve a `ShipId` to its owning `BotId` via the ship registry. Dead ships stay in
    /// `world.ships`, so this works for the firer of a lethal shot too.
    fn bot_for_ship(&self, ship_id: &ShipId) -> Option<BotId> {
        self.world.ships.get(ship_id).map(|s| s.bot_id.clone())
    }

    /// Fold one tick's combat events into `match_stats`. Damage taken is credited to the
    /// victim; damage dealt, hits and kills to the firing ship's bot (friendly fire and
    /// self-kills included — the report shows raw figures, not adjusted scores).
    fn accumulate_combat_stats(&mut self, events: &[CombatEvent]) {
        for event in events {
            match event {
                CombatEvent::Hit {
                    ship_id,
                    amount,
                    source,
                    ..
                } => {
                    if let Some(victim) = self.bot_for_ship(ship_id) {
                        if let Some(stats) = self.match_stats.get_mut(&victim) {
                            stats.damage_taken += *amount;
                        }
                    }
                    if let Some(shooter) = self.bot_for_ship(source) {
                        if let Some(stats) = self.match_stats.get_mut(&shooter) {
                            stats.damage_dealt += *amount;
                            stats.hits_landed += 1;
                        }
                    }
                }
                CombatEvent::Death { source, .. } => {
                    if let Some(shooter) = self.bot_for_ship(source) {
                        if let Some(stats) = self.match_stats.get_mut(&shooter) {
                            stats.kills += 1;
                        }
                    }
                }
                CombatEvent::Splash { .. } => {}
            }
        }
    }

    /// Freeze the current match into a [`MatchReport`]. `aborted` distinguishes an
    /// operator abort (no winner) from a natural draw.
    fn build_match_report(&self, winner: Option<BotId>, aborted: bool) -> MatchReport {
        let outcome = if aborted {
            "aborted"
        } else if winner.is_some() {
            "winner"
        } else {
            "draw"
        };
        let winner_name = winner
            .as_ref()
            .and_then(|w| self.bots.get(w).map(|b| b.name.clone()));
        let duration_ticks = self.world.tick;
        let duration_seconds = if self.tick_hz > 0 {
            duration_ticks as f32 / self.tick_hz as f32
        } else {
            0.0
        };
        let bots = self
            .bots
            .values()
            .map(|entry| {
                let stats = self
                    .match_stats
                    .get(&entry.bot_id)
                    .cloned()
                    .unwrap_or_default();
                let ship = self.world.ships.get(&entry.ship_id);
                let accuracy = if stats.shots_fired > 0 {
                    stats.hits_landed as f32 / stats.shots_fired as f32
                } else {
                    0.0
                };
                BotReport {
                    bot_id: entry.bot_id.clone(),
                    name: entry.name.clone(),
                    shots_fired: stats.shots_fired,
                    hits_landed: stats.hits_landed,
                    accuracy,
                    damage_dealt: stats.damage_dealt,
                    damage_taken: stats.damage_taken,
                    kills: stats.kills,
                    final_hp: ship.map(|s| s.hp).unwrap_or(0),
                    survived: ship.map(|s| s.alive).unwrap_or(false),
                }
            })
            .collect();
        MatchReport {
            room: self.name.clone(),
            replay_id: self.replay_id.clone(),
            outcome: outcome.into(),
            winner,
            winner_name,
            duration_ticks,
            duration_seconds,
            bots,
        }
    }

    /// Build a `SpectatorMsg::World` from the current world state and the given combat
    /// events. Shared by the live broadcast path and offline replay capture.
    fn build_spectator_world(
        &self,
        events: &[CombatEvent],
        activations: &[(ShipId, PowerupId)],
    ) -> SpectatorMsg {
        // Ships in BotId order via the bot registry, so the wire payload is stable across
        // identical runs. Falling back to `world.ships` would also be deterministic
        // (BTreeMap on ShipId), but going through `bots` keeps `bot_name` in lock-step.
        let world_tick = self.world.tick;
        let cps_window = u64::from(self.tick_hz);
        let cps_cutoff = world_tick.saturating_sub(cps_window.saturating_sub(1));
        let ships: Vec<SpectatorShip> = self
            .bots
            .values()
            .filter_map(|entry| {
                let ship = self.world.ships.get(&entry.ship_id)?;
                let recent = entry
                    .command_ticks
                    .iter()
                    .filter(|&&t| t >= cps_cutoff)
                    .count() as f32;
                let powerup_status: Vec<PowerupStatus> = ship
                    .powerups
                    .selected
                    .iter()
                    .map(|id| PowerupStatus {
                        id: *id,
                        used: ship.powerups.used.contains(id),
                        active_ticks_left: ship.powerups.ticks_remaining(*id, world_tick),
                    })
                    .collect();
                Some(SpectatorShip {
                    id: ship.id.clone(),
                    bot_name: entry.name.clone(),
                    pos: [ship.pos.x, ship.pos.y],
                    heading_deg: ship.heading_deg,
                    speed: ship.speed,
                    hp: ship.hp,
                    ammo: ship.ammo,
                    throttle: ship.throttle,
                    rudder: ship.rudder,
                    alive: ship.alive,
                    ready: entry.ready,
                    commands_per_sec: recent,
                    sensor_mode: entry.sensor_mode,
                    selected_powerups: ship.powerups.selected.clone(),
                    powerup_status,
                })
            })
            .collect();

        let shells: Vec<SpectatorShell> = self
            .world
            .shells
            .iter()
            .map(|s| SpectatorShell {
                id_index: s.id_index,
                pos: [s.pos.x, s.pos.y],
                vel: [s.vel.x, s.vel.y],
                ttl_ticks: s.ttl_ticks,
            })
            .collect();

        let mut spec_events: Vec<SpectatorEvent> = events
            .iter()
            .map(|e| match e {
                CombatEvent::Hit {
                    ship_id, amount, ..
                } => SpectatorEvent::Hit {
                    ship_id: ship_id.clone(),
                    amount: *amount,
                },
                CombatEvent::Splash { pos } => SpectatorEvent::ShellSplash {
                    pos: [pos.x, pos.y],
                },
                CombatEvent::Death { ship_id, .. } => SpectatorEvent::Death {
                    ship_id: ship_id.clone(),
                },
            })
            .collect();
        for (ship_id, powerup) in activations {
            spec_events.push(SpectatorEvent::PowerupActivated {
                ship_id: ship_id.clone(),
                powerup: *powerup,
            });
        }

        let smoke_clouds: Vec<SpectatorSmokeCloud> = self
            .world
            .smoke_clouds
            .iter()
            .map(|c| SpectatorSmokeCloud {
                pos: [c.pos.x, c.pos.y],
                radius: c.radius,
                expires_at: c.expires_at,
            })
            .collect();
        let decoys: Vec<SpectatorDecoy> = self
            .world
            .decoys
            .iter()
            .map(|d| SpectatorDecoy {
                fake_id: d.fake_id,
                owner: d.owner.clone(),
                pos: [d.pos.x, d.pos.y],
                heading_deg: d.heading_deg,
                expires_at: d.expires_at,
            })
            .collect();

        SpectatorMsg::World {
            tick: self.world.tick,
            ships,
            shells,
            events: spec_events,
            smoke_clouds,
            decoys,
        }
    }

    /// Snapshot the current world as a `SpectatorMsg::World` with no combat events.
    /// Replay capture uses this for the tick-0 frame, which precedes any simulation step.
    pub fn spectator_world_snapshot(&self) -> SpectatorMsg {
        self.build_spectator_world(&[], &[])
    }

    /// Build a `SpectatorMsg::World` and push it onto the spectator broadcast channel.
    /// No-op when no channel is wired (unit tests). Send failures (no subscribers) are
    /// intentionally swallowed — the simulation never stalls because nobody is watching.
    ///
    /// In Monte Carlo mode the per-tick JSON serialization would dominate runtime, so
    /// the room throttles broadcasts to every Nth tick (configurable). `game_over` ticks
    /// always emit a frame so the spectator UI sees the deciding frame regardless of
    /// throttle.
    fn broadcast_spectator_world(
        &self,
        events: &[CombatEvent],
        activations: &[(ShipId, PowerupId)],
    ) {
        let Some(tx) = self.spectator_tx.as_ref() else {
            return;
        };
        if tx.receiver_count() == 0 {
            // Nothing to do; skip the JSON serialization cost when nobody's watching.
            return;
        }
        // Spectator throttling for MC mode. `0` disables non-final broadcasts entirely;
        // any positive value gates on `tick % throttle == 0`. The deciding tick (the one
        // accompanied by combat events that produced a death, or an activation worth
        // highlighting) bypasses the gate so the UI never misses the final frame.
        let throttle = self.spectator_throttle();
        let is_deciding_tick = events
            .iter()
            .any(|e| matches!(e, CombatEvent::Death { .. }));
        if throttle == 0 && !is_deciding_tick {
            return;
        }
        if throttle > 1 && !is_deciding_tick && !self.world.tick.is_multiple_of(u64::from(throttle))
        {
            return;
        }
        let msg = self.build_spectator_world(events, activations);
        let json = match serde_json::to_string(&msg) {
            Ok(s) => s,
            Err(e) => {
                warn!(room = %self.name, error = %e, "failed to serialize spectator world");
                return;
            }
        };
        // SendError only fires when there are no active receivers; we already guarded
        // above, but a race is possible. Either way, swallowing it is correct.
        let _ = tx.send(Arc::new(json));
    }

    /// Operator-triggered abort. Force-ends a running match with no winner and starts
    /// the post-game pause; the room will auto-return to `Lobby` after
    /// `POST_GAME_LOBBY_TICKS` ticks, identical to a natural match end.
    fn abort_match(&mut self) -> Result<(), AbortError> {
        if self.state != RoomState::Running {
            return Err(AbortError::NotRunning);
        }
        info!(room = %self.name, tick = self.world.tick, "match aborted by operator");
        self.state = RoomState::Ended;
        self.end_tick = Some(self.world.tick);
        self.last_winner = None;
        self.last_report = Some(self.build_match_report(None, true));
        self.broadcast_game_over(None);
        Ok(())
    }

    /// Operator-triggered reset. Only valid in `Ended`; cuts the post-game pause short
    /// and returns to `Lobby` immediately so the operator can start the next match
    /// without waiting out the timer.
    fn reset_to_lobby(&mut self) -> Result<(), ResetError> {
        if self.state != RoomState::Ended {
            return Err(ResetError::NotEnded);
        }
        self.transition_to_lobby();
        Ok(())
    }

    /// Return the room to `Lobby` after a match. Clears world state (shells, ship damage
    /// and motion), clears per-bot `ready` flags, reseeds the RNG so the next match is
    /// deterministic from the same `seed`, and broadcasts `ServerMsg::Lobby` to every
    /// bot so SDKs can rearm.
    fn transition_to_lobby(&mut self) {
        info!(room = %self.name, "returning to lobby for next match");
        let center = Vec2::new(self.world.width * 0.5, self.world.height * 0.5);
        let config = self.world.config;
        self.world.tick = 0;
        self.world.shells.clear();
        self.world.next_shell_index = 0;
        self.world.smoke_clouds.clear();
        self.world.decoys.clear();
        self.world.next_decoy_index = 0;
        for entry in self.bots.values_mut() {
            if let Some(ship) = self.world.ships.get_mut(&entry.ship_id) {
                ship.reset_for_round(center, 0.0, &config);
                // Returning to Lobby drops the committed loadout — bots may rebind for
                // the next match. The empty list is reflected back through the room's
                // copy via `register_bot`-time defaults; we also clear here for clarity.
                ship.powerups.selected.clear();
            }
            entry.ready = false;
            entry.pending_command = None;
            entry.sensor_mode = SensorMode::Passive;
            entry.last_fire_error_tick = None;
            entry.command_ticks.clear();
            entry.selected_powerups.clear();
        }
        self.tick_send_time = None;
        self.previous_active_pingers.clear();
        self.starting_bot_count = 0;
        self.end_tick = None;
        self.state = RoomState::Lobby;
        self.rng = Pcg64::seed_from_u64(self.seed);
        for entry in self.bots.values() {
            let msg = ServerMsg::Lobby { tick: 0 };
            if let Err(e) = entry.outbound.try_send(msg) {
                debug!(
                    room = %self.name,
                    bot = %entry.bot_id,
                    error = %e,
                    "lobby frame not delivered"
                );
            }
        }
    }

    /// Remove a bot from the room and delete its ship. Called both from the natural
    /// disconnect path (the connection task observed a close) and from operator kick.
    fn handle_bot_disconnect(&mut self, bot_id: BotId, reason: &'static str) {
        if let Some(entry) = self.bots.remove(&bot_id) {
            self.world.ships.remove(&entry.ship_id);
            info!(
                room = %self.name,
                bot = %bot_id,
                ship = %entry.ship_id,
                reason,
                "bot removed"
            );
        }
        // A Monte Carlo run requires a stable roster; any disconnect aborts the run.
        // We finalize the controller state but do not force-abort the current match —
        // it will end naturally (or by the timeout) and the chain logic will see the
        // run state is gone and stop.
        if self.mc_run.is_some() {
            self.mc_abort("bot_disconnected");
        }
    }

    /// Publish the current room state to admin subscribers. No-op when no admin channel
    /// is wired or no admin client is currently connected.
    fn publish_admin_state(&self) {
        let Some(tx) = self.admin_tx.as_ref() else {
            return;
        };
        if tx.receiver_count() == 0 {
            return;
        }
        let _ = tx.send(AdminServerMsg::State(self.admin_state_snapshot()));
    }

    /// Build a snapshot of room state suitable for the admin wire protocol.
    fn admin_state_snapshot(&self) -> AdminState {
        let state_str = match self.state {
            RoomState::Lobby => "lobby",
            RoomState::Running => "running",
            RoomState::Ended => "ended",
        };
        let bots = self
            .bots
            .values()
            .map(|entry| AdminBotInfo {
                bot_id: entry.bot_id.clone(),
                name: entry.name.clone(),
                ship_id: entry.ship_id.clone(),
                ready: entry.ready,
                alive: self
                    .world
                    .ships
                    .get(&entry.ship_id)
                    .map(|s| s.alive)
                    .unwrap_or(false),
            })
            .collect();
        AdminState {
            room: self.name.clone(),
            state: state_str.into(),
            tick: self.world.tick,
            last_winner: self.last_winner.clone(),
            bots,
        }
    }

    /// Send `game_over` to every registered bot — alive or dead. The dead bots' channels
    /// have been kept open precisely so they can receive this message. Also writes the
    /// terminal `end` record to the replay log and drops the writer (which flushes the
    /// underlying file).
    fn broadcast_game_over(&mut self, winner: Option<BotId>) {
        let final_tick = self.world.tick;
        let replay_id = self
            .replay_id
            .clone()
            .unwrap_or_else(|| format!("match_{}_{}", self.name, final_tick));
        info!(
            room = %self.name,
            final_tick,
            winner = ?winner,
            "match ended"
        );
        for entry in self.bots.values() {
            let msg = ServerMsg::GameOver {
                winner: winner.clone(),
                final_tick,
                replay_id: replay_id.clone(),
            };
            if let Err(e) = entry.outbound.try_send(msg) {
                debug!(
                    room = %self.name,
                    bot = %entry.bot_id,
                    error = %e,
                    "game_over not delivered"
                );
            }
        }

        // Close the replay log: write the terminal record, then drop the writer so the
        // BufWriter flushes to disk.
        if let Some(writer) = self.replay_writer.as_mut() {
            let end = ReplayRecord::End(ReplayEnd {
                tick: final_tick,
                winner: winner.clone(),
            });
            if let Err(e) = writer.write(&end) {
                warn!(room = %self.name, error = %e, "failed to write replay end record");
            }
        }
        if let Some(writer) = self.replay_writer.take() {
            if let Some(path) = writer.path() {
                info!(
                    room = %self.name,
                    replay_id = %replay_id,
                    path = %path.display(),
                    "replay log closed"
                );
            }
            drop(writer);
        }
    }

    /// If this bot has a pending counter-battery trace reveal, append a synthetic precise
    /// contact for the attacker to `contacts` while the reveal track is live. Non-consuming:
    /// the track is time-bounded by `trace_reveal_until` (refreshed on each hit during the
    /// armed window), so this just reads it rather than decrementing a counter. The synthetic
    /// contact carries a `cbt_<index>` id and full confidence so bots can tell it apart from a
    /// regular sensor return if they want to.
    fn consume_counter_battery_reveal(
        &mut self,
        ship_id: &ShipId,
        viewer_pos: Vec2,
        contacts: &mut Vec<ProtocolContact>,
    ) {
        let tick = self.world.tick;
        let attacker_pos = {
            let ship = match self.world.ships.get(ship_id) {
                Some(s) => s,
                None => return,
            };
            if ship.powerups.trace_reveal_until <= tick {
                // Track expired (or never started). Clear the stale attacker reference.
                if ship.powerups.trace_attacker.is_some() {
                    if let Some(s) = self.world.ships.get_mut(ship_id) {
                        s.powerups.trace_attacker = None;
                    }
                }
                return;
            }
            let attacker_id = match &ship.powerups.trace_attacker {
                Some(a) => a.clone(),
                None => return,
            };
            self.world.ships.get(&attacker_id).map(|s| s.pos)
        };
        // Insert at front so the trace is easy to find regardless of how many other
        // contacts the sensor returned.
        if let Some(pos) = attacker_pos {
            let to = pos - viewer_pos;
            let dist = to.length();
            let bearing = {
                let deg = to.x.atan2(-to.y).to_degrees();
                if deg < 0.0 {
                    deg + 360.0
                } else {
                    deg
                }
            };
            let id_index = contacts.len();
            contacts.insert(
                0,
                ProtocolContact {
                    id: format!("cbt_{id_index}"),
                    kind: ProtocolContactKind::Ship,
                    pos: [pos.x, pos.y],
                    bearing_deg: bearing,
                    range: Some(dist),
                    confidence: 1.0,
                },
            );
        }
        // Non-consuming: the track is bounded by `trace_reveal_until` (checked above), so
        // there's no counter to decrement here. A missing attacker (e.g. disconnected) simply
        // produces no contact this tick while the track is live.
    }

    /// Coarse "would the viewer currently see this ship" check, used to gate
    /// `PowerupActivated` events. Mirrors the sensor module's range rules without
    /// re-running the RNG: any ship inside the relevant range counts as visible. Smoke
    /// blocks for active; silent_running hides from passive.
    fn is_ship_visible_to(
        &self,
        viewer_ship_id: &ShipId,
        viewer_pos: Vec2,
        sensor_mode: SensorMode,
        target_ship_id: &ShipId,
    ) -> bool {
        let Some(target) = self.world.ships.get(target_ship_id) else {
            return false;
        };
        if !target.alive {
            return false;
        }
        let tick = self.world.tick;
        let config = &self.world.config;
        let dist = target.pos.distance(viewer_pos);
        match sensor_mode {
            SensorMode::Active => {
                let viewer = self.world.ships.get(viewer_ship_id);
                if let Some(v) = viewer {
                    if v.powerups.is_active(PowerupId::EmpBurst, tick) {
                        return false;
                    }
                }
                let awacs = viewer
                    .map(|v| v.powerups.is_active(PowerupId::AwacsScan, tick))
                    .unwrap_or(false);
                let base_range = config.active_radar_range;
                let radar_range = if awacs {
                    base_range * config.powerups.awacs_range_mult
                } else {
                    base_range
                };
                let effective_range =
                    if target.powerups.is_active(PowerupId::SilentRunning, tick) && !awacs {
                        radar_range * config.powerups.silent_running_active_range_mult
                    } else {
                        radar_range
                    };
                if dist > effective_range {
                    return false;
                }
                // Smoke blocks active sight when target is in a cloud the viewer isn't in.
                for cloud in &self.world.smoke_clouds {
                    if cloud.expires_at <= tick {
                        continue;
                    }
                    let target_in = target.pos.distance(cloud.pos) <= cloud.radius;
                    if !target_in {
                        continue;
                    }
                    let viewer_in = viewer_pos.distance(cloud.pos) <= cloud.radius;
                    if !viewer_in {
                        return false;
                    }
                }
                true
            }
            SensorMode::Passive => {
                if target.powerups.is_active(PowerupId::SilentRunning, tick) {
                    return false;
                }
                let nearby = config.passive_hear_nearby_range;
                let active_hear = config.passive_hear_active_range;
                let pinging = self.previous_active_pingers.contains(target_ship_id);
                dist <= nearby || (pinging && dist <= active_hear)
            }
        }
    }

    /// Translate an `ActivationError` into a typed protocol error frame.
    fn send_activation_error(&mut self, bot_id: &BotId, id: PowerupId, err: ActivationError) {
        let (code, msg): (&str, String) = match err {
            ActivationError::NotSelected => (
                error_code::POWERUP_NOT_SELECTED,
                format!("powerup `{}` was not picked for this match", id.as_str()),
            ),
            ActivationError::AlreadyUsed => (
                error_code::POWERUP_ALREADY_USED,
                format!(
                    "powerup `{}` has already been activated this match",
                    id.as_str()
                ),
            ),
            // Dead ships / unknown ships are silent — bot is about to get `game_over` or
            // already disconnected.
            ActivationError::ShipDead | ActivationError::UnknownShip => return,
        };
        let Some(entry) = self.bots.get_mut(bot_id) else {
            return;
        };
        if let Err(e) = entry.outbound.try_send(protocol::error_msg(code, msg)) {
            debug!(
                room = %self.name,
                bot = %bot_id,
                error = %e,
                "powerup activation error not delivered",
            );
        }
    }

    /// Translate a `FireError` into a protocol error and queue it on the bot's outbound
    /// channel. `ShipDead` and `UnknownShip` are silent — the bot already received (or
    /// is about to receive) `game_over`; spamming an error message would be noise.
    ///
    /// Cooldown / no-ammo errors are coalesced: at most one per (bot, tick). A bot that
    /// blindly issues `fire` every tick would otherwise queue one error frame per tick
    /// into a 32-slot outbound buffer, pushing real `tick` frames over the edge.
    fn send_fire_error(&mut self, bot_id: &BotId, err: FireError) {
        let (code, msg): (&str, String) = match err {
            FireError::CooldownActive => {
                // Look up the ship's remaining cooldown so the bot knows how long to wait.
                let cooldown_remaining = self
                    .bots
                    .get(bot_id)
                    .and_then(|entry| self.world.ships.get(&entry.ship_id))
                    .map(|ship| ship.gun_cooldown)
                    .unwrap_or(0);
                (
                    error_code::COOLDOWN_ACTIVE,
                    format!(
                        "gun on cooldown at tick {}: {} tick(s) remaining",
                        self.world.tick, cooldown_remaining,
                    ),
                )
            }
            FireError::NoAmmo => (
                error_code::NO_AMMO,
                format!(
                    "ship is out of ammo (no resupply during a match); rejected at tick {}",
                    self.world.tick,
                ),
            ),
            FireError::ShipDead | FireError::UnknownShip => return,
        };
        let world_tick = self.world.tick;
        let Some(entry) = self.bots.get_mut(bot_id) else {
            return;
        };
        if entry.last_fire_error_tick == Some(world_tick) {
            return;
        }
        entry.last_fire_error_tick = Some(world_tick);
        if let Err(e) = entry.outbound.try_send(protocol::error_msg(code, msg)) {
            debug!(
                room = %self.name,
                bot = %bot_id,
                error = %e,
                "fire error not delivered"
            );
        }
    }

    /// Apply a single `RoomEvent`. The connection task waits on `oneshot` replies; other
    /// events are fire-and-forget.
    pub fn handle_event(&mut self, event: RoomEvent) {
        match event {
            RoomEvent::BotConnect {
                peer,
                name,
                version,
                reply,
            } => {
                let result = self.register_bot(peer, name, &version);
                let _ = reply.send(result);
                self.publish_admin_state();
            }
            RoomEvent::BotReady { bot_id } => {
                let mut changed = false;
                if let Some(entry) = self.bots.get_mut(&bot_id) {
                    if !entry.ready {
                        entry.ready = true;
                        changed = true;
                        info!(room = %self.name, bot = %bot_id, "bot ready");
                    }
                } else {
                    warn!(room = %self.name, bot = %bot_id, "ready from unknown bot, ignored");
                }
                if changed {
                    self.publish_admin_state();
                }
            }
            RoomEvent::BotSelectPowerups { bot_id, powerups } => {
                self.handle_select_powerups(bot_id, powerups);
            }
            RoomEvent::BotCommand { bot_id, command } => {
                self.handle_bot_command(bot_id, command);
            }
            RoomEvent::BotDisconnect { bot_id } => {
                if self.bots.contains_key(&bot_id) {
                    self.handle_bot_disconnect(bot_id, "disconnected");
                    self.publish_admin_state();
                }
            }
            RoomEvent::OperatorStart { room, reply } => {
                let result = self.start_match(&room);
                if let Err(ref e) = result {
                    warn!(room = %self.name, requested = %room, reason = e.as_str(), "operator start refused");
                }
                let _ = reply.send(result);
                self.publish_admin_state();
            }
            RoomEvent::OperatorAbort { reply } => {
                let result = self.abort_match();
                if let Err(ref e) = result {
                    warn!(room = %self.name, reason = e.as_str(), "operator abort refused");
                }
                let _ = reply.send(result);
                self.publish_admin_state();
            }
            RoomEvent::OperatorReset { reply } => {
                let result = self.reset_to_lobby();
                if let Err(ref e) = result {
                    warn!(room = %self.name, reason = e.as_str(), "operator reset refused");
                }
                let _ = reply.send(result);
                self.publish_admin_state();
            }
            RoomEvent::OperatorKick { bot_id, reply } => {
                let result = if self.bots.contains_key(&bot_id) {
                    self.handle_bot_disconnect(bot_id.clone(), "kicked by operator");
                    Ok(())
                } else {
                    Err(KickError::UnknownBot)
                };
                if let Err(ref e) = result {
                    warn!(room = %self.name, bot = %bot_id, reason = e.as_str(), "operator kick refused");
                }
                let _ = reply.send(result);
                self.publish_admin_state();
            }
            RoomEvent::AdminSubscribe { reply } => {
                if let Some(tx) = self.admin_tx.as_ref() {
                    let rx = tx.subscribe();
                    // Push the current snapshot through the broadcast so the new
                    // receiver's first frame is the room state. The send may report no
                    // active receivers (the reply hasn't been delivered yet), but the
                    // tokio broadcast queues the message internally so the next `recv`
                    // on `rx` will still pick it up.
                    let _ = tx.send(AdminServerMsg::State(self.admin_state_snapshot()));
                    let _ = reply.send(rx);
                }
                // No-op when no admin channel is wired (unit tests).
            }
            RoomEvent::QueryState { reply } => {
                let _ = reply.send(RoomSnapshot {
                    state: self.admin_state_snapshot(),
                    config: self.world.config,
                });
            }
            RoomEvent::QueryReport { reply } => {
                let _ = reply.send(self.last_report.clone());
            }
            RoomEvent::OperatorConfigure { config, reply } => {
                let result = self.configure(config);
                if let Err(ref e) = result {
                    warn!(room = %self.name, reason = e.as_str(), "operator configure refused");
                }
                let _ = reply.send(result);
                self.publish_admin_state();
            }
            RoomEvent::StartMonteCarlo { config, reply } => {
                let result = self.start_monte_carlo(config);
                if let Err(ref e) = result {
                    warn!(room = %self.name, reason = e.as_str(), "monte carlo start refused");
                }
                let _ = reply.send(result);
                self.publish_admin_state();
            }
            RoomEvent::StopMonteCarlo { force_abort, reply } => {
                let result = self.stop_monte_carlo(force_abort);
                if let Err(ref e) = result {
                    warn!(room = %self.name, reason = e.as_str(), "monte carlo stop refused");
                }
                let _ = reply.send(result);
                self.publish_admin_state();
            }
            RoomEvent::QueryMonteCarloStatus { reply } => {
                let _ = reply.send(self.mc_status_snapshot());
            }
        }
    }

    /// Apply an operator-supplied [`SimConfig`]. Only valid in `Lobby`; the parameters are
    /// validated before they replace the match config.
    fn configure(&mut self, config: SimConfig) -> Result<(), ConfigureError> {
        if self.state != RoomState::Lobby {
            return Err(ConfigureError::NotInLobby);
        }
        config.validate().map_err(ConfigureError::Invalid)?;
        self.world.config = config;
        info!(room = %self.name, "match parameters updated");
        Ok(())
    }

    /// Validate `powerups` and record them on the bot's entry. Constraints (per
    /// `docs/POWERUPS.md`): exactly two distinct entries, both must be known ids, and the
    /// room must be in `Lobby`. On failure the previous selection (if any) is preserved
    /// and a typed `error` frame is sent to the bot.
    fn handle_select_powerups(&mut self, bot_id: BotId, powerups: Vec<PowerupId>) {
        let state = self.state;
        let Some(entry) = self.bots.get_mut(&bot_id) else {
            warn!(
                room = %self.name,
                bot = %bot_id,
                "select_powerups from unknown bot, ignored"
            );
            return;
        };
        if state != RoomState::Lobby {
            let _ = entry.outbound.try_send(protocol::error_msg(
                error_code::POWERUP_LOBBY_ONLY,
                "powerup selection is only accepted while the room is in lobby",
            ));
            return;
        }
        if powerups.len() != 2 {
            let _ = entry.outbound.try_send(protocol::error_msg(
                error_code::POWERUP_WRONG_COUNT,
                format!(
                    "select_powerups requires exactly 2 entries, got {}",
                    powerups.len()
                ),
            ));
            return;
        }
        if powerups[0] == powerups[1] {
            let _ = entry.outbound.try_send(protocol::error_msg(
                error_code::POWERUP_DUPLICATE,
                "select_powerups requires two distinct powerups",
            ));
            return;
        }
        entry.selected_powerups = powerups;
        info!(
            room = %self.name,
            bot = %bot_id,
            picks = ?entry.selected_powerups,
            "bot loadout selected",
        );
        self.publish_admin_state();
    }

    /// Queue a command for the next tick or reject it as `late_command` per §1.3 of the
    /// protocol. Late commands leave `pending_command` untouched so the previous tick's
    /// throttle / rudder / sensor_mode persist. Out-of-running-state commands are dropped
    /// silently — the ship has nothing to drive yet.
    fn handle_bot_command(&mut self, bot_id: BotId, command: PendingCommand) {
        let now = Instant::now();
        let state = self.state;
        let deadline_ms = self.tick_deadline_ms;
        let send_time = self.tick_send_time;
        let world_tick = self.world.tick;

        let Some(entry) = self.bots.get_mut(&bot_id) else {
            warn!(room = %self.name, bot = %bot_id, "command from unknown bot, ignored");
            return;
        };

        if state == RoomState::Running {
            // The bot must echo the tick of the last frame it received. Accept the current
            // tick plus a one-tick window for racing frame boundaries; anything further
            // out is either a confused bot or a replay attempt.
            let max_lag: u64 = 1;
            let min_acceptable = world_tick.saturating_sub(max_lag);
            let max_acceptable = world_tick.saturating_add(max_lag);
            if command.tick < min_acceptable || command.tick > max_acceptable {
                let err = protocol::error_msg(
                    error_code::STALE_COMMAND,
                    format!(
                        "command.tick {} is outside the accepted window [{}, {}]",
                        command.tick, min_acceptable, max_acceptable,
                    ),
                );
                if let Err(e) = entry.outbound.try_send(err) {
                    debug!(
                        room = %self.name,
                        bot = %bot_id,
                        error = %e,
                        "couldn't push stale_command error",
                    );
                }
                debug!(
                    room = %self.name,
                    bot = %bot_id,
                    command_tick = command.tick,
                    world_tick,
                    "rejected stale command",
                );
                return;
            }

            if let Some(t) = send_time {
                let elapsed = now.duration_since(t);
                if elapsed.as_millis() > u128::from(deadline_ms) {
                    let err = protocol::error_msg(
                        error_code::LATE_COMMAND,
                        format!(
                            "command for tick {} arrived {}ms after frame (deadline {}ms)",
                            command.tick,
                            elapsed.as_millis(),
                            deadline_ms,
                        ),
                    );
                    if let Err(e) = entry.outbound.try_send(err) {
                        debug!(
                            room = %self.name,
                            bot = %bot_id,
                            error = %e,
                            "couldn't push late_command error",
                        );
                    }
                    debug!(
                        room = %self.name,
                        bot = %bot_id,
                        elapsed_ms = elapsed.as_millis(),
                        deadline_ms,
                        "rejected late command",
                    );
                    return;
                }
            }
        }

        entry.pending_command = Some(command);
        record_command_tick(
            &mut entry.command_ticks,
            world_tick,
            u64::from(self.tick_hz),
        );
    }

    /// Operator-triggered transition `Lobby` → `Running`. Places ships on the §5.6 ring
    /// (radius `STARTING_RING_RADIUS` around map center, all facing center), broadcasts
    /// `game_start` to every registered bot, and resets the tick counter to 0.
    fn start_match(&mut self, room_name: &str) -> Result<(), StartError> {
        if room_name != self.name {
            return Err(StartError::UnknownRoom);
        }
        if self.state != RoomState::Lobby {
            return Err(StartError::NotInLobby);
        }
        if self.bots.is_empty() {
            return Err(StartError::NoBots);
        }
        if !self.bots.values().all(|b| b.ready) {
            return Err(StartError::NotAllReady);
        }

        let n_bots = self.bots.len();
        let layout = default_ring_layout(self.world.width, self.world.height, n_bots);
        self.apply_match_layout(&layout);
        Ok(())
    }

    /// Internal helper: place ships using the precomputed `layout` (one entry per bot in
    /// `BotId` order), broadcast `game_start`, reset state and open the replay log.
    /// Shared by [`Room::start_match`] and the Monte Carlo per-match path.
    fn apply_match_layout(&mut self, layout: &[(Vec2, f32)]) {
        // Freeze the balance parameters for the whole match: ship hull / ammo and every
        // physics tunable are read from this snapshot from here on.
        let config = self.world.config;
        // Snapshot bot ids so we can mutate `self.world` and read `self.bots` without
        // simultaneous &mut+&. Iteration order is BotId-stable (BTreeMap).
        let ordered_ids: Vec<BotId> = self.bots.keys().cloned().collect();
        debug_assert_eq!(
            ordered_ids.len(),
            layout.len(),
            "layout must have one entry per registered bot",
        );

        for (i, bot_id) in ordered_ids.iter().enumerate() {
            let (pos, heading_deg) = layout[i];

            let (ship_id, selected_powerups) = {
                let entry = self.bots.get_mut(bot_id).expect("snapshot still in map");
                // Drop any commands queued before the match started.
                entry.pending_command = None;
                (entry.ship_id.clone(), entry.selected_powerups.clone())
            };
            if let Some(ship) = self.world.ships.get_mut(&ship_id) {
                // A fresh hull for the match, sized by the configured parameters.
                ship.reset_for_round(pos, heading_deg, &config);
                // Mirror the bot's loadout onto the ship so the simulation can read it
                // without going through the room.
                ship.powerups.selected = selected_powerups;
            }

            let entry = self.bots.get(bot_id).expect("snapshot still in map");
            let game_start = ServerMsg::GameStart {
                tick: 0,
                starting_position: [pos.x, pos.y],
                starting_heading_deg: heading_deg,
            };
            // Buffer is sized for many messages; on the rare full case we drop and log.
            if let Err(e) = entry.outbound.try_send(game_start) {
                warn!(room = %self.name, bot = %bot_id, error = %e, "game_start drop");
            }
        }

        self.world.tick = 0;
        self.world.shells.clear();
        self.world.next_shell_index = 0;
        // Drop any leftover smoke / decoy state from a previous match.
        self.world.smoke_clouds.clear();
        self.world.decoys.clear();
        self.world.next_decoy_index = 0;
        self.state = RoomState::Running;
        // Cleared on entry; the first `step_tick` will populate them after broadcasting.
        self.tick_send_time = None;
        self.previous_active_pingers.clear();
        self.starting_bot_count = self.bots.len() as u32;
        // Fresh match — the previous winner is no longer "the current winner". Admin
        // clients see this through the next `AdminServerMsg::State` push.
        self.last_winner = None;
        self.end_tick = None;
        // Fresh statistics: one zeroed entry per starting bot. The previous match's report
        // is dropped — `GET /api/room/report` 404s until this match finishes.
        self.match_stats = self
            .bots
            .keys()
            .map(|id| (id.clone(), BotStats::default()))
            .collect();
        self.last_report = None;
        info!(room = %self.name, bots = self.bots.len(), "match started");

        // Open a new replay log (unless a writer was injected externally — e.g. by tests)
        // and emit the header. Failures here are logged but not fatal: the match runs even
        // if we can't write the log.
        self.open_replay_writer_if_configured();
        self.write_replay_header();
    }

    /// If `replay_dir` was set and no writer is yet open, generate a replay id and create
    /// a `<dir>/<replay_id>.jsonl` writer. Errors are logged; the match continues without
    /// a log on failure.
    fn open_replay_writer_if_configured(&mut self) {
        if self.replay_writer.is_some() {
            return;
        }
        let Some(dir) = self.replay_dir.clone() else {
            return;
        };
        // Inside a Monte Carlo run, embed the run id + match index + seed so every
        // replay's filename uniquely identifies its position in the batch.
        let replay_id = match self.mc_run.as_ref() {
            Some(mc) => monte_carlo::make_mc_replay_id(&mc.run_id, mc.current_index + 1, self.seed),
            None => replay::make_replay_id(&self.name),
        };
        match ReplayWriter::create_file(&dir, replay_id.clone()) {
            Ok(writer) => {
                if let Some(path) = writer.path() {
                    info!(
                        room = %self.name,
                        replay_id = %replay_id,
                        path = %path.display(),
                        "replay log opened"
                    );
                }
                self.replay_id = Some(replay_id);
                self.replay_writer = Some(writer);
            }
            Err(e) => {
                warn!(
                    room = %self.name,
                    dir = %dir.display(),
                    error = %e,
                    "failed to open replay writer"
                );
            }
        }
    }

    /// Build and write the JSONL header from the current room/world state. No-op when no
    /// writer is open.
    fn write_replay_header(&mut self) {
        let Some(writer) = self.replay_writer.as_mut() else {
            return;
        };
        let header = ReplayHeader {
            version: REPLAY_FORMAT_VERSION,
            replay_id: writer.replay_id().to_string(),
            room: self.name.clone(),
            seed: self.seed,
            tick_hz: self.tick_hz,
            tick_deadline_ms: self.tick_deadline_ms,
            map: MapInfo {
                width: self.world.width as u32,
                height: self.world.height as u32,
            },
            max_bots: self.max_bots,
            sim_config: self.world.config,
            bots: {
                // `self.bots` is a `BTreeMap` keyed by the string `BotId`, so `.values()`
                // yields lexicographic order (`b_10` ahead of `b_2`). Sort into
                // registration order so the header matches what `rebuild_room_from_header`
                // reconstructs.
                let mut bots: Vec<ReplayBot> = self
                    .bots
                    .values()
                    .map(|b| {
                        // Record the ship's actual placed spawn (set by `apply_match_layout`
                        // just before this) so a replay reproduces the true starting layout
                        // regardless of the variance mode the live run used. Falls back to
                        // the origin only if the ship is somehow missing.
                        let (spawn_pos, spawn_heading_deg) = self
                            .world
                            .ships
                            .get(&b.ship_id)
                            .map(|s| ([s.pos.x, s.pos.y], s.heading_deg))
                            .unwrap_or(([0.0, 0.0], 0.0));
                        ReplayBot {
                            bot_id: b.bot_id.clone(),
                            ship_id: b.ship_id.clone(),
                            name: b.name.clone(),
                            selected_powerups: b.selected_powerups.clone(),
                            spawn_pos,
                            spawn_heading_deg,
                        }
                    })
                    .collect();
                bots.sort_by_key(|b| replay::bot_id_seq(&b.bot_id));
                bots
            },
        };
        if let Err(e) = writer.write(&ReplayRecord::Header(Box::new(header))) {
            warn!(room = %self.name, error = %e, "failed to write replay header");
        }
    }

    /// Append a `tick` record to the log. Skipped silently when no writer is open or no
    /// commands fired this tick — empty-tick lines aren't useful and just inflate the log.
    fn write_replay_tick(&mut self, commands: Vec<ReplayCommand>) {
        if commands.is_empty() {
            return;
        }
        let tick = self.world.tick;
        let Some(writer) = self.replay_writer.as_mut() else {
            return;
        };
        let record = ReplayRecord::Tick(ReplayTick { tick, commands });
        if let Err(e) = writer.write(&record) {
            warn!(room = %self.name, error = %e, "failed to write replay tick");
        }
    }

    // ---- Monte Carlo helpers ----------------------------------------------
    //
    // The state machine integration: an MC run lives in `mc_run`. Starting the run kicks
    // off the first match through `start_match_with_seeded_layout`. When `step_tick`
    // detects a match end, it records the outcome (`mc_record_match_end`) and either
    // chains to the next match or finalizes the run (`mc_advance_after_match`). A
    // disconnect mid-run aborts the controller via `mc_abort`.

    /// Start a Monte Carlo batch run. The room must be in `Lobby` with at least two
    /// ready bots; the first match is started synchronously here, subsequent matches
    /// are chained from inside `step_tick`. Returns the run id on success.
    fn start_monte_carlo(&mut self, config: McConfig) -> Result<String, McStartError> {
        if self.state != RoomState::Lobby {
            return Err(McStartError::NotInLobby);
        }
        if self.mc_run.is_some() {
            return Err(McStartError::AlreadyRunning);
        }
        config.validate().map_err(McStartError::Invalid)?;
        let ready_count = self.bots.values().filter(|b| b.ready).count();
        if ready_count < 2 {
            return Err(McStartError::InsufficientBots);
        }
        if ready_count != self.bots.len() {
            // Mirror the single-match precondition: all registered bots must be ready.
            return Err(McStartError::Invalid(
                "every connected bot must be ready before starting a monte carlo run".into(),
            ));
        }

        // Apply the optional SimConfig override once at the start of the run.
        if let Some(cfg) = config.sim_config {
            cfg.validate().map_err(McStartError::Invalid)?;
            self.world.config = cfg;
        }

        let started_at_unix = unix_secs();
        let run_id = make_mc_run_id(started_at_unix);
        let state = McState::new(config, run_id.clone(), started_at_unix);
        self.mc_run = Some(state);
        self.mc_last_status = None;
        info!(room = %self.name, run_id = %run_id, "monte carlo run started");

        // Kick off the first match. If that fails the run state is rolled back so a
        // subsequent attempt isn't blocked by `AlreadyRunning`.
        if let Err(e) = self.mc_begin_next_match() {
            self.mc_run = None;
            return Err(McStartError::Invalid(e));
        }
        Ok(run_id)
    }

    /// Stop the active Monte Carlo run. If `force_abort` is `true` and a match is in
    /// flight the room calls `abort_match` first; otherwise the controller exits at the
    /// next end-of-match boundary.
    fn stop_monte_carlo(&mut self, force_abort: bool) -> Result<(), McStopError> {
        if self.mc_run.is_none() {
            return Err(McStopError::NotRunning);
        }
        if force_abort && self.state == RoomState::Running {
            // Aborting the match drops to Ended, which step_tick won't auto-advance once
            // we drop the run state below — so finalize immediately.
            let _ = self.abort_match();
        }
        let mc = self.mc_run.take().expect("checked above");
        self.mc_last_status = Some(self.build_mc_status(&mc, false, Some("stopped")));
        info!(room = %self.name, run_id = %mc.run_id, "monte carlo run stopped");
        Ok(())
    }

    /// Called from `step_tick` after a match's `game_over` has been broadcast. Records
    /// the outcome on the MC state. Safe to call only while `mc_run.is_some()`.
    fn mc_record_match_end(
        &mut self,
        winner: Option<BotId>,
        duration_ticks: u64,
        replay_id: Option<String>,
    ) {
        let Some(mc) = self.mc_run.as_mut() else {
            return;
        };
        let winner_name = winner
            .as_ref()
            .and_then(|w| self.bots.get(w).map(|b| b.name.clone()));
        let seed = mc.seed_for_next_match();
        mc.record_result(winner, winner_name, duration_ticks, replay_id, seed);
    }

    /// Called from `step_tick` after `mc_record_match_end`. Either chains to the next
    /// match in the batch or finalizes the run.
    fn mc_advance_after_match(&mut self) {
        let has_more = self
            .mc_run
            .as_ref()
            .map(|m| m.has_more_matches())
            .unwrap_or(false);
        if has_more {
            // Disconnects between matches would have been handled separately; here we
            // double-check the roster is still healthy enough to keep going.
            if self.bots.len() < 2 {
                self.mc_finalize(false, Some("bot_disconnected"));
                return;
            }
            if let Err(e) = self.mc_begin_next_match() {
                warn!(room = %self.name, error = %e, "monte carlo: failed to start next match");
                self.mc_finalize(false, Some("error"));
            }
        } else {
            self.mc_finalize(true, Some("completed"));
        }
    }

    /// Internal: reset the world, compute a new layout from the next match seed, and
    /// start the next match. The room stays in `Running` from the bots' perspective —
    /// they see `game_over` immediately followed by `game_start` for match N+1.
    fn mc_begin_next_match(&mut self) -> Result<(), String> {
        let (seed, variance_mode, n_bots, width, height) = {
            let mc = self
                .mc_run
                .as_ref()
                .ok_or_else(|| "no active monte carlo run".to_string())?;
            (
                mc.seed_for_next_match(),
                mc.config.variance_mode,
                self.bots.len(),
                self.world.width,
                self.world.height,
            )
        };

        // Reseed the room RNG so every match starts from a known, varied position in the
        // PCG stream — and record the new seed so the replay header reflects it. Also
        // clear shells and per-bot match state via the same reset_for_round path the
        // lobby transition uses, but without dropping the `ready` flag (the bot is
        // already mid-batch).
        self.seed = seed;
        self.rng = Pcg64::seed_from_u64(seed);
        for entry in self.bots.values_mut() {
            entry.pending_command = None;
            entry.sensor_mode = SensorMode::Passive;
            entry.last_fire_error_tick = None;
            entry.command_ticks.clear();
        }
        self.world.shells.clear();
        self.world.next_shell_index = 0;
        self.tick_send_time = None;
        self.previous_active_pingers.clear();

        let layout =
            monte_carlo::place_ships_for_variance(variance_mode, seed, n_bots, width, height);
        // `apply_match_layout` opens a fresh replay writer (which uses the MC naming
        // scheme because `self.mc_run` is set), writes the new header, and broadcasts
        // `game_start` to every bot.
        self.apply_match_layout(&layout);
        Ok(())
    }

    /// Finalize a Monte Carlo run: snapshot its status, drop the active state, and log.
    /// `completed` is true when the run finished naturally (all matches done), false on
    /// abort/error/stop — surfaced in the status payload via `ended_reason`.
    fn mc_finalize(&mut self, completed: bool, reason: Option<&str>) {
        let Some(mc) = self.mc_run.take() else {
            return;
        };
        info!(
            room = %self.name,
            run_id = %mc.run_id,
            completed,
            "monte carlo run finished",
        );
        // Once finalized the run is no longer running — `running: false` regardless of
        // whether it completed or was stopped.
        self.mc_last_status = Some(self.build_mc_status(&mc, false, reason));
    }

    /// Abort the current MC run because of a fatal condition (e.g. bot disconnect during
    /// a running match). Surfaces the reason in the last-status snapshot.
    fn mc_abort(&mut self, reason: &'static str) {
        if let Some(mc) = self.mc_run.take() {
            warn!(room = %self.name, run_id = %mc.run_id, reason, "monte carlo run aborted");
            self.mc_last_status = Some(self.build_mc_status(&mc, false, Some(reason)));
            // If a match was in flight, end it with no winner so the bots get game_over.
            if self.state == RoomState::Running {
                let _ = self.abort_match();
            }
        }
    }

    fn build_mc_status(&self, state: &McState, running: bool, reason: Option<&str>) -> McStatus {
        let bot_names: BTreeMap<BotId, String> = self
            .bots
            .values()
            .map(|b| (b.bot_id.clone(), b.name.clone()))
            .collect();
        McStatus {
            running,
            run_id: state.run_id.clone(),
            completed: state.current_index,
            total: state.config.n_matches,
            variance_mode: state.config.variance_mode,
            mc_seed: state.config.mc_seed,
            started_at_unix: state.started_at_unix,
            finished_at_unix: if running { None } else { Some(unix_secs()) },
            current_match_tick: if running { self.world.tick } else { 0 },
            wins: state.wins.clone(),
            bot_names,
            draws: state.draws,
            results: state.results.clone(),
            ended_reason: reason.map(str::to_string),
        }
    }

    /// Public snapshot for `GET /api/montecarlo/status`. Reports the in-flight run's
    /// progress, or the last completed run's results, or an empty status if nothing has
    /// ever run.
    fn mc_status_snapshot(&self) -> McStatus {
        if let Some(mc) = self.mc_run.as_ref() {
            self.build_mc_status(mc, true, None)
        } else if let Some(snap) = self.mc_last_status.clone() {
            snap
        } else {
            McStatus::empty()
        }
    }

    /// `true` if the room is currently in lockstep mode (i.e. an MC run is active and
    /// the match is `Running`). The tick loop uses this to gate its pacing strategy.
    pub fn in_lockstep(&self) -> bool {
        self.mc_run.is_some() && self.state == RoomState::Running
    }

    /// `true` if every registered bot has a `pending_command` queued for the current
    /// tick. The lockstep tick loop steps immediately once this returns `true`.
    pub fn all_pending_commands_ready(&self) -> bool {
        !self.bots.is_empty() && self.bots.values().all(|b| b.pending_command.is_some())
    }

    /// Per-tick timeout configured by the active MC run, or a generous default outside
    /// MC mode. Used by `run_room` to bound how long it waits for the slowest bot.
    pub fn lockstep_timeout(&self) -> Duration {
        self.mc_run
            .as_ref()
            .map(|m| m.config.per_tick_timeout())
            .unwrap_or(Duration::from_secs(1))
    }

    /// Spectator broadcast cadence in MC mode (every Nth tick); `1` in normal mode.
    /// `0` means "do not broadcast at all this run".
    pub fn spectator_throttle(&self) -> u32 {
        self.mc_run
            .as_ref()
            .map(|m| m.config.effective_spectator_throttle())
            .unwrap_or(1)
    }

    fn register_bot(
        &mut self,
        peer: SocketAddr,
        name: String,
        version: &str,
    ) -> Result<BotRegistration, JoinError> {
        if self.state != RoomState::Lobby {
            return Err(JoinError::NotInLobby);
        }
        if self.bot_count() as u32 >= self.max_bots {
            return Err(JoinError::RoomFull);
        }
        // Defensive: net.rs already enforces the charset, but the room is the
        // authoritative gatekeeper for what ends up in replay logs and spectator UIs.
        if protocol::validate_bot_name(&name).is_err() {
            return Err(JoinError::InvalidName);
        }
        if self.bots.values().any(|b| b.name == name) {
            return Err(JoinError::DuplicateName);
        }

        let n = self.next_index;
        self.next_index += 1;
        let bot_id: BotId = format!("b_{n}");
        let ship_id: ShipId = format!("s_{n}");

        // Spawn a ship at the map center. The actual starting position is reset by
        // `game_start` (Phase 4.2) using the §5.6 ring layout once the bot count is final.
        let center = Vec2::new(self.world.width * 0.5, self.world.height * 0.5);
        self.world
            .insert_ship(Ship::new_at(ship_id.clone(), bot_id.clone(), center, 0.0));

        let (out_tx, out_rx) = mpsc::channel::<ServerMsg>(BOT_OUTBOUND_BUFFER);

        let welcome = ServerMsg::Welcome {
            bot_id: bot_id.clone(),
            ship_id: ship_id.clone(),
            map: MapInfo {
                width: self.world.width as u32,
                height: self.world.height as u32,
            },
            tick_hz: self.tick_hz,
            ship_specs: ShipSpecs::from_config(&self.world.config),
            available_powerups: PowerupId::all().to_vec(),
        };
        // The receiver was just created and has buffer >= 1, so this never fails.
        out_tx
            .try_send(welcome)
            .expect("welcome fits in fresh buffer");

        self.bots.insert(
            bot_id.clone(),
            BotEntry {
                bot_id: bot_id.clone(),
                ship_id: ship_id.clone(),
                name: name.clone(),
                peer,
                outbound: out_tx,
                ready: false,
                pending_command: None,
                sensor_mode: SensorMode::Passive,
                last_fire_error_tick: None,
                command_ticks: VecDeque::new(),
                selected_powerups: Vec::new(),
            },
        );

        info!(
            room = %self.name,
            bot = %bot_id,
            ship = %ship_id,
            %peer,
            name = %name,
            version,
            "bot registered"
        );

        Ok(BotRegistration {
            bot_id,
            ship_id,
            outbound: out_rx,
        })
    }
}

/// Append `world_tick` to a bot's command-receipt window, trimming entries older than
/// `window` ticks. Used to derive the commands-per-second figure shown to spectators.
fn record_command_tick(history: &mut VecDeque<u64>, world_tick: u64, window: u64) {
    let cutoff = world_tick.saturating_sub(window.saturating_sub(1));
    while let Some(&front) = history.front() {
        if front < cutoff {
            history.pop_front();
        } else {
            break;
        }
    }
    history.push_back(world_tick);
}

/// Pick the combat events this bot should perceive this tick:
/// - `Hit` events on the bot's own ship are always reported.
/// - `Splash` events are reported when the splash centre falls within the bot's current
///   sensor range (active radar 350u, passive engine-noise threshold 150u).
/// - `Death` events are not surfaced to bots — the dead bot learns via `game_over`,
///   survivors learn by losing the contact (and ultimately via `game_over`).
fn filter_events_for_bot(
    own_ship: &ShipId,
    viewer_pos: Vec2,
    sensor_mode: SensorMode,
    config: &SimConfig,
    events: &[CombatEvent],
) -> Vec<TickEvent> {
    let splash_range = match sensor_mode {
        SensorMode::Active => config.active_radar_range,
        SensorMode::Passive => config.passive_hear_nearby_range,
    };
    let mut out = Vec::new();
    for event in events {
        match event {
            CombatEvent::Hit {
                ship_id, amount, ..
            } if ship_id == own_ship => {
                out.push(TickEvent::Hit { amount: *amount });
            }
            CombatEvent::Splash { pos } => {
                if pos.distance(viewer_pos) <= splash_range {
                    out.push(TickEvent::ShellSplash {
                        pos: [pos.x, pos.y],
                    });
                }
            }
            CombatEvent::Hit { .. } | CombatEvent::Death { .. } => {}
        }
    }
    out
}

/// Translate a sim-internal `Contact` to its on-the-wire `protocol::Contact`, assigning
/// a per-tick contact id of `c_<index>`.
fn translate_contact(index: usize, c: SimContact) -> ProtocolContact {
    ProtocolContact {
        id: format!("c_{index}"),
        kind: match c.kind {
            SimContactKind::Ship => ProtocolContactKind::Ship,
            SimContactKind::Shell => ProtocolContactKind::Shell,
            SimContactKind::Unknown => ProtocolContactKind::Unknown,
        },
        pos: [c.pos.x, c.pos.y],
        bearing_deg: c.bearing_deg,
        range: c.range,
        confidence: c.confidence,
    }
}

/// Compass bearing (0° = north / -y, 90° = east / +x) of the vector pointing from `from`
/// to `to`. Returns a value in `[0, 360)`.
fn compass_deg_facing(from: Vec2, to: Vec2) -> f32 {
    let v = to - from;
    let deg = v.x.atan2(-v.y).to_degrees();
    if deg < 0.0 {
        deg + 360.0
    } else {
        deg
    }
}

/// Default ring layout used by [`Room::start_match`]. Mirrors the historical hardcoded
/// placement: every bot evenly spaced on `STARTING_RING_RADIUS` around the map centre,
/// facing inward.
fn default_ring_layout(width: f32, height: f32, bot_count: usize) -> Vec<(Vec2, f32)> {
    let center = Vec2::new(width * 0.5, height * 0.5);
    let n = bot_count as f32;
    // BALANCE/DETERMINISM: bound the spawn ring to the map so ships start strictly inside
    // the walls even on small maps. `STARTING_RING_RADIUS` is the upper bound (unchanged on
    // 1000x1000 maps, where 0.4*1000 == 400); on smaller maps it shrinks so physics never
    // clamps a spawn onto a wall. Matches `monte_carlo::bounded_ring_radius`.
    let ring_radius = STARTING_RING_RADIUS.min(0.4 * width.min(height));
    (0..bot_count)
        .map(|i| {
            let angle = std::f32::consts::TAU * (i as f32) / n;
            let offset = Vec2::new(angle.cos(), angle.sin()) * ring_radius;
            let pos = center + offset;
            let heading = compass_deg_facing(pos, center);
            (pos, heading)
        })
        .collect()
}

/// Wall-clock Unix timestamp in seconds. Used outside the deterministic simulation —
/// purely for human-readable timestamps in the MC run id and status payload.
fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Build a short, filesystem-safe identifier for a Monte Carlo run.
fn make_mc_run_id(unix_secs_now: u64) -> String {
    format!("{:016x}", unix_secs_now)
}

/// Drive a room's tick loop until the shutdown channel fires. Consumes `RoomEvent`s
/// from `event_rx` between ticks; events are applied in arrival order.
///
/// Two pacing strategies, switched at runtime based on [`Room::in_lockstep`]:
///
/// - **Normal mode** (no MC run active): `tokio::time::interval` paces ticks at the
///   configured `tick_hz`. Commands that arrive between ticks queue per-bot and are
///   applied at the next tick boundary; bots that miss the `tick_deadline_ms` window get
///   a `late_command` error.
/// - **Lockstep mode** (an MC run is active and a match is running): the loop waits for
///   every registered bot to send a command for the current tick, then steps immediately.
///   A per-tick timeout (configurable via [`McConfig::per_tick_timeout_ms`]) backstops a
///   stalled bot. This is what makes a 100-match batch finish in a fraction of the time
///   the wall-clocked equivalent would take — the match goes as fast as the slowest bot
///   can respond, instead of being capped at the wall-clock tick rate.
pub async fn run_room(
    mut room: Room,
    mut event_rx: mpsc::Receiver<RoomEvent>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> u64 {
    let period = Duration::from_secs_f64(1.0 / f64::from(room.tick_hz.max(1)));
    let mut ticker = interval(period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let name = room.name.clone();
    info!(room = %name, tick_hz = room.tick_hz, "room started");

    // Deadline for the current lockstep tick, set when we first enter lockstep with no
    // commands queued; cleared every time we step. `None` outside lockstep mode.
    let mut lockstep_deadline: Option<tokio::time::Instant> = None;

    loop {
        // Recompute lockstep status each iteration — entering/exiting an MC run can
        // change it under us.
        let lockstep = room.in_lockstep();
        if lockstep {
            // First tick in a new lockstep window: arm the deadline. Subsequent
            // iterations preserve the existing deadline until we actually step.
            if lockstep_deadline.is_none() {
                lockstep_deadline = Some(tokio::time::Instant::now() + room.lockstep_timeout());
            }
            // If all bots have already sent commands, step immediately — no need to wait.
            if room.all_pending_commands_ready() {
                room.step_tick();
                lockstep_deadline = None;
                continue;
            }
        } else {
            lockstep_deadline = None;
        }

        let deadline_future = async {
            match lockstep_deadline {
                Some(d) => tokio::time::sleep_until(d).await,
                None => std::future::pending::<()>().await,
            }
        };

        tokio::select! {
            biased;
            _ = shutdown_rx.recv() => {
                info!(room = %name, final_tick = room.world.tick, "room: shutdown");
                break;
            }
            Some(event) = event_rx.recv() => {
                room.handle_event(event);
                // Most events touch room state in a way that may end the current
                // lockstep window (e.g. a kick reduces the roster). Re-check on the
                // next loop iteration; no need to short-circuit here.
            }
            _ = deadline_future, if lockstep && lockstep_deadline.is_some() => {
                // Per-tick timeout fired: step anyway with whatever commands we have.
                // Bots that didn't respond keep their previous throttle/rudder.
                debug!(
                    room = %name,
                    tick = room.world.tick,
                    "lockstep deadline fired; stepping with partial commands",
                );
                room.step_tick();
                lockstep_deadline = None;
            }
            _ = ticker.tick(), if !lockstep => {
                room.step_tick();
                debug!(room = %name, tick = room.world.tick, state = ?room.state, "tick");
            }
        }
    }
    room.world.tick
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::constants;
    use std::net::Ipv4Addr;

    fn test_peer() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, 12345))
    }

    fn test_room() -> Room {
        Room::new("test".into(), 1000.0, 1000.0, 42, 10, 80, 4)
    }

    fn connect(room: &mut Room, name: &str) -> Result<BotRegistration, JoinError> {
        let (tx, mut rx) = oneshot::channel();
        room.handle_event(RoomEvent::BotConnect {
            peer: test_peer(),
            name: name.into(),
            version: "1.0".into(),
            reply: tx,
        });
        rx.try_recv().expect("oneshot reply")
    }

    #[test]
    fn step_tick_in_lobby_does_not_advance_physics() {
        let mut room = test_room();
        // Ship-less Lobby still increments the tick counter (it's the wall-clock heartbeat),
        // but physics::step_world should not run. Verify by registering a bot and checking
        // its ship doesn't move despite a non-zero throttle.
        let _ = connect(&mut room, "kirk");
        let ship_id = "s_1";
        let original_pos = room.world.ships.get(ship_id).unwrap().pos;
        room.world.ships.get_mut(ship_id).unwrap().throttle = 1.0;
        for _ in 0..50 {
            room.step_tick();
        }
        let new_pos = room.world.ships.get(ship_id).unwrap().pos;
        assert_eq!(original_pos, new_pos, "ship moved during Lobby");
        assert_eq!(room.world.tick, 50, "tick counter still advances");
    }

    #[test]
    fn bot_connect_assigns_ids_and_spawns_ship() {
        let mut room = test_room();
        let mut reg = connect(&mut room, "alice").expect("registration");
        assert_eq!(reg.bot_id, "b_1");
        assert_eq!(reg.ship_id, "s_1");
        assert!(room.world.ships.contains_key("s_1"));
        assert_eq!(room.bot_count(), 1);

        // Welcome message was queued onto the outbound channel.
        let msg = reg.outbound.try_recv().expect("welcome queued");
        match msg {
            ServerMsg::Welcome {
                bot_id,
                ship_id,
                map,
                tick_hz,
                ..
            } => {
                assert_eq!(bot_id, "b_1");
                assert_eq!(ship_id, "s_1");
                assert_eq!(
                    map,
                    MapInfo {
                        width: 1000,
                        height: 1000
                    }
                );
                assert_eq!(tick_hz, 10);
            }
            other => panic!("expected Welcome, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_name_is_rejected() {
        let mut room = test_room();
        let _r1 = connect(&mut room, "alice").expect("first alice");
        let err = connect(&mut room, "alice").expect_err("duplicate should fail");
        assert_eq!(err, JoinError::DuplicateName);
        // The second registration must not have consumed a slot.
        assert_eq!(room.bot_count(), 1);
    }

    #[test]
    fn invalid_name_is_rejected() {
        let mut room = test_room();
        let err = connect(&mut room, "alice\n").expect_err("invalid name should fail");
        assert_eq!(err, JoinError::InvalidName);
        assert_eq!(room.bot_count(), 0);
    }

    #[test]
    fn stale_command_tick_is_rejected_with_error() {
        let mut room = test_room();
        let mut r = connect(&mut room, "a").expect("a");
        let _ = r.outbound.try_recv();
        room.handle_event(RoomEvent::BotReady {
            bot_id: r.bot_id.clone(),
        });
        start(&mut room, "test").expect("start");
        let _ = r.outbound.try_recv();
        // Advance well past tick 0 so a tick=0 command is far outside the ±1 window.
        for _ in 0..5 {
            room.step_tick();
            let _ = r.outbound.try_recv();
        }
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r.bot_id.clone(),
            command: cmd(0, 1.0, 0.0),
        });
        let msg = r.outbound.try_recv().expect("error frame");
        match msg {
            ServerMsg::Error { code, .. } => assert_eq!(code, error_code::STALE_COMMAND),
            other => panic!("expected stale_command error, got {other:?}"),
        }
        // Ship retains previous (zero) throttle since the stale command was discarded.
        let ship = room.world.ships.get(&r.ship_id).unwrap();
        assert_eq!(ship.throttle, 0.0);
    }

    #[test]
    fn duplicate_fire_errors_in_same_tick_are_coalesced() {
        let mut room = test_room();
        let mut r = connect(&mut room, "a").expect("a");
        let _ = r.outbound.try_recv();
        room.handle_event(RoomEvent::BotReady {
            bot_id: r.bot_id.clone(),
        });
        start(&mut room, "test").expect("start");
        let _ = r.outbound.try_recv();
        // Empty the magazine.
        room.world.ships.get_mut(&r.ship_id).unwrap().ammo = 0;
        for _ in 0..5 {
            room.send_fire_error(&r.bot_id, FireError::NoAmmo);
        }
        let mut error_count = 0;
        while let Ok(ServerMsg::Error { code, .. }) = r.outbound.try_recv() {
            if code == error_code::NO_AMMO {
                error_count += 1;
            }
        }
        assert_eq!(
            error_count, 1,
            "five fire errors in one tick should coalesce to a single frame"
        );
    }

    #[test]
    fn bot_ids_are_assigned_monotonically() {
        let mut room = test_room();
        let r1 = connect(&mut room, "a").expect("first");
        let r2 = connect(&mut room, "b").expect("second");
        assert_eq!(r1.bot_id, "b_1");
        assert_eq!(r2.bot_id, "b_2");
        assert_eq!(r1.ship_id, "s_1");
        assert_eq!(r2.ship_id, "s_2");
    }

    #[test]
    fn ready_flag_tracked_per_bot() {
        let mut room = test_room();
        let r1 = connect(&mut room, "a").expect("first");
        let r2 = connect(&mut room, "b").expect("second");
        assert!(!room.all_ready());

        room.handle_event(RoomEvent::BotReady {
            bot_id: r1.bot_id.clone(),
        });
        assert!(!room.all_ready(), "one ready, one not");

        room.handle_event(RoomEvent::BotReady {
            bot_id: r2.bot_id.clone(),
        });
        assert!(room.all_ready(), "all ready");
    }

    #[test]
    fn bot_disconnect_removes_ship() {
        let mut room = test_room();
        let r = connect(&mut room, "a").expect("registration");
        assert!(room.world.ships.contains_key(&r.ship_id));
        room.handle_event(RoomEvent::BotDisconnect {
            bot_id: r.bot_id.clone(),
        });
        assert!(!room.world.ships.contains_key(&r.ship_id));
        assert_eq!(room.bot_count(), 0);
    }

    #[test]
    fn cannot_join_after_room_starts() {
        let mut room = test_room();
        room.state = RoomState::Running;
        let err = connect(&mut room, "latecomer").expect_err("should refuse");
        assert_eq!(err, JoinError::NotInLobby);
    }

    #[test]
    fn rejects_join_when_full() {
        let mut room = Room::new("test".into(), 1000.0, 1000.0, 42, 10, 80, 2);
        connect(&mut room, "a").expect("first");
        connect(&mut room, "b").expect("second");
        let err = connect(&mut room, "c").expect_err("third should refuse");
        assert_eq!(err, JoinError::RoomFull);
    }

    fn start(room: &mut Room, name: &str) -> Result<(), StartError> {
        let (tx, mut rx) = oneshot::channel();
        room.handle_event(RoomEvent::OperatorStart {
            room: name.into(),
            reply: tx,
        });
        rx.try_recv().expect("oneshot reply")
    }

    #[test]
    fn operator_start_succeeds_when_all_ready() {
        let mut room = test_room();
        let mut r1 = connect(&mut room, "a").expect("a");
        let mut r2 = connect(&mut room, "b").expect("b");
        // Drain the welcome frames so the next item we pop is `game_start`.
        let _ = r1.outbound.try_recv().expect("welcome a");
        let _ = r2.outbound.try_recv().expect("welcome b");

        room.handle_event(RoomEvent::BotReady {
            bot_id: r1.bot_id.clone(),
        });
        room.handle_event(RoomEvent::BotReady {
            bot_id: r2.bot_id.clone(),
        });
        // Lobby tick counter advanced; verify it resets on start.
        for _ in 0..7 {
            room.step_tick();
        }
        assert_eq!(room.world.tick, 7);

        start(&mut room, "test").expect("start");
        assert_eq!(room.state, RoomState::Running);
        assert_eq!(room.world.tick, 0, "tick should reset on game_start");

        let g1 = r1.outbound.try_recv().expect("game_start a");
        let g2 = r2.outbound.try_recv().expect("game_start b");
        for (msg, ship_id) in [(g1, &r1.ship_id), (g2, &r2.ship_id)] {
            match msg {
                ServerMsg::GameStart {
                    tick,
                    starting_position,
                    starting_heading_deg,
                } => {
                    assert_eq!(tick, 0);
                    let ship = room.world.ships.get(ship_id).unwrap();
                    assert!((ship.pos.x - starting_position[0]).abs() < 1e-4);
                    assert!((ship.pos.y - starting_position[1]).abs() < 1e-4);
                    assert!((ship.heading_deg - starting_heading_deg).abs() < 1e-4);
                    let center = Vec2::new(500.0, 500.0);
                    let r = (ship.pos - center).length();
                    assert!(
                        (r - STARTING_RING_RADIUS).abs() < 1e-3,
                        "ship not on ring: r={r}"
                    );
                    // Heading points toward center: walking forward by `speed` should
                    // shrink the distance to center.
                    let dir = Vec2::new(
                        starting_heading_deg.to_radians().sin(),
                        -starting_heading_deg.to_radians().cos(),
                    );
                    let towards = center - ship.pos;
                    assert!(
                        dir.dot(towards.normalize()) > 0.999,
                        "heading not facing center: dot={}",
                        dir.dot(towards.normalize())
                    );
                }
                other => panic!("expected GameStart, got {other:?}"),
            }
        }
    }

    #[test]
    fn operator_start_rejects_unknown_room() {
        let mut room = test_room();
        let _ = connect(&mut room, "a").expect("a");
        let err = start(&mut room, "nonexistent").expect_err("should refuse");
        assert_eq!(err, StartError::UnknownRoom);
        assert_eq!(room.state, RoomState::Lobby);
    }

    #[test]
    fn operator_start_rejects_when_no_bots() {
        let mut room = test_room();
        let err = start(&mut room, "test").expect_err("should refuse");
        assert_eq!(err, StartError::NoBots);
    }

    #[test]
    fn operator_start_rejects_when_not_all_ready() {
        let mut room = test_room();
        let r1 = connect(&mut room, "a").expect("a");
        let _r2 = connect(&mut room, "b").expect("b");
        room.handle_event(RoomEvent::BotReady {
            bot_id: r1.bot_id.clone(),
        });
        let err = start(&mut room, "test").expect_err("should refuse");
        assert_eq!(err, StartError::NotAllReady);
        assert_eq!(room.state, RoomState::Lobby);
    }

    #[test]
    fn operator_start_rejects_when_already_running() {
        let mut room = test_room();
        let r = connect(&mut room, "a").expect("a");
        room.handle_event(RoomEvent::BotReady {
            bot_id: r.bot_id.clone(),
        });
        start(&mut room, "test").expect("first start");
        let err = start(&mut room, "test").expect_err("second start should refuse");
        assert_eq!(err, StartError::NotInLobby);
    }

    #[test]
    fn ships_advance_after_running_transition() {
        let mut room = test_room();
        let r = connect(&mut room, "a").expect("a");
        room.handle_event(RoomEvent::BotReady {
            bot_id: r.bot_id.clone(),
        });
        start(&mut room, "test").expect("start");

        // Push throttle and confirm physics now moves the ship.
        let ship = room.world.ships.get_mut(&r.ship_id).unwrap();
        let pos0 = ship.pos;
        ship.throttle = 1.0;
        for _ in 0..30 {
            room.step_tick();
        }
        let pos1 = room.world.ships.get(&r.ship_id).unwrap().pos;
        assert!(pos0 != pos1, "ship should have moved in Running state");
        assert!(room.world.tick > 0, "tick advances in Running");
    }

    fn cmd(tick: u64, throttle: f32, rudder: f32) -> PendingCommand {
        PendingCommand {
            tick,
            throttle,
            rudder,
            sensor_mode: SensorMode::Passive,
            fire: None,
            activate_powerup: None,
        }
    }

    #[test]
    fn step_tick_in_running_emits_tick_frames_with_self_state() {
        let mut room = test_room();
        let mut r = connect(&mut room, "a").expect("a");
        let _ = r.outbound.try_recv().expect("welcome");
        room.handle_event(RoomEvent::BotReady {
            bot_id: r.bot_id.clone(),
        });
        start(&mut room, "test").expect("start");
        let _ = r.outbound.try_recv().expect("game_start");

        room.step_tick();
        let msg = r.outbound.try_recv().expect("first tick frame");
        match msg {
            ServerMsg::Tick {
                tick,
                deadline_ms,
                self_state,
                contacts,
                events,
            } => {
                assert_eq!(tick, 1, "first tick after game_start");
                assert_eq!(deadline_ms, 80);
                assert!(contacts.is_empty(), "Phase 4.3 has empty contacts");
                assert!(events.is_empty(), "Phase 4.3 has empty events");
                let ship = room.world.ships.get(&r.ship_id).unwrap();
                assert!((self_state.pos[0] - ship.pos.x).abs() < 1e-4);
                assert!((self_state.pos[1] - ship.pos.y).abs() < 1e-4);
                assert_eq!(self_state.hp, ship.hp);
                assert_eq!(self_state.ammo, ship.ammo);
            }
            other => panic!("expected Tick, got {other:?}"),
        }
    }

    #[test]
    fn command_applies_throttle_and_rudder_on_next_tick() {
        let mut room = test_room();
        let mut r = connect(&mut room, "a").expect("a");
        let _ = r.outbound.try_recv();
        room.handle_event(RoomEvent::BotReady {
            bot_id: r.bot_id.clone(),
        });
        start(&mut room, "test").expect("start");
        let _ = r.outbound.try_recv();

        room.handle_event(RoomEvent::BotCommand {
            bot_id: r.bot_id.clone(),
            command: cmd(0, 1.0, 0.5),
        });
        room.step_tick();

        let ship = room.world.ships.get(&r.ship_id).unwrap();
        assert!((ship.throttle - 1.0).abs() < 1e-6);
        assert!((ship.rudder - 0.5).abs() < 1e-6);
        // Speed should have started accelerating: ACCELERATION * DT = 0.15 (one step toward 6.0).
        assert!(ship.speed > 0.0, "ship should have begun moving forward");
    }

    #[test]
    fn command_clamps_out_of_range_values() {
        let mut room = test_room();
        let mut r = connect(&mut room, "a").expect("a");
        let _ = r.outbound.try_recv();
        room.handle_event(RoomEvent::BotReady {
            bot_id: r.bot_id.clone(),
        });
        start(&mut room, "test").expect("start");
        let _ = r.outbound.try_recv();

        room.handle_event(RoomEvent::BotCommand {
            bot_id: r.bot_id.clone(),
            command: cmd(0, 5.0, -7.0),
        });
        room.step_tick();
        let ship = room.world.ships.get(&r.ship_id).unwrap();
        assert!((ship.throttle - 1.0).abs() < 1e-6);
        assert!((ship.rudder + 1.0).abs() < 1e-6);
    }

    #[test]
    fn missing_command_persists_previous_throttle_rudder() {
        let mut room = test_room();
        let mut r = connect(&mut room, "a").expect("a");
        let _ = r.outbound.try_recv();
        room.handle_event(RoomEvent::BotReady {
            bot_id: r.bot_id.clone(),
        });
        start(&mut room, "test").expect("start");
        let _ = r.outbound.try_recv();

        room.handle_event(RoomEvent::BotCommand {
            bot_id: r.bot_id.clone(),
            command: cmd(0, 0.7, 0.2),
        });
        room.step_tick();
        let throttle_after_first = room.world.ships.get(&r.ship_id).unwrap().throttle;
        let rudder_after_first = room.world.ships.get(&r.ship_id).unwrap().rudder;

        // No new command this tick → ship.throttle / .rudder must stay put.
        room.step_tick();
        let ship = room.world.ships.get(&r.ship_id).unwrap();
        assert!((ship.throttle - throttle_after_first).abs() < 1e-6);
        assert!((ship.rudder - rudder_after_first).abs() < 1e-6);
    }

    #[test]
    fn late_command_rejected_with_error_and_does_not_overwrite_controls() {
        let mut room = Room::new("test".into(), 1000.0, 1000.0, 42, 10, 5, 4);
        let mut r = connect(&mut room, "a").expect("a");
        let _ = r.outbound.try_recv();
        room.handle_event(RoomEvent::BotReady {
            bot_id: r.bot_id.clone(),
        });
        start(&mut room, "test").expect("start");
        let _ = r.outbound.try_recv();

        // Apply a real command first so the ship has non-zero controls.
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r.bot_id.clone(),
            command: cmd(0, 0.5, 0.3),
        });
        room.step_tick();
        let _ = r.outbound.try_recv(); // drop tick frame
        let throttle_before = room.world.ships.get(&r.ship_id).unwrap().throttle;
        let rudder_before = room.world.ships.get(&r.ship_id).unwrap().rudder;
        assert!((throttle_before - 0.5).abs() < 1e-6);

        // Force the deadline to expire. tick_deadline_ms = 5; sleep 30ms.
        std::thread::sleep(Duration::from_millis(30));

        // Send a late command. It must not change ship state and must produce an error.
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r.bot_id.clone(),
            command: cmd(1, -1.0, -1.0),
        });

        let err = r.outbound.try_recv().expect("late_command error queued");
        match err {
            ServerMsg::Error { code, .. } => {
                assert_eq!(code, error_code::LATE_COMMAND);
            }
            other => panic!("expected Error, got {other:?}"),
        }

        // Step a tick. Since pending_command is still None, the previous controls persist.
        room.step_tick();
        let ship = room.world.ships.get(&r.ship_id).unwrap();
        assert!(
            (ship.throttle - throttle_before).abs() < 1e-6,
            "throttle changed: was {throttle_before}, now {}",
            ship.throttle
        );
        assert!(
            (ship.rudder - rudder_before).abs() < 1e-6,
            "rudder changed: was {rudder_before}, now {}",
            ship.rudder
        );
    }

    #[test]
    fn command_within_deadline_applies_normally() {
        // tick_deadline_ms = 200 (generous); the command sent immediately after step_tick
        // should be applied on the next step.
        let mut room = Room::new("test".into(), 1000.0, 1000.0, 42, 10, 200, 4);
        let mut r = connect(&mut room, "a").expect("a");
        let _ = r.outbound.try_recv();
        room.handle_event(RoomEvent::BotReady {
            bot_id: r.bot_id.clone(),
        });
        start(&mut room, "test").expect("start");
        let _ = r.outbound.try_recv();

        room.step_tick();
        let _ = r.outbound.try_recv();
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r.bot_id.clone(),
            command: cmd(1, 0.4, 0.0),
        });
        // No error queued.
        assert!(r.outbound.try_recv().is_err(), "should be no error frame");
        room.step_tick();
        let ship = room.world.ships.get(&r.ship_id).unwrap();
        assert!((ship.throttle - 0.4).abs() < 1e-6);
    }

    #[test]
    fn command_outside_running_is_ignored_by_step() {
        let mut room = test_room();
        let mut r = connect(&mut room, "a").expect("a");
        let _ = r.outbound.try_recv();
        // Queue a command while still in Lobby.
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r.bot_id.clone(),
            command: cmd(0, 1.0, 0.0),
        });
        room.step_tick();
        let ship = room.world.ships.get(&r.ship_id).unwrap();
        assert_eq!(
            ship.throttle, 0.0,
            "Lobby step_tick must not apply commands"
        );
        // And the pending command is cleared once `start_match` runs.
        room.handle_event(RoomEvent::BotReady {
            bot_id: r.bot_id.clone(),
        });
        start(&mut room, "test").expect("start");
        let _ = r.outbound.try_recv();
        room.step_tick();
        let ship = room.world.ships.get(&r.ship_id).unwrap();
        assert_eq!(ship.throttle, 0.0, "stale Lobby command should not apply");
    }

    fn cmd_with_mode(tick: u64, throttle: f32, rudder: f32, mode: SensorMode) -> PendingCommand {
        PendingCommand {
            tick,
            throttle,
            rudder,
            sensor_mode: mode,
            fire: None,
            activate_powerup: None,
        }
    }

    /// Run the standard "two bots, ready, started" prelude and return their
    /// `BotRegistration`s. Welcome and `game_start` frames are drained.
    fn started_two_bot_room() -> (Room, BotRegistration, BotRegistration) {
        let mut room = test_room();
        let mut r1 = connect(&mut room, "a").expect("a");
        let mut r2 = connect(&mut room, "b").expect("b");
        let _ = r1.outbound.try_recv();
        let _ = r2.outbound.try_recv();
        room.handle_event(RoomEvent::BotReady {
            bot_id: r1.bot_id.clone(),
        });
        room.handle_event(RoomEvent::BotReady {
            bot_id: r2.bot_id.clone(),
        });
        start(&mut room, "test").expect("start");
        let _ = r1.outbound.try_recv();
        let _ = r2.outbound.try_recv();
        (room, r1, r2)
    }

    fn next_tick_contacts(reg: &mut BotRegistration) -> Vec<ProtocolContact> {
        match reg.outbound.try_recv().expect("tick frame") {
            ServerMsg::Tick { contacts, .. } => contacts,
            other => panic!("expected Tick, got {other:?}"),
        }
    }

    #[test]
    fn active_bot_gets_ranged_contact_for_in_range_ship() {
        let (mut room, mut r1, mut r2) = started_two_bot_room();

        // Reposition: 100u apart so the active radar (350u) sees s_2 and the passive
        // listener also hears s_1 via the 150u nearby rule.
        room.world.ships.get_mut(&r1.ship_id).unwrap().pos = Vec2::new(500.0, 500.0);
        room.world.ships.get_mut(&r2.ship_id).unwrap().pos = Vec2::new(600.0, 500.0);

        room.handle_event(RoomEvent::BotCommand {
            bot_id: r1.bot_id.clone(),
            command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Active),
        });
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r2.bot_id.clone(),
            command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Passive),
        });
        room.step_tick();

        let active_contacts = next_tick_contacts(&mut r1);
        assert_eq!(active_contacts.len(), 1, "active sees one ship");
        let c = &active_contacts[0];
        assert_eq!(c.id, "c_0");
        assert_eq!(c.kind, ProtocolContactKind::Ship);
        let r = c.range.expect("active range");
        assert!((r - 100.0).abs() < 1.0, "range was {r}");

        let passive_contacts = next_tick_contacts(&mut r2);
        assert_eq!(
            passive_contacts.len(),
            1,
            "100u within 150u nearby threshold"
        );
        assert!(
            passive_contacts[0].range.is_none(),
            "passive must not report range"
        );
    }

    #[test]
    fn passive_hears_pinger_with_one_tick_delay_at_300_units() {
        let (mut room, mut r1, mut r2) = started_two_bot_room();

        // 300u apart: out of nearby (150) but within active-listening (500) and active
        // radar (350) ranges.
        room.world.ships.get_mut(&r1.ship_id).unwrap().pos = Vec2::new(400.0, 500.0);
        room.world.ships.get_mut(&r2.ship_id).unwrap().pos = Vec2::new(700.0, 500.0);

        // Tick 1: r1 commands Active, r2 commands Passive. previous_active_pingers is
        // empty (cleared at start_match), so the passive listener should NOT yet hear.
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r1.bot_id.clone(),
            command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Active),
        });
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r2.bot_id.clone(),
            command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Passive),
        });
        room.step_tick();

        let active_contacts = next_tick_contacts(&mut r1);
        assert_eq!(active_contacts.len(), 1, "active sees s_2 at 300u");

        let passive_contacts = next_tick_contacts(&mut r2);
        assert!(
            passive_contacts.is_empty(),
            "passive must not yet hear (one-tick delay): {passive_contacts:?}"
        );

        // Tick 2: send the same commands. Now previous_active_pingers contains s_1, so
        // the passive listener picks it up.
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r1.bot_id.clone(),
            command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Active),
        });
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r2.bot_id.clone(),
            command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Passive),
        });
        room.step_tick();

        let _ = next_tick_contacts(&mut r1);
        let passive_contacts = next_tick_contacts(&mut r2);
        assert_eq!(
            passive_contacts.len(),
            1,
            "passive should now hear the pinger"
        );
        assert!(passive_contacts[0].range.is_none());
        // Bearing from r2 (east of s_1) toward s_1 is west (~270°), within ±5°.
        let b = passive_contacts[0].bearing_deg;
        let dev = (b - 270.0).abs().min((b - 270.0 + 360.0).abs());
        assert!(dev < 5.0 + 1e-3, "bearing {b} too far from 270°");
    }

    #[test]
    fn passive_does_not_hear_silent_distant_ship() {
        let (mut room, mut r1, mut r2) = started_two_bot_room();

        // 300u apart, both passive — nobody pings, so the 500u "hear actives" rule
        // doesn't fire and 300u > 150u nearby threshold.
        room.world.ships.get_mut(&r1.ship_id).unwrap().pos = Vec2::new(400.0, 500.0);
        room.world.ships.get_mut(&r2.ship_id).unwrap().pos = Vec2::new(700.0, 500.0);

        for _ in 0..3 {
            room.handle_event(RoomEvent::BotCommand {
                bot_id: r1.bot_id.clone(),
                command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Passive),
            });
            room.handle_event(RoomEvent::BotCommand {
                bot_id: r2.bot_id.clone(),
                command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Passive),
            });
            room.step_tick();
            assert!(next_tick_contacts(&mut r1).is_empty());
            assert!(next_tick_contacts(&mut r2).is_empty());
        }
    }

    #[test]
    fn active_bot_does_not_see_target_beyond_350_units() {
        let (mut room, mut r1, r2) = started_two_bot_room();
        // 400u apart > active radar range (350).
        room.world.ships.get_mut(&r1.ship_id).unwrap().pos = Vec2::new(300.0, 500.0);
        room.world.ships.get_mut(&r2.ship_id).unwrap().pos = Vec2::new(700.0, 500.0);

        room.handle_event(RoomEvent::BotCommand {
            bot_id: r1.bot_id.clone(),
            command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Active),
        });
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r2.bot_id.clone(),
            command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Passive),
        });
        room.step_tick();

        assert!(next_tick_contacts(&mut r1).is_empty(), "400u > 350u radar");
    }

    // ----- Phase 6 (combat) ------------------------------------------------

    fn cmd_fire(tick: u64, throttle: f32, fire: FireCommand, mode: SensorMode) -> PendingCommand {
        PendingCommand {
            tick,
            throttle,
            rudder: 0.0,
            sensor_mode: mode,
            fire: Some(fire),
            activate_powerup: None,
        }
    }

    /// Drain every queued `ServerMsg` from a bot's outbound channel. Useful when a test
    /// only cares about the *most recent* message kind (e.g. game_over after a hail of
    /// ticks).
    fn drain(reg: &mut BotRegistration) -> Vec<ServerMsg> {
        let mut out = Vec::new();
        while let Ok(m) = reg.outbound.try_recv() {
            out.push(m);
        }
        out
    }

    fn last_of<F>(msgs: &[ServerMsg], f: F) -> Option<&ServerMsg>
    where
        F: Fn(&ServerMsg) -> bool,
    {
        msgs.iter().rev().find(|m| f(m))
    }

    #[test]
    fn fire_command_spawns_shell_in_world() {
        let (mut room, r1, _r2) = started_two_bot_room();
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r1.bot_id.clone(),
            command: cmd_fire(
                room.world.tick,
                0.0,
                FireCommand {
                    bearing_deg: 90.0,
                    range: 200.0,
                },
                SensorMode::Passive,
            ),
        });
        room.step_tick();
        assert_eq!(room.world.shells.len(), 1, "shell should have spawned");
        // Cooldown decremented one tick by physics::step_world after fire.
        let firer = room.world.ships.get(&r1.ship_id).unwrap();
        assert_eq!(firer.gun_cooldown, constants::GUN_COOLDOWN_TICKS - 1);
        assert_eq!(firer.ammo, constants::MAX_AMMO - 1);
    }

    #[test]
    fn fire_during_cooldown_yields_cooldown_active_error() {
        let (mut room, mut r1, _r2) = started_two_bot_room();
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r1.bot_id.clone(),
            command: cmd_fire(
                room.world.tick,
                0.0,
                FireCommand {
                    bearing_deg: 0.0,
                    range: 100.0,
                },
                SensorMode::Passive,
            ),
        });
        room.step_tick();
        let _ = drain(&mut r1); // tick frame for the first shot

        // Try to fire again while still on cooldown.
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r1.bot_id.clone(),
            command: cmd_fire(
                room.world.tick,
                0.0,
                FireCommand {
                    bearing_deg: 0.0,
                    range: 100.0,
                },
                SensorMode::Passive,
            ),
        });
        room.step_tick();
        let msgs = drain(&mut r1);
        let err = msgs.iter().find(
            |m| matches!(m, ServerMsg::Error { code, .. } if code == error_code::COOLDOWN_ACTIVE),
        );
        assert!(err.is_some(), "cooldown_active error missing: {msgs:?}");
        // Still only one shell in world.
        assert_eq!(room.world.shells.len(), 1);
    }

    #[test]
    fn match_ends_with_winner_when_only_one_ship_alive() {
        // Acceptance check from projectplan §6.3.
        let (mut room, mut r1, mut r2) = started_two_bot_room();
        // Position close enough that the splash kills one ship outright.
        room.world.ships.get_mut(&r1.ship_id).unwrap().pos = Vec2::new(500.0, 500.0);
        room.world.ships.get_mut(&r2.ship_id).unwrap().pos = Vec2::new(700.0, 500.0);
        // Pre-damage s_2 to 1 HP so a single splash finishes it.
        room.world.ships.get_mut(&r2.ship_id).unwrap().hp = 1;

        // r1 fires a 200-unit shot east, range 200 → splash 25 dmg lands on s_2.
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r1.bot_id.clone(),
            command: cmd_fire(
                room.world.tick,
                0.0,
                FireCommand {
                    bearing_deg: 90.0,
                    range: 200.0,
                },
                SensorMode::Passive,
            ),
        });
        // Both bots park on idle commands while the shell flies. Drain outbound queues
        // each step so the per-bot channel (32 deep) doesn't overflow before game_over.
        let mut m1: Vec<ServerMsg> = Vec::new();
        let mut m2: Vec<ServerMsg> = Vec::new();
        for _ in 0..40 {
            room.handle_event(RoomEvent::BotCommand {
                bot_id: r2.bot_id.clone(),
                command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Passive),
            });
            room.step_tick();
            m1.extend(drain(&mut r1));
            m2.extend(drain(&mut r2));
            if room.state == RoomState::Ended {
                break;
            }
        }
        assert_eq!(room.state, RoomState::Ended, "match should have ended");
        let g1 = last_of(&m1, |m| matches!(m, ServerMsg::GameOver { .. }))
            .expect("r1 should receive game_over");
        let g2 = last_of(&m2, |m| matches!(m, ServerMsg::GameOver { .. }))
            .expect("r2 should receive game_over");
        for msg in [g1, g2] {
            match msg {
                ServerMsg::GameOver { winner, .. } => {
                    assert_eq!(
                        winner.as_deref(),
                        Some(r1.bot_id.as_str()),
                        "winner should be r1 (only survivor)"
                    );
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn match_report_captures_combat_stats() {
        let (mut room, mut r1, mut r2) = started_two_bot_room();
        // No report exists until a match finishes.
        assert!(room.last_report.is_none(), "no report before match ends");

        room.world.ships.get_mut(&r1.ship_id).unwrap().pos = Vec2::new(500.0, 500.0);
        room.world.ships.get_mut(&r2.ship_id).unwrap().pos = Vec2::new(700.0, 500.0);
        room.world.ships.get_mut(&r2.ship_id).unwrap().hp = 1;

        // r1 fires one shot that kills r2.
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r1.bot_id.clone(),
            command: cmd_fire(
                room.world.tick,
                0.0,
                FireCommand {
                    bearing_deg: 90.0,
                    range: 200.0,
                },
                SensorMode::Passive,
            ),
        });
        for _ in 0..40 {
            room.handle_event(RoomEvent::BotCommand {
                bot_id: r2.bot_id.clone(),
                command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Passive),
            });
            room.step_tick();
            let _ = drain(&mut r1);
            let _ = drain(&mut r2);
            if room.state == RoomState::Ended {
                break;
            }
        }
        assert_eq!(room.state, RoomState::Ended);

        let report = room.last_report.clone().expect("report after match end");
        assert_eq!(report.outcome, "winner");
        assert_eq!(report.winner.as_deref(), Some(r1.bot_id.as_str()));
        assert_eq!(report.winner_name.as_deref(), Some("a"));
        assert!(report.duration_ticks > 0);
        assert_eq!(report.bots.len(), 2);

        let r1_row = report
            .bots
            .iter()
            .find(|b| b.bot_id == r1.bot_id)
            .expect("r1 row");
        assert_eq!(r1_row.shots_fired, 1);
        assert_eq!(r1_row.hits_landed, 1);
        assert_eq!(r1_row.kills, 1);
        assert!(r1_row.damage_dealt > 0);
        assert!((r1_row.accuracy - 1.0).abs() < 1e-4);
        assert!(r1_row.survived);

        let r2_row = report
            .bots
            .iter()
            .find(|b| b.bot_id == r2.bot_id)
            .expect("r2 row");
        assert_eq!(r2_row.shots_fired, 0);
        assert_eq!(r2_row.accuracy, 0.0);
        assert!(r2_row.damage_taken > 0);
        assert!(!r2_row.survived);
        assert_eq!(r2_row.final_hp, 0);

        // The report is queryable via the event channel and survives into Lobby.
        let (tx, mut rx) = oneshot::channel();
        room.handle_event(RoomEvent::QueryReport { reply: tx });
        assert!(rx.try_recv().expect("reply").is_some());
    }

    #[test]
    fn aborted_match_reports_aborted_outcome() {
        let (mut room, _r1, _r2) = started_two_bot_room();
        room.step_tick();
        let (tx, mut rx) = oneshot::channel();
        room.handle_event(RoomEvent::OperatorAbort { reply: tx });
        rx.try_recv().expect("reply").expect("abort ok");
        let report = room.last_report.clone().expect("report after abort");
        assert_eq!(report.outcome, "aborted");
        assert!(report.winner.is_none());
    }

    #[test]
    fn timeout_picks_highest_hp_winner() {
        let (mut room, mut r1, mut r2) = started_two_bot_room();
        // Both alive, no shots fired. Force tick past timeout, then step to trigger end.
        room.world.ships.get_mut(&r1.ship_id).unwrap().hp = 80;
        room.world.ships.get_mut(&r2.ship_id).unwrap().hp = 50;
        room.world.tick = MATCH_TIMEOUT_TICKS - 1;
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r1.bot_id.clone(),
            command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Passive),
        });
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r2.bot_id.clone(),
            command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Passive),
        });
        room.step_tick();
        assert_eq!(room.state, RoomState::Ended);
        let msgs = drain(&mut r1);
        let g = last_of(&msgs, |m| matches!(m, ServerMsg::GameOver { .. })).unwrap();
        match g {
            ServerMsg::GameOver { winner, .. } => {
                assert_eq!(winner.as_deref(), Some(r1.bot_id.as_str()));
            }
            _ => unreachable!(),
        }
        let _ = drain(&mut r2);
    }

    #[test]
    fn timeout_tiebreaks_by_remaining_ammo() {
        let (mut room, mut r1, _r2) = started_two_bot_room();
        // Equal HP; r1 has more ammo → r1 wins on tie-break.
        room.world.ships.get_mut(&r1.ship_id).unwrap().hp = 40;
        room.world.ships.get_mut(&r1.ship_id).unwrap().ammo = 12;
        room.world.ships.get_mut("s_2").unwrap().hp = 40;
        room.world.ships.get_mut("s_2").unwrap().ammo = 5;
        room.world.tick = MATCH_TIMEOUT_TICKS - 1;
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r1.bot_id.clone(),
            command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Passive),
        });
        room.handle_event(RoomEvent::BotCommand {
            bot_id: "b_2".to_string(),
            command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Passive),
        });
        room.step_tick();
        let msgs = drain(&mut r1);
        let g = last_of(&msgs, |m| matches!(m, ServerMsg::GameOver { .. })).unwrap();
        match g {
            ServerMsg::GameOver { winner, .. } => {
                assert_eq!(winner.as_deref(), Some(r1.bot_id.as_str()));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn draw_when_no_ships_alive() {
        let (mut room, mut r1, mut r2) = started_two_bot_room();
        // Kill both ships in the same tick: pre-damage to 1, fire a shot from each that
        // lands centred on the other's position with friendly fire on.
        room.world.ships.get_mut(&r1.ship_id).unwrap().pos = Vec2::new(400.0, 500.0);
        room.world.ships.get_mut(&r2.ship_id).unwrap().pos = Vec2::new(500.0, 500.0);
        // Start them at the brink so any nonzero hit kills them.
        room.world.ships.get_mut(&r1.ship_id).unwrap().hp = 1;
        room.world.ships.get_mut(&r2.ship_id).unwrap().hp = 1;
        // Spawn a shell at each ship's position with TTL=1 so it explodes next tick.
        let cfg = room.world.config;
        room.world.shells.push(crate::sim::world::Shell {
            id_index: 0,
            source_ship: r2.ship_id.clone(),
            pos: Vec2::new(400.0, 500.0),
            vel: Vec2::ZERO,
            ttl_ticks: 1,
            splash_radius: cfg.splash_radius,
            max_splash_damage: cfg.max_splash_damage,
        });
        room.world.shells.push(crate::sim::world::Shell {
            id_index: 1,
            source_ship: r1.ship_id.clone(),
            pos: Vec2::new(500.0, 500.0),
            vel: Vec2::ZERO,
            ttl_ticks: 1,
            splash_radius: cfg.splash_radius,
            max_splash_damage: cfg.max_splash_damage,
        });
        room.world.next_shell_index = 2;
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r1.bot_id.clone(),
            command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Passive),
        });
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r2.bot_id.clone(),
            command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Passive),
        });
        room.step_tick();
        assert_eq!(room.state, RoomState::Ended);
        for reg in [&mut r1, &mut r2] {
            let msgs = drain(reg);
            let g = last_of(&msgs, |m| matches!(m, ServerMsg::GameOver { .. })).unwrap();
            match g {
                ServerMsg::GameOver { winner, .. } => {
                    assert!(winner.is_none(), "should be a draw, got {winner:?}");
                }
                _ => unreachable!(),
            }
        }
    }

    /// Phase 7.2 acceptance: when a spectator broadcast channel is wired, every
    /// `step_tick` publishes a JSON `world` frame with the current ground-truth state,
    /// including each ship's last-commanded `sensor_mode` (Phase 7.4).
    #[test]
    fn spectator_broadcast_emits_world_frames_each_tick() {
        let mut room = test_room();
        let (spec_tx, mut spec_rx) = broadcast::channel::<SpectatorFrame>(8);
        room.set_spectator_broadcast(spec_tx);

        let mut r = connect(&mut room, "alice").expect("registration");
        let _ = r.outbound.try_recv();
        room.handle_event(RoomEvent::BotReady {
            bot_id: r.bot_id.clone(),
        });
        start(&mut room, "test").expect("start");
        let _ = r.outbound.try_recv();

        // Drain anything published during start_match (none — start runs outside step_tick).
        while spec_rx.try_recv().is_ok() {}

        // Active sensor command so the world payload reports `sensor_mode: "active"`.
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r.bot_id.clone(),
            command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Active),
        });
        room.step_tick();

        let frame = spec_rx.try_recv().expect("spectator frame published");
        let parsed: serde_json::Value = serde_json::from_str(&frame).expect("frame is valid JSON");
        assert_eq!(parsed["type"], "world");
        assert_eq!(parsed["tick"], 1);
        let ships = parsed["ships"].as_array().expect("ships array");
        assert_eq!(ships.len(), 1);
        assert_eq!(ships[0]["id"], r.ship_id);
        assert_eq!(ships[0]["bot_name"], "alice");
        assert_eq!(ships[0]["alive"], true);
        assert_eq!(ships[0]["sensor_mode"], "active");
        // New observability fields land in every frame.
        assert!(ships[0]["speed"].is_number(), "speed missing from frame");
        assert!(ships[0]["ammo"].is_number(), "ammo missing from frame");
        assert!(
            ships[0]["throttle"].is_number(),
            "throttle missing from frame"
        );
        assert!(ships[0]["rudder"].is_number(), "rudder missing from frame");
        assert_eq!(ships[0]["ready"], true);
        // We accepted one command this second, so cps is at least 1.
        assert!(
            ships[0]["commands_per_sec"].as_f64().unwrap() >= 1.0,
            "expected commands_per_sec ≥ 1 after one accepted command, got {}",
            ships[0]["commands_per_sec"],
        );
    }

    #[test]
    fn hit_event_appears_in_victims_tick_payload() {
        // Acceptance check from projectplan §6.4.
        let (mut room, _r1, mut r2) = started_two_bot_room();
        room.world.ships.get_mut("s_1").unwrap().pos = Vec2::new(500.0, 500.0);
        room.world.ships.get_mut(&r2.ship_id).unwrap().pos = Vec2::new(700.0, 500.0);
        room.handle_event(RoomEvent::BotCommand {
            bot_id: "b_1".to_string(),
            command: cmd_fire(
                room.world.tick,
                0.0,
                FireCommand {
                    bearing_deg: 90.0,
                    range: 200.0,
                },
                SensorMode::Passive,
            ),
        });
        // Run the shell out, with r2 idling.
        let mut hit_event_seen = false;
        for _ in 0..45 {
            room.handle_event(RoomEvent::BotCommand {
                bot_id: r2.bot_id.clone(),
                command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Passive),
            });
            room.step_tick();
            // Pull off the latest tick frame and check.
            while let Ok(msg) = r2.outbound.try_recv() {
                if let ServerMsg::Tick { events, .. } = msg {
                    if events.iter().any(|e| matches!(e, TickEvent::Hit { .. })) {
                        hit_event_seen = true;
                    }
                }
            }
            if hit_event_seen {
                break;
            }
        }
        assert!(hit_event_seen, "victim never received a Hit event");
    }

    #[test]
    fn splash_event_visible_to_active_radar_within_350_units() {
        let (mut room, _r1, mut r2) = started_two_bot_room();
        // Place a far-away splash relative to r2 (active mode at the time).
        room.world.ships.get_mut("s_1").unwrap().pos = Vec2::new(100.0, 500.0);
        room.world.ships.get_mut(&r2.ship_id).unwrap().pos = Vec2::new(500.0, 500.0);
        // r1 fires a tiny-range shot east → shell explodes ~5u east of itself, ~395u
        // from r2 — outside r2's 350u active radar. Should NOT be reported.
        room.handle_event(RoomEvent::BotCommand {
            bot_id: "b_1".to_string(),
            command: cmd_fire(
                room.world.tick,
                0.0,
                FireCommand {
                    bearing_deg: 90.0,
                    range: 5.0,
                },
                SensorMode::Passive,
            ),
        });
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r2.bot_id.clone(),
            command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Active),
        });
        room.step_tick();
        let msgs = drain(&mut r2);
        let tick = msgs
            .iter()
            .rev()
            .find_map(|m| match m {
                ServerMsg::Tick { events, .. } => Some(events.clone()),
                _ => None,
            })
            .expect("r2 should receive a tick frame");
        assert!(
            tick.iter()
                .all(|e| !matches!(e, TickEvent::ShellSplash { .. })),
            "distant splash leaked into r2's events: {tick:?}"
        );

        // Now place r2 within 350u of the splash and re-fire: should be visible.
        room.world.ships.get_mut("s_1").unwrap().pos = Vec2::new(400.0, 500.0);
        room.world.ships.get_mut(&r2.ship_id).unwrap().pos = Vec2::new(500.0, 500.0);
        // Wait out cooldown.
        for _ in 0..constants::GUN_COOLDOWN_TICKS as usize {
            room.handle_event(RoomEvent::BotCommand {
                bot_id: r2.bot_id.clone(),
                command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Active),
            });
            room.step_tick();
            let _ = drain(&mut r2);
        }
        room.handle_event(RoomEvent::BotCommand {
            bot_id: "b_1".to_string(),
            command: cmd_fire(
                room.world.tick,
                0.0,
                FireCommand {
                    bearing_deg: 90.0,
                    range: 5.0,
                },
                SensorMode::Passive,
            ),
        });
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r2.bot_id.clone(),
            command: cmd_with_mode(room.world.tick, 0.0, 0.0, SensorMode::Active),
        });
        room.step_tick();
        let msgs = drain(&mut r2);
        let tick = msgs
            .iter()
            .rev()
            .find_map(|m| match m {
                ServerMsg::Tick { events, .. } => Some(events.clone()),
                _ => None,
            })
            .expect("r2 should receive a tick frame");
        assert!(
            tick.iter()
                .any(|e| matches!(e, TickEvent::ShellSplash { .. })),
            "near splash should be reported: {tick:?}"
        );
    }

    #[test]
    fn compass_facing_returns_normalized_bearing() {
        // Place point south of center → heading 0° (north).
        let p = Vec2::new(500.0, 600.0);
        let c = Vec2::new(500.0, 500.0);
        assert!((compass_deg_facing(p, c) - 0.0).abs() < 1e-4);

        // West of center → heading 90° (east).
        let p = Vec2::new(400.0, 500.0);
        assert!((compass_deg_facing(p, c) - 90.0).abs() < 1e-4);

        // North of center → heading 180° (south).
        let p = Vec2::new(500.0, 400.0);
        assert!((compass_deg_facing(p, c) - 180.0).abs() < 1e-4);

        // East of center → heading 270° (west).
        let p = Vec2::new(600.0, 500.0);
        assert!((compass_deg_facing(p, c) - 270.0).abs() < 1e-4);
    }

    // -----------------------------------------------------------------------
    // Lifecycle tests: abort / reset / kick / post-game-pause / multi-round
    // -----------------------------------------------------------------------

    fn abort(room: &mut Room) -> Result<(), AbortError> {
        let (tx, mut rx) = oneshot::channel();
        room.handle_event(RoomEvent::OperatorAbort { reply: tx });
        rx.try_recv().expect("oneshot reply")
    }

    fn reset(room: &mut Room) -> Result<(), ResetError> {
        let (tx, mut rx) = oneshot::channel();
        room.handle_event(RoomEvent::OperatorReset { reply: tx });
        rx.try_recv().expect("oneshot reply")
    }

    fn kick(room: &mut Room, bot_id: &str) -> Result<(), KickError> {
        let (tx, mut rx) = oneshot::channel();
        room.handle_event(RoomEvent::OperatorKick {
            bot_id: bot_id.to_string(),
            reply: tx,
        });
        rx.try_recv().expect("oneshot reply")
    }

    /// Drain any frames in a bot's outbound channel into a Vec for assertions.
    fn drain_msgs(reg: &mut BotRegistration) -> Vec<ServerMsg> {
        let mut out = Vec::new();
        while let Ok(m) = reg.outbound.try_recv() {
            out.push(m);
        }
        out
    }

    #[test]
    fn abort_running_match_marks_ended_and_broadcasts_no_winner() {
        let mut room = test_room();
        let mut r1 = connect(&mut room, "a").expect("a");
        let mut r2 = connect(&mut room, "b").expect("b");
        for r in [&mut r1, &mut r2] {
            room.handle_event(RoomEvent::BotReady {
                bot_id: r.bot_id.clone(),
            });
        }
        start(&mut room, "test").expect("start");
        // Drain handshake frames so the next pop is the game_over.
        let _ = drain_msgs(&mut r1);
        let _ = drain_msgs(&mut r2);

        abort(&mut room).expect("abort");
        assert_eq!(room.state, RoomState::Ended);
        assert!(room.end_tick.is_some(), "abort sets end_tick");
        for r in [&mut r1, &mut r2] {
            let msgs = drain_msgs(r);
            let game_over = msgs
                .iter()
                .rev()
                .find(|m| matches!(m, ServerMsg::GameOver { .. }));
            match game_over.expect("game_over after abort") {
                ServerMsg::GameOver { winner, .. } => {
                    assert_eq!(*winner, None, "abort produces no winner");
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn abort_in_lobby_is_rejected() {
        let mut room = test_room();
        let _ = connect(&mut room, "a").expect("a");
        let err = abort(&mut room).expect_err("should refuse");
        assert_eq!(err, AbortError::NotRunning);
        assert_eq!(room.state, RoomState::Lobby);
    }

    #[test]
    fn reset_in_running_is_rejected() {
        let mut room = test_room();
        let r = connect(&mut room, "a").expect("a");
        room.handle_event(RoomEvent::BotReady {
            bot_id: r.bot_id.clone(),
        });
        start(&mut room, "test").expect("start");
        let err = reset(&mut room).expect_err("should refuse");
        assert_eq!(err, ResetError::NotEnded);
        assert_eq!(room.state, RoomState::Running);
    }

    #[test]
    fn auto_lobby_transition_after_post_game_ticks() {
        let mut room = test_room();
        let mut r1 = connect(&mut room, "a").expect("a");
        let mut r2 = connect(&mut room, "b").expect("b");
        for r in [&mut r1, &mut r2] {
            room.handle_event(RoomEvent::BotReady {
                bot_id: r.bot_id.clone(),
            });
        }
        start(&mut room, "test").expect("start");
        let _ = drain_msgs(&mut r1);
        let _ = drain_msgs(&mut r2);

        // Force the match into Ended by killing one ship.
        room.world.ships.get_mut(&r2.ship_id).unwrap().alive = false;
        room.step_tick();
        assert_eq!(room.state, RoomState::Ended);

        // Run enough ticks for the post-game pause to elapse plus a buffer.
        for _ in 0..(POST_GAME_LOBBY_TICKS + 5) {
            room.step_tick();
        }
        assert_eq!(room.state, RoomState::Lobby, "auto-transition fires");
        assert_eq!(
            room.world.tick, 5,
            "tick reset to 0 then advanced ~5 ticks in lobby"
        );
        assert!(
            room.world.shells.is_empty(),
            "shells cleared on lobby transition"
        );

        // Every bot's ship is healed and respawned at center; ready flags cleared.
        for entry in room.bots.values() {
            assert!(!entry.ready, "ready flag cleared after lobby transition");
            let ship = room.world.ships.get(&entry.ship_id).expect("ship present");
            assert_eq!(ship.hp, crate::sim::constants::HULL_HP);
            assert_eq!(ship.ammo, crate::sim::constants::MAX_AMMO);
            assert!(ship.alive);
        }

        // Every bot received exactly one `lobby` frame.
        for r in [&mut r1, &mut r2] {
            let msgs = drain_msgs(r);
            let lobby_count = msgs
                .iter()
                .filter(|m| matches!(m, ServerMsg::Lobby { .. }))
                .count();
            assert_eq!(lobby_count, 1, "exactly one lobby frame per bot");
        }
    }

    #[test]
    fn bot_id_and_ship_id_stable_across_rounds() {
        let mut room = test_room();
        let mut r1 = connect(&mut room, "a").expect("a");
        let mut r2 = connect(&mut room, "b").expect("b");
        let bot_ids_before: Vec<BotId> = room.bots.keys().cloned().collect();
        let ship_ids_before: Vec<ShipId> = room.bots.values().map(|e| e.ship_id.clone()).collect();

        for r in [&mut r1, &mut r2] {
            room.handle_event(RoomEvent::BotReady {
                bot_id: r.bot_id.clone(),
            });
        }
        start(&mut room, "test").expect("start");
        // Force end-of-match (one ship dies).
        room.world.ships.get_mut(&r2.ship_id).unwrap().alive = false;
        room.step_tick();
        for _ in 0..(POST_GAME_LOBBY_TICKS + 1) {
            room.step_tick();
        }
        assert_eq!(room.state, RoomState::Lobby);

        // Bot and ship identities preserved.
        let bot_ids_after: Vec<BotId> = room.bots.keys().cloned().collect();
        let ship_ids_after: Vec<ShipId> = room.bots.values().map(|e| e.ship_id.clone()).collect();
        assert_eq!(bot_ids_before, bot_ids_after);
        assert_eq!(ship_ids_before, ship_ids_after);
    }

    #[test]
    fn reset_cuts_post_game_pause_short() {
        let mut room = test_room();
        let mut r = connect(&mut room, "a").expect("a");
        room.handle_event(RoomEvent::BotReady {
            bot_id: r.bot_id.clone(),
        });
        start(&mut room, "test").expect("start");
        let _ = drain_msgs(&mut r);

        abort(&mut room).expect("abort");
        assert_eq!(room.state, RoomState::Ended);
        // Immediately reset (skip the pause) — must succeed and transition to lobby.
        reset(&mut room).expect("reset");
        assert_eq!(room.state, RoomState::Lobby);
    }

    #[test]
    fn kick_removes_bot_and_ship() {
        let mut room = test_room();
        let r1 = connect(&mut room, "a").expect("a");
        let _ = connect(&mut room, "b").expect("b");
        assert_eq!(room.bot_count(), 2);

        kick(&mut room, &r1.bot_id).expect("kick");
        assert_eq!(room.bot_count(), 1);
        assert!(!room.world.ships.contains_key(&r1.ship_id));
    }

    #[test]
    fn kick_unknown_bot_is_rejected() {
        let mut room = test_room();
        let err = kick(&mut room, "b_999").expect_err("should refuse");
        assert_eq!(err, KickError::UnknownBot);
    }

    #[test]
    fn admin_subscribe_pushes_initial_snapshot() {
        let mut room = test_room();
        let (tx, _rx) = broadcast::channel::<AdminServerMsg>(8);
        room.set_admin_broadcast(tx.clone());
        let _r = connect(&mut room, "a").expect("a");

        // Subscribe; the reply receiver will contain the snapshot.
        let (reply_tx, mut reply_rx) = oneshot::channel();
        room.handle_event(RoomEvent::AdminSubscribe { reply: reply_tx });
        let mut admin_rx = reply_rx.try_recv().expect("subscribe reply");
        let first = admin_rx.try_recv().expect("initial snapshot frame");
        match first {
            AdminServerMsg::State(state) => {
                assert_eq!(state.room, "test");
                assert_eq!(state.state, "lobby");
                assert_eq!(state.bots.len(), 1);
                assert_eq!(state.bots[0].name, "a");
            }
            other => panic!("expected State, got {other:?}"),
        }
    }

    #[test]
    fn lifecycle_events_publish_admin_state() {
        let mut room = test_room();
        let (tx, mut rx) = broadcast::channel::<AdminServerMsg>(16);
        room.set_admin_broadcast(tx);
        // Drain any initial frames so the next push corresponds to BotConnect.
        while rx.try_recv().is_ok() {}

        let _r = connect(&mut room, "alice").expect("a");
        let snap = rx.try_recv().expect("BotConnect pushes state");
        match snap {
            AdminServerMsg::State(state) => assert_eq!(state.bots.len(), 1),
            other => panic!("expected State, got {other:?}"),
        }
    }
}
