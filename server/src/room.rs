//! Room: a single match. Owns the world, the RNG, and the tick loop.
//!
//! The room is the bridge between the protocol (`net.rs`) and the deterministic simulation
//! (`sim/`). It receives `RoomEvent`s over an mpsc channel, mutates the world, and replies
//! to bots via per-connection mpsc senders. Bot lifecycle (Phase 4.1) lives here; per-tick
//! command exchange lands in Phase 4.3.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use glam::Vec2;
use rand::SeedableRng;
use rand_pcg::Pcg64;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{interval, MissedTickBehavior};
use tracing::{debug, info, warn};

use crate::protocol::{MapInfo, ServerMsg, ShipSpecs};
use crate::sim::{physics, BotId, Ship, ShipId, World};

/// Channel buffer for outbound messages to a single bot. Sized for a few ticks of slack —
/// the bot consumes one message per tick under normal operation.
const BOT_OUTBOUND_BUFFER: usize = 32;

/// Channel buffer for inbound `RoomEvent`s. One event per bot action; tens of bots tops.
pub const ROOM_EVENT_BUFFER: usize = 256;

/// Radius of the §5.6 starting circle. Bots are placed evenly around the map center,
/// all facing inward.
const STARTING_RING_RADIUS: f32 = 400.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomState {
    Lobby,
    Running,
    Ended,
}

/// Per-bot state tracked by the room.
#[derive(Debug)]
#[allow(dead_code)] // `name`/`peer`/`outbound` are read once tick→command lands (Phase 4.3).
struct BotEntry {
    bot_id: BotId,
    ship_id: ShipId,
    name: String,
    peer: SocketAddr,
    outbound: mpsc::Sender<ServerMsg>,
    ready: bool,
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
}

impl JoinError {
    pub fn as_str(&self) -> &'static str {
        match self {
            JoinError::NotInLobby => "room is not accepting bots (already running or ended)",
            JoinError::RoomFull => "room is full",
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
    pub tick_hz: u32,
    pub max_bots: u32,
    bots: BTreeMap<BotId, BotEntry>,
    next_index: u32,
}

impl Room {
    pub fn new(
        name: String,
        width: f32,
        height: f32,
        seed: u64,
        tick_hz: u32,
        max_bots: u32,
    ) -> Self {
        Self {
            name,
            world: World::new(width, height),
            state: RoomState::Lobby,
            rng: Pcg64::seed_from_u64(seed),
            tick_hz,
            max_bots,
            bots: BTreeMap::new(),
            next_index: 1,
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
    pub fn step_tick(&mut self) {
        if self.state == RoomState::Running {
            physics::step_world(&mut self.world);
        }
        self.world.tick = self.world.tick.saturating_add(1);
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

            let entry = self.bots.get(bot_id).expect("snapshot still in map");
            if let Some(ship) = self.world.ships.get_mut(&entry.ship_id) {
                ship.pos = pos;
                ship.heading_deg = heading_deg;
                ship.speed = 0.0;
                ship.throttle = 0.0;
                ship.rudder = 0.0;
            }

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
        info!(room = %self.name, bots = self.bots.len(), "match started");
        Ok(())
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

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!(room = %name, final_tick = room.world.tick, "room: shutdown");
                break;
            }
            Some(event) = event_rx.recv() => {
                room.handle_event(event);
            }
            _ = ticker.tick() => {
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
    use std::net::Ipv4Addr;

    fn test_peer() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, 12345))
    }

    fn test_room() -> Room {
        Room::new("test".into(), 1000.0, 1000.0, 42, 10, 4)
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
        let mut room = Room::new("test".into(), 1000.0, 1000.0, 42, 10, 2);
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
