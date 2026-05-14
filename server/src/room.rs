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
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{interval, MissedTickBehavior};
use tracing::{debug, info, warn};

use crate::protocol::{
    self, error_code, Contact as ProtocolContact, ContactKind as ProtocolContactKind, FireCommand,
    MapInfo, SelfState, SensorMode, ServerMsg, ShipSpecs, SpectatorEvent, SpectatorMsg,
    SpectatorShell, SpectatorShip, TickEvent,
};
use crate::replay::{
    self, ReplayBot, ReplayCommand, ReplayEnd, ReplayHeader, ReplayRecord, ReplayTick,
    ReplayWriter, REPLAY_FORMAT_VERSION,
};
use crate::sim::combat::{self, CombatEvent, FireError};
use crate::sim::constants::{ACTIVE_RADAR_RANGE, PASSIVE_HEAR_NEARBY_RANGE};
use crate::sim::sensors::{self, Contact as SimContact, ContactKind as SimContactKind};
use crate::sim::{physics, BotId, Ship, ShipId, World};

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
    /// Override for the default `MATCH_TIMEOUT_TICKS` cap. `None` means use the default.
    /// Lowering this is how integration scenarios bound a run's wall-clock length without
    /// changing physics behavior.
    max_ticks: Option<u64>,
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
            world: World::new(width, height),
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
            max_ticks: None,
        }
    }

    /// Override the default match tick cap. `None` reverts to `MATCH_TIMEOUT_TICKS`.
    pub fn set_max_ticks(&mut self, max_ticks: Option<u64>) {
        self.max_ticks = max_ticks;
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
        if self.state != RoomState::Running {
            self.world.tick = self.world.tick.saturating_add(1);
            // Spectators still see the lobby/ended state — full ground truth, no events.
            self.broadcast_spectator_world(&[]);
            return;
        }

        let bot_ids: Vec<BotId> = self.bots.keys().cloned().collect();

        // Snapshot of commands actually applied this tick, in BotId order. Written to the
        // replay log after the tick counter is bumped so the on-disk tick number matches
        // the post-step world state.
        let mut applied_commands: Vec<ReplayCommand> = Vec::new();

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
                if let Err(err) = combat::fire(
                    &mut self.world,
                    &ship_id,
                    fire_cmd.bearing_deg,
                    fire_cmd.range,
                ) {
                    self.send_fire_error(bot_id, err);
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
                });
            }
        }

        // 2 + 3. Movement, then shell flight & splashes.
        physics::step_world(&mut self.world);
        let combat_events = combat::step_shells(&mut self.world);

        // 4. Bump the tick counter so the outbound frames carry the new tick number.
        self.world.tick = self.world.tick.saturating_add(1);

        // Persist the commands that drove this tick. Writing here (post-bump) means the
        // recorded `tick` field equals the world tick the commands produced, which is the
        // tick the bots received next time around.
        self.write_replay_tick(applied_commands);

        // Spectator broadcast: full ground truth + every combat event. Done before the
        // end-of-match check so the deciding tick (with its death events) is visible.
        self.broadcast_spectator_world(&combat_events);

        // 5. End-of-match check. Broadcasting `game_over` and returning early means dead
        //    and surviving bots all hear about the outcome via the same message; no final
        //    `tick` frame is sent for the deciding tick.
        if let Some(winner) = self.match_outcome() {
            self.state = RoomState::Ended;
            self.broadcast_game_over(winner);
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
            let contacts = sim_contacts
                .into_iter()
                .enumerate()
                .map(|(i, c)| translate_contact(i, c))
                .collect();
            let events = filter_events_for_bot(&ship_id, viewer_pos, sensor_mode, &combat_events);

            let entry = self.bots.get(bot_id).expect("bot still present");
            let ship = self.world.ships.get(&ship_id).expect("ship still present");
            let tick_msg = ServerMsg::Tick {
                tick: self.world.tick,
                deadline_ms: self.tick_deadline_ms,
                self_state: SelfState {
                    pos: [ship.pos.x, ship.pos.y],
                    heading_deg: ship.heading_deg,
                    speed: ship.speed,
                    hp: ship.hp,
                    ammo: ship.ammo,
                    rudder: ship.rudder,
                    throttle: ship.throttle,
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
        let tick_cap = self.max_ticks.unwrap_or(MATCH_TIMEOUT_TICKS);
        if self.world.tick >= tick_cap {
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

    /// Build a `SpectatorMsg::World` from the current world state and push it onto the
    /// spectator broadcast channel. No-op when no channel is wired (unit tests). Send
    /// failures (no subscribers) are intentionally swallowed — the simulation never
    /// stalls because nobody is watching.
    fn broadcast_spectator_world(&self, events: &[CombatEvent]) {
        let Some(tx) = self.spectator_tx.as_ref() else {
            return;
        };
        if tx.receiver_count() == 0 {
            // Nothing to do; skip the JSON serialization cost when nobody's watching.
            return;
        }

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

        let events: Vec<SpectatorEvent> = events
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
                CombatEvent::Death { ship_id } => SpectatorEvent::Death {
                    ship_id: ship_id.clone(),
                },
            })
            .collect();

        let msg = SpectatorMsg::World {
            tick: self.world.tick,
            ships,
            shells,
            events,
        };
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

    /// Write a terminal `end` record if a replay log is still open. Called when the room
    /// is torn down by shutdown rather than a natural match end, so the on-disk JSONL is
    /// well-formed for downstream validators. `winner` is `None` because there was no
    /// `match_outcome` resolution — the run was cut short.
    pub fn finalize_replay_if_running(&mut self) {
        if self.state != RoomState::Running {
            return;
        }
        let final_tick = self.world.tick;
        if let Some(writer) = self.replay_writer.as_mut() {
            let end = ReplayRecord::End(ReplayEnd {
                tick: final_tick,
                winner: None,
            });
            if let Err(e) = writer.write(&end) {
                warn!(room = %self.name, error = %e, "failed to write replay end on shutdown");
            }
        }
        // Drop the writer so the BufWriter flushes to disk.
        if let Some(writer) = self.replay_writer.take() {
            if let Some(path) = writer.path() {
                info!(
                    room = %self.name,
                    final_tick,
                    path = %path.display(),
                    "replay log closed on shutdown"
                );
            }
            drop(writer);
        }
        self.state = RoomState::Ended;
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
            FireError::CooldownActive => (
                error_code::COOLDOWN_ACTIVE,
                format!("gun is on cooldown for tick {}", self.world.tick),
            ),
            FireError::NoAmmo => (error_code::NO_AMMO, "ship is out of ammo".to_string()),
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
            }
            RoomEvent::BotReady { bot_id } => {
                if let Some(entry) = self.bots.get_mut(&bot_id) {
                    if !entry.ready {
                        entry.ready = true;
                        info!(room = %self.name, bot = %bot_id, "bot ready");
                    }
                } else {
                    warn!(room = %self.name, bot = %bot_id, "ready from unknown bot, ignored");
                }
            }
            RoomEvent::BotCommand { bot_id, command } => {
                self.handle_bot_command(bot_id, command);
            }
            RoomEvent::BotDisconnect { bot_id } => {
                if let Some(entry) = self.bots.remove(&bot_id) {
                    self.world.ships.remove(&entry.ship_id);
                    info!(room = %self.name, bot = %bot_id, ship = %entry.ship_id, "bot disconnected");
                }
            }
            RoomEvent::OperatorStart { room, reply } => {
                let result = self.start_match(&room);
                if let Err(ref e) = result {
                    warn!(room = %self.name, requested = %room, reason = e.as_str(), "operator start refused");
                }
                let _ = reply.send(result);
            }
        }
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

        let center = Vec2::new(self.world.width * 0.5, self.world.height * 0.5);
        let n = self.bots.len() as f32;
        // Snapshot bot ids so we can mutate `self.world` and read `self.bots` without
        // simultaneous &mut+&. Iteration order is BotId-stable (BTreeMap).
        let ordered_ids: Vec<BotId> = self.bots.keys().cloned().collect();
        for (i, bot_id) in ordered_ids.iter().enumerate() {
            let angle = std::f32::consts::TAU * (i as f32) / n;
            let offset = Vec2::new(angle.cos(), angle.sin()) * STARTING_RING_RADIUS;
            let pos = center + offset;
            let heading_deg = compass_deg_facing(pos, center);

            let ship_id = {
                let entry = self.bots.get_mut(bot_id).expect("snapshot still in map");
                // Drop any commands queued before the match started.
                entry.pending_command = None;
                entry.ship_id.clone()
            };
            if let Some(ship) = self.world.ships.get_mut(&ship_id) {
                ship.pos = pos;
                ship.heading_deg = heading_deg;
                ship.speed = 0.0;
                ship.throttle = 0.0;
                ship.rudder = 0.0;
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
        self.state = RoomState::Running;
        // Cleared on entry; the first `step_tick` will populate them after broadcasting.
        self.tick_send_time = None;
        self.previous_active_pingers.clear();
        self.starting_bot_count = self.bots.len() as u32;
        info!(room = %self.name, bots = self.bots.len(), "match started");

        // Open a new replay log (unless a writer was injected externally — e.g. by tests)
        // and emit the header. Failures here are logged but not fatal: the match runs even
        // if we can't write the log.
        self.open_replay_writer_if_configured();
        self.write_replay_header();
        Ok(())
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
        let replay_id = replay::make_replay_id(&self.name);
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
            bots: self
                .bots
                .values()
                .map(|b| ReplayBot {
                    bot_id: b.bot_id.clone(),
                    ship_id: b.ship_id.clone(),
                    name: b.name.clone(),
                })
                .collect(),
        };
        if let Err(e) = writer.write(&ReplayRecord::Header(header)) {
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
            ship_specs: ShipSpecs::DEFAULT,
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
    events: &[CombatEvent],
) -> Vec<TickEvent> {
    let splash_range = match sensor_mode {
        SensorMode::Active => ACTIVE_RADAR_RANGE,
        SensorMode::Passive => PASSIVE_HEAR_NEARBY_RANGE,
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

/// Drive a room's tick loop until the shutdown channel fires. Consumes `RoomEvent`s
/// from `event_rx` between ticks; events are applied in arrival order.
///
/// When `auto_start` is true, the room transitions to `Running` as soon as `max_bots`
/// bots are connected and every one is `ready` — no operator command required. The room
/// also self-terminates once `state == Ended`, so a scripted scenario reaches a clean
/// shutdown without an external `quit` signal.
pub async fn run_room(
    mut room: Room,
    mut event_rx: mpsc::Receiver<RoomEvent>,
    mut shutdown_rx: broadcast::Receiver<()>,
    auto_start: bool,
) -> u64 {
    let period = Duration::from_secs_f64(1.0 / f64::from(room.tick_hz.max(1)));
    let mut ticker = interval(period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let name = room.name.clone();
    let target_bots = room.max_bots;
    info!(room = %name, tick_hz = room.tick_hz, auto_start, "room started");

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!(room = %name, final_tick = room.world.tick, "room: shutdown");
                // If the match was still running, close the replay log so the validator
                // sees a terminal `end` record instead of a truncated file.
                room.finalize_replay_if_running();
                break;
            }
            Some(event) = event_rx.recv() => {
                room.handle_event(event);
                if auto_start
                    && room.state == RoomState::Lobby
                    && room.bot_count() as u32 >= target_bots
                    && room.all_ready()
                {
                    let (tx, _rx) = oneshot::channel();
                    room.handle_event(RoomEvent::OperatorStart {
                        room: name.clone(),
                        reply: tx,
                    });
                }
            }
            _ = ticker.tick() => {
                room.step_tick();
                debug!(room = %name, tick = room.world.tick, state = ?room.state, "tick");
                if room.state == RoomState::Ended {
                    info!(room = %name, final_tick = room.world.tick, "room: match ended, stopping");
                    break;
                }
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
        room.world.shells.push(crate::sim::world::Shell {
            id_index: 0,
            source_ship: r2.ship_id.clone(),
            pos: Vec2::new(400.0, 500.0),
            vel: Vec2::ZERO,
            ttl_ticks: 1,
        });
        room.world.shells.push(crate::sim::world::Shell {
            id_index: 1,
            source_ship: r1.ship_id.clone(),
            pos: Vec2::new(500.0, 500.0),
            vel: Vec2::ZERO,
            ttl_ticks: 1,
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
}
