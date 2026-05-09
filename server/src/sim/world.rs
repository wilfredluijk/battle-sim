//! Authoritative simulation state for a single room.
//!
//! Determinism contract (see `CLAUDE.md`): all maps use `BTreeMap` for stable iteration
//! order, all floats are `f32`, no wall-clock reads inside this module.

use std::collections::BTreeMap;

use glam::Vec2;

use super::constants;

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
        }
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
}

#[derive(Debug, Clone)]
pub struct World {
    pub tick: u64,
    pub width: f32,
    pub height: f32,
    pub ships: BTreeMap<ShipId, Ship>,
    pub shells: Vec<Shell>,
    pub next_shell_index: u32,
}

impl World {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            tick: 0,
            width,
            height,
            ships: BTreeMap::new(),
            shells: Vec::new(),
            next_shell_index: 0,
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
        let mut world = World::new(1000.0, 1000.0);
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
        let mut world = World::new(1000.0, 1000.0);
        for id in ["s_3", "s_1", "s_2"] {
            world.insert_ship(Ship::new_at(id.into(), "b".into(), Vec2::ZERO, 0.0));
        }
        let order: Vec<&str> = world.ships.keys().map(String::as_str).collect();
        assert_eq!(order, vec!["s_1", "s_2", "s_3"]);
    }
}
