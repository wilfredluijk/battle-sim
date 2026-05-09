//! Weapons. Pure simulation: a ship fires a shell, the shell flies for a fixed number of
//! ticks, on expiry it splashes and applies linear-falloff damage to nearby ships.
//!
//! Determinism contract (see `CLAUDE.md`): no wall-clock reads, no thread-local RNG. Ship
//! iteration during splash uses `BTreeMap` order. The `next_shell_index` counter on the
//! world is the only source of shell IDs, so two replays with the same command log produce
//! the same shells in the same order.

use glam::Vec2;

use super::constants::{
    DT, GUN_COOLDOWN_TICKS, MAX_SHELL_RANGE, MAX_SPLASH_DAMAGE, SHELL_SPEED, SPLASH_RADIUS,
};
use super::world::{Shell, ShipId, World};

/// Why `fire` refused to spawn a shell. The room translates this into the appropriate
/// `error` payload for the bot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FireError {
    /// Gun is still cooling down from a previous shot.
    CooldownActive,
    /// Ship is out of ammo.
    NoAmmo,
    /// Source ship has no entry in the world (e.g. just disconnected).
    UnknownShip,
    /// Source ship is not alive — corpses don't shoot.
    ShipDead,
}

/// Outcome events from a single tick of combat. Keep these sim-local; the room translates
/// them into protocol events (filtered by sensor range for bots, full for spectators).
#[derive(Debug, Clone, PartialEq)]
pub enum CombatEvent {
    /// `ship_id` took `amount` HP of splash damage at `pos` (the splash centre).
    Hit {
        ship_id: ShipId,
        amount: u32,
        pos: Vec2,
    },
    /// A shell expired and exploded at `pos` (regardless of whether anyone was hit).
    Splash { pos: Vec2 },
    /// `ship_id` dropped to 0 HP this tick.
    Death { ship_id: ShipId },
}

/// Spawn a shell from `ship_id` along `bearing_deg`, requested travel distance `range`.
/// Range is clamped to `MAX_SHELL_RANGE`. On success the ship's ammo is decremented and
/// gun cooldown is set; on failure the world is unchanged.
pub fn fire(
    world: &mut World,
    ship_id: &ShipId,
    bearing_deg: f32,
    range: f32,
) -> Result<(), FireError> {
    let ship = world.ships.get_mut(ship_id).ok_or(FireError::UnknownShip)?;
    if !ship.alive {
        return Err(FireError::ShipDead);
    }
    if ship.gun_cooldown > 0 {
        return Err(FireError::CooldownActive);
    }
    if ship.ammo == 0 {
        return Err(FireError::NoAmmo);
    }

    let clamped_range = range.clamp(0.0, MAX_SHELL_RANGE);
    let dir = bearing_to_unit_vec(bearing_deg);
    let vel = dir * SHELL_SPEED;
    // Time to travel `clamped_range` at `SHELL_SPEED`, in fixed-DT ticks. `ceil` so a
    // request just over a tick boundary still gets that final tick of flight.
    let ttl_ticks = (clamped_range / (SHELL_SPEED * DT)).ceil() as u32;

    let shell = Shell {
        id_index: world.next_shell_index,
        source_ship: ship_id.clone(),
        pos: ship.pos,
        vel,
        ttl_ticks,
    };
    world.next_shell_index = world.next_shell_index.wrapping_add(1);
    world.shells.push(shell);

    ship.gun_cooldown = GUN_COOLDOWN_TICKS;
    ship.ammo -= 1;
    Ok(())
}

