//! Authoritative simulation state for a single room.
//!
//! Determinism contract (see `CLAUDE.md`): all maps use `BTreeMap` for stable iteration
//! order, all floats are `f32`, no wall-clock reads inside this module.

use std::collections::BTreeMap;

use glam::Vec2;

use super::config::SimConfig;
use super::constants;
use super::powerups::PowerupState;

/// Stable bot identifier, e.g. `"b_3"`. Assigned by the room on `hello`.
pub type BotId = String;

/// Stable ship identifier, e.g. `"s_3"`. Assigned by the room on `hello`.
pub type ShipId = String;

#[derive(Debug, Clone, PartialEq)]
pub struct Ship {
    pub id: ShipId,
    pub bot_id: BotId,
    pub pos: Vec2,
    /// Absolute compass heading in degrees. 0° = north (-y), 90° = east (+x).
    pub heading_deg: f32,
    /// Signed scalar speed: positive = ahead, negative = reverse.
    pub speed: f32,
    pub hp: u32,
    pub ammo: u32,
    /// Last commanded throttle in `[-1, 1]`. Persists between commands per §4.1.
    pub throttle: f32,
    /// Last commanded rudder in `[-1, 1]`. Persists between commands per §4.1.
    pub rudder: f32,
    /// Ticks remaining until the gun is ready to fire again.
    pub gun_cooldown: u32,
    pub alive: bool,
    /// Per-ship powerup state: which powerups were picked, which were used, and active
    /// effect expirations. See [`super::powerups`].
    pub powerups: PowerupState,
}

impl Ship {
    pub fn new_at(id: ShipId, bot_id: BotId, pos: Vec2, heading_deg: f32) -> Self {
        Self {
            id,
            bot_id,
            pos,
            heading_deg,
            speed: 0.0,
            hp: constants::HULL_HP,
            ammo: constants::MAX_AMMO,
            throttle: 0.0,
            rudder: 0.0,
            gun_cooldown: 0,
            alive: true,
            powerups: PowerupState::default(),
        }
    }

    /// Reset all mutable state to fresh-spawn values without changing `id` / `bot_id`.
    /// Used by the room to recycle a ship between back-to-back matches on the same
    /// connection: the bot keeps its identity, the hull is brand new. `hp` / `ammo` are
    /// taken from `config` so a rebalanced match starts with the configured values.
    /// Powerup selections are preserved (they were committed at match start); only the
    /// transient effect state and "already-used" set are cleared.
    pub fn reset_for_round(&mut self, pos: Vec2, heading_deg: f32, config: &SimConfig) {
        self.pos = pos;
        self.heading_deg = heading_deg;
        self.speed = 0.0;
        self.hp = config.hull_hp;
        self.ammo = config.max_ammo;
        self.throttle = 0.0;
        self.rudder = 0.0;
        self.gun_cooldown = 0;
        self.alive = true;
        self.powerups.reset_for_round();
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shell {
    /// Stable per-shell index used by spectators to track trails.
    pub id_index: u32,
    pub source_ship: ShipId,
    pub pos: Vec2,
    pub vel: Vec2,
    /// Ticks remaining until the shell expires and resolves splash damage.
    pub ttl_ticks: u32,
    /// Splash radius this shell will detonate with. Baked at fire time so a `heavy_shell`
    /// buff expiring mid-flight does not de-buff in-flight shells.
    pub splash_radius: f32,
    /// Peak splash damage this shell will deal at its centre.
    pub max_splash_damage: u32,
}

/// A static smoke cloud spawned by `smoke_screen`. Position and radius are frozen at
/// activation time; the cloud is garbage-collected by `powerups::step_tick_maintenance`
/// once `world.tick >= expires_at`.
#[derive(Debug, Clone, PartialEq)]
pub struct SmokeCloud {
    pub pos: Vec2,
    pub radius: f32,
    pub expires_at: u64,
}

/// A phantom contact spawned by `decoy_flare`. Stationary at `pos`; the activating ship
/// does not perceive its own decoy.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoy {
    /// Stable per-decoy index for spectator UIs. The bot-facing protocol uses a synthetic
    /// `d_<index>` id derived from this number.
    pub fake_id: u32,
    pub owner: ShipId,
    pub pos: Vec2,
    pub heading_deg: f32,
    pub expires_at: u64,
}

#[derive(Debug, Clone)]
pub struct World {
    pub tick: u64,
    pub width: f32,
    pub height: f32,
    pub ships: BTreeMap<ShipId, Ship>,
    pub shells: Vec<Shell>,
    pub next_shell_index: u32,
    /// Live smoke clouds, in insertion order. Iteration is deterministic; expired entries
    /// are removed by `powerups::step_tick_maintenance`.
    pub smoke_clouds: Vec<SmokeCloud>,
    /// Live decoys, in insertion order.
    pub decoys: Vec<Decoy>,
    /// Monotonic counter for decoy ids — used to label decoys in the spectator wire and in
    /// the synthetic contacts they generate.
    pub next_decoy_index: u32,
    /// Balance parameters for the current match. Frozen when the match starts; the
    /// simulation reads ship / weapon / sensor tunables from here.
    pub config: SimConfig,
}

impl World {
    pub fn new(width: f32, height: f32, config: SimConfig) -> Self {
        Self {
            tick: 0,
            width,
            height,
            ships: BTreeMap::new(),
            shells: Vec::new(),
            next_shell_index: 0,
            smoke_clouds: Vec::new(),
            decoys: Vec::new(),
            next_decoy_index: 0,
            config,
        }
    }

    pub fn insert_ship(&mut self, ship: Ship) {
        self.ships.insert(ship.id.clone(), ship);
    }

    /// Number of ships currently `alive`.
    pub fn alive_count(&self) -> usize {
        self.ships.values().filter(|s| s.alive).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_construct_world_with_two_ships_and_read_back_state() {
        let mut world = World::new(1000.0, 1000.0, SimConfig::default());
        world.insert_ship(Ship::new_at(
            "s_1".into(),
            "b_1".into(),
            Vec2::new(100.0, 200.0),
            0.0,
        ));
        world.insert_ship(Ship::new_at(
            "s_2".into(),
            "b_2".into(),
            Vec2::new(800.0, 800.0),
            180.0,
        ));

        assert_eq!(world.tick, 0);
        assert_eq!(world.ships.len(), 2);
        assert_eq!(world.alive_count(), 2);

        let s1 = world.ships.get("s_1").expect("s_1 present");
        assert_eq!(s1.pos, Vec2::new(100.0, 200.0));
        assert_eq!(s1.heading_deg, 0.0);
        assert_eq!(s1.hp, constants::HULL_HP);
        assert_eq!(s1.ammo, constants::MAX_AMMO);
        assert!(s1.alive);

        let s2 = world.ships.get("s_2").expect("s_2 present");
        assert_eq!(s2.heading_deg, 180.0);
        assert_eq!(s2.bot_id, "b_2");
    }

    #[test]
    fn btreemap_iteration_is_stable() {
        // Determinism check: ships always iterate in BotId order regardless of insert order.
        let mut world = World::new(1000.0, 1000.0, SimConfig::default());
        for id in ["s_3", "s_1", "s_2"] {
            world.insert_ship(Ship::new_at(id.into(), "b".into(), Vec2::ZERO, 0.0));
        }
        let order: Vec<&str> = world.ships.keys().map(String::as_str).collect();
        assert_eq!(order, vec!["s_1", "s_2", "s_3"]);
    }
}