/// Advance every in-flight shell by one tick. Shells whose TTL reaches 0 explode in place
/// and apply splash damage to all alive ships within `SPLASH_RADIUS`. Friendly fire is on.
///
/// Iteration order: shells are processed in spawn order (insertion order is stable);
/// hit / death events for a single splash are emitted in `BTreeMap` (BotId) order.
pub fn step_shells(world: &mut World) -> Vec<CombatEvent> {
    let mut events = Vec::new();
    let mut remaining = Vec::with_capacity(world.shells.len());
    let shells = std::mem::take(&mut world.shells);

    for mut shell in shells {
        shell.pos += shell.vel * DT;
        // Saturating in case fire() ever spawns a TTL=0 shell — explode immediately rather
        // than wrap to u32::MAX.
        shell.ttl_ticks = shell.ttl_ticks.saturating_sub(1);
        if shell.ttl_ticks > 0 {
            remaining.push(shell);
            continue;
        }

        events.push(CombatEvent::Splash { pos: shell.pos });
        for (id, ship) in world.ships.iter_mut() {
            if !ship.alive {
                continue;
            }
            let d = ship.pos.distance(shell.pos);
            if d > SPLASH_RADIUS {
                continue;
            }
            // Linear falloff: full damage at centre, zero at the edge. `round` keeps the
            // integer HP loss honest at the boundaries.
            let frac = 1.0 - (d / SPLASH_RADIUS);
            let dmg = (MAX_SPLASH_DAMAGE as f32 * frac).round() as u32;
            if dmg == 0 {
                continue;
            }
            ship.hp = ship.hp.saturating_sub(dmg);
            events.push(CombatEvent::Hit {
                ship_id: id.clone(),
                amount: dmg,
                pos: shell.pos,
            });
            if ship.hp == 0 {
                ship.alive = false;
                events.push(CombatEvent::Death {
                    ship_id: id.clone(),
                });
            }
        }
    }

    world.shells = remaining;
    events
}

/// Compass bearing → unit vector. Matches `physics::heading_to_unit_vec`: 0° = north (-y),
/// 90° = east (+x). Duplicated here to keep `combat` independent of `physics`.
fn bearing_to_unit_vec(bearing_deg: f32) -> Vec2 {
    let r = bearing_deg.to_radians();
    Vec2::new(r.sin(), -r.cos())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::constants;
    use crate::sim::world::Ship;

    fn world_with(ships: Vec<Ship>) -> World {
        let mut w = World::new(1000.0, 1000.0);
        for s in ships {
            w.insert_ship(s);
        }
        w
    }

    fn ship_at(id: &str, pos: Vec2) -> Ship {
        Ship::new_at(id.into(), format!("b_{id}"), pos, 0.0)
    }

    #[test]
    fn fire_at_bearing_90_range_200_spawns_shell_with_eastward_velocity_and_40_ticks_ttl() {
        // Acceptance check from projectplan §6.1.
        let mut world = world_with(vec![ship_at("s_1", Vec2::new(500.0, 500.0))]);
        let result = fire(&mut world, &"s_1".into(), 90.0, 200.0);
        assert!(result.is_ok(), "fire should succeed: {result:?}");

        assert_eq!(world.shells.len(), 1);
        let s = &world.shells[0];
        assert_eq!(s.source_ship, "s_1");
        assert_eq!(s.id_index, 0);
        assert_eq!(s.ttl_ticks, 40, "ttl = range / (speed * dt) = 200 / 5 = 40");
        // Bearing 90° → east → vel = (50, 0).
        assert!(
            (s.vel.x - constants::SHELL_SPEED).abs() < 1e-4,
            "vx = {}",
            s.vel.x
        );
        assert!(s.vel.y.abs() < 1e-4, "vy = {}", s.vel.y);
        // Spawn position is the firer's position.
        assert!((s.pos - Vec2::new(500.0, 500.0)).length() < 1e-4);

        // Side effects: ammo down by one, cooldown set, next_shell_index bumped.
        let firer = world.ships.get("s_1").unwrap();
        assert_eq!(firer.ammo, constants::MAX_AMMO - 1);
        assert_eq!(firer.gun_cooldown, constants::GUN_COOLDOWN_TICKS);
        assert_eq!(world.next_shell_index, 1);
    }

    #[test]
    fn fire_clamps_range_to_max() {
        let mut world = world_with(vec![ship_at("s_1", Vec2::new(500.0, 500.0))]);
        fire(&mut world, &"s_1".into(), 0.0, 9999.0).expect("fire");
        let s = &world.shells[0];
        // 300 / 5 = 60 ticks at the cap.
        assert_eq!(s.ttl_ticks, 60);
    }

    #[test]
    fn fire_rejected_during_cooldown() {
        let mut world = world_with(vec![ship_at("s_1", Vec2::new(500.0, 500.0))]);
        fire(&mut world, &"s_1".into(), 0.0, 100.0).expect("first fire");
        let err = fire(&mut world, &"s_1".into(), 0.0, 100.0).expect_err("second fire");
        assert_eq!(err, FireError::CooldownActive);
        // Only the first shell exists; ammo only dropped once.
        assert_eq!(world.shells.len(), 1);
        assert_eq!(
            world.ships.get("s_1").unwrap().ammo,
            constants::MAX_AMMO - 1
        );
    }

    #[test]
    fn fire_rejected_when_out_of_ammo() {
        let mut world = world_with(vec![ship_at("s_1", Vec2::new(500.0, 500.0))]);
        world.ships.get_mut("s_1").unwrap().ammo = 0;
        let err = fire(&mut world, &"s_1".into(), 0.0, 100.0).expect_err("no ammo");
        assert_eq!(err, FireError::NoAmmo);
        assert!(world.shells.is_empty());
    }

    #[test]
    fn dead_ships_cannot_fire() {
        let mut world = world_with(vec![ship_at("s_1", Vec2::new(500.0, 500.0))]);
        world.ships.get_mut("s_1").unwrap().alive = false;
        let err = fire(&mut world, &"s_1".into(), 0.0, 100.0).expect_err("dead");
        assert_eq!(err, FireError::ShipDead);
    }

    #[test]
    fn shell_advances_each_tick_then_explodes_on_ttl_expiry() {
        // Acceptance check from projectplan §6.2: shell expires next to a stationary ship
        // and lands the expected damage.
        let mut world = world_with(vec![
            ship_at("s_1", Vec2::new(500.0, 500.0)),
            ship_at("s_2", Vec2::new(700.0, 500.0)),
        ]);
        // Aim s_1 east; with range = 200 the shell will land exactly on s_2.
        fire(&mut world, &"s_1".into(), 90.0, 200.0).expect("fire");
        // Step until expiry — 40 ticks.
        let mut last_events = Vec::new();
        for _ in 0..40 {
            last_events = step_shells(&mut world);
        }
        // The world's shells list is empty after explosion.
        assert!(world.shells.is_empty());
        // We should see a Splash + Hit pair (and a Death — full splash kills 25 hp, so s_2
        // remains alive at 75 HP). Check by filtering the event list.
        let splashes: Vec<_> = last_events
            .iter()
            .filter(|e| matches!(e, CombatEvent::Splash { .. }))
            .collect();
        let hits: Vec<_> = last_events
            .iter()
            .filter(|e| matches!(e, CombatEvent::Hit { .. }))
            .collect();
        assert_eq!(splashes.len(), 1, "one splash expected");
        assert_eq!(hits.len(), 1, "one hit expected");
        let CombatEvent::Hit {
            ship_id, amount, ..
        } = hits[0]
        else {
            unreachable!()
        };
        assert_eq!(ship_id, "s_2");
        assert_eq!(*amount, constants::MAX_SPLASH_DAMAGE);
        // s_2 took the full 25 splash dmg.
        assert_eq!(world.ships.get("s_2").unwrap().hp, constants::HULL_HP - 25);
        assert!(world.ships.get("s_2").unwrap().alive);
    }

    #[test]
    fn splash_damage_falls_off_linearly_with_distance() {
        // Place s_2 halfway out: dmg should be ~50% of max (rounded).
        let mut world = world_with(vec![
            ship_at("s_1", Vec2::new(500.0, 500.0)),
            // 207.5 = 200 + half of splash radius (15/2 = 7.5). Means at impact distance
            // ~7.5, frac = 0.5 → dmg ≈ 12 (round 12.5).
            ship_at("s_2", Vec2::new(707.5, 500.0)),
        ]);
        fire(&mut world, &"s_1".into(), 90.0, 200.0).expect("fire");
        for _ in 0..40 {
            step_shells(&mut world);
        }
        let hp_loss = constants::HULL_HP - world.ships.get("s_2").unwrap().hp;
        // Round of 12.5 in Rust's `f32::round` is half-away-from-zero, so 13.
        assert!(
            (11..=14).contains(&hp_loss),
            "expected ~half splash damage, got {hp_loss}"
        );
    }

    #[test]
    fn ship_outside_splash_radius_takes_no_damage() {
        let mut world = world_with(vec![
            ship_at("s_1", Vec2::new(500.0, 500.0)),
            // Place s_2 well outside splash (200 + 20).
            ship_at("s_2", Vec2::new(720.0, 500.0)),
        ]);
        fire(&mut world, &"s_1".into(), 90.0, 200.0).expect("fire");
        for _ in 0..40 {
            step_shells(&mut world);
        }
        assert_eq!(world.ships.get("s_2").unwrap().hp, constants::HULL_HP);
    }

    #[test]
    fn friendly_fire_damages_self() {
        // Acceptance check from projectplan §6.2: ship hit by its own shell takes damage.
        // Aim straight up but request a tiny range so the shell expires next to the firer.
        let mut world = world_with(vec![ship_at("s_1", Vec2::new(500.0, 500.0))]);
        fire(&mut world, &"s_1".into(), 0.0, 5.0).expect("fire");
        // Range 5 → ttl = ceil(5/5) = 1 tick. Shell flies 5 units north and explodes 5
        // units from the firer; well inside the 15-unit splash.
        let events = step_shells(&mut world);
        let self_hit = events
            .iter()
            .find(|e| matches!(e, CombatEvent::Hit { ship_id, .. } if ship_id == "s_1"));
        assert!(self_hit.is_some(), "self-hit event missing: {events:?}");
        assert!(world.ships.get("s_1").unwrap().hp < constants::HULL_HP);
    }

    #[test]
    fn ship_at_zero_hp_is_marked_dead_and_emits_death_event() {
        let mut world = world_with(vec![
            ship_at("s_1", Vec2::new(500.0, 500.0)),
            ship_at("s_2", Vec2::new(700.0, 500.0)),
        ]);
        // Knock s_2 to 1 HP so a single splash kills it.
        world.ships.get_mut("s_2").unwrap().hp = 1;
        fire(&mut world, &"s_1".into(), 90.0, 200.0).expect("fire");
        let mut last = Vec::new();
        for _ in 0..40 {
            last = step_shells(&mut world);
        }
        assert!(!world.ships.get("s_2").unwrap().alive);
        assert_eq!(world.ships.get("s_2").unwrap().hp, 0);
        let died = last
            .iter()
            .any(|e| matches!(e, CombatEvent::Death { ship_id } if ship_id == "s_2"));
        assert!(died, "death event missing: {last:?}");
    }

    #[test]
    fn shell_index_is_monotonic_across_fires() {
        let mut world = world_with(vec![
            ship_at("s_1", Vec2::new(500.0, 500.0)),
            ship_at("s_2", Vec2::new(600.0, 500.0)),
        ]);
        fire(&mut world, &"s_1".into(), 0.0, 100.0).expect("a");
        fire(&mut world, &"s_2".into(), 0.0, 100.0).expect("b");
        assert_eq!(world.shells[0].id_index, 0);
        assert_eq!(world.shells[1].id_index, 1);
        assert_eq!(world.next_shell_index, 2);
    }
}
