//! Weapons. Pure simulation: a ship fires a shell, the shell flies for a fixed number of
//! ticks, on expiry it splashes and applies linear-falloff damage to nearby ships.
//!
//! Determinism contract (see `CLAUDE.md`): no wall-clock reads, no thread-local RNG. Ship
//! iteration during splash uses `BTreeMap` order. The `next_shell_index` counter on the
//! world is the only source of shell IDs, so two replays with the same command log produce
//! the same shells in the same order.

use glam::Vec2;

use super::constants::DT;
use super::powerups::{self, PowerupId};
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
/// them into protocol events (filtered by sensor range for bots, full for spectators) and
/// folds them into per-bot match statistics.
#[derive(Debug, Clone, PartialEq)]
pub enum CombatEvent {
    /// `ship_id` took `amount` HP of splash damage at `pos` (the splash centre). `source`
    /// is the ship that fired the shell — used for damage-dealt attribution in the report.
    Hit {
        ship_id: ShipId,
        amount: u32,
        pos: Vec2,
        source: ShipId,
    },
    /// A shell expired and exploded at `pos` (regardless of whether anyone was hit).
    Splash { pos: Vec2 },
    /// `ship_id` dropped to 0 HP this tick. `source` is the ship whose shell landed the
    /// killing blow — used for kill attribution in the post-match report.
    Death { ship_id: ShipId, source: ShipId },
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
    let config = world.config;
    let tick = world.tick;
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

    // Shell-tag buffs that travel with the projectile, baked at fire time.
    let heavy_active = ship.powerups.is_active(PowerupId::HeavyShell, tick);
    let long_active = ship.powerups.is_active(PowerupId::LongRangeSalvo, tick);
    let shell_splash_radius = if heavy_active {
        powerups::buffed_splash_radius(config.splash_radius, &config.powerups)
    } else {
        config.splash_radius
    };
    let shell_max_damage = if heavy_active {
        powerups::buffed_splash_damage(config.max_splash_damage, &config.powerups)
    } else {
        config.max_splash_damage
    };
    let shell_speed = if long_active {
        powerups::buffed_shell_speed(config.shell_speed, &config.powerups)
    } else {
        config.shell_speed
    };
    let max_shell_range = if long_active {
        powerups::buffed_max_shell_range(config.max_shell_range, &config.powerups)
    } else {
        config.max_shell_range
    };

    let clamped_range = range.clamp(0.0, max_shell_range);
    let dir = bearing_to_unit_vec(bearing_deg);
    let vel = dir * shell_speed;
    // Time to travel `clamped_range` at `shell_speed`, in fixed-DT ticks. `ceil` so a
    // request just over a tick boundary still gets that final tick of flight.
    let ttl_ticks = (clamped_range / (shell_speed * DT)).ceil() as u32;

    let shell = Shell {
        id_index: world.next_shell_index,
        source_ship: ship_id.clone(),
        pos: ship.pos,
        vel,
        ttl_ticks,
        splash_radius: shell_splash_radius,
        max_splash_damage: shell_max_damage,
    };
    world.next_shell_index = world.next_shell_index.wrapping_add(1);
    world.shells.push(shell);

    let effective_cooldown = powerups::effective_gun_cooldown_ticks(
        config.gun_cooldown_ticks,
        &ship.powerups,
        &config.powerups,
        tick,
    );
    ship.gun_cooldown = effective_cooldown;
    ship.ammo -= 1;
    // Firing breaks silent_running immediately — the muzzle flash is unambiguous.
    if ship.powerups.silent_running_expires_at > tick {
        ship.powerups.silent_running_expires_at = tick;
    }
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
    let powerup_config = world.config.powerups;
    let tick = world.tick;
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
        // Per-shell splash parameters: a heavy_shell-tagged shell carries its (boosted)
        // values from fire time.
        let splash_radius = shell.splash_radius;
        let max_splash_damage = shell.max_splash_damage;
        for (id, ship) in world.ships.iter_mut() {
            if !ship.alive {
                continue;
            }
            let d = ship.pos.distance(shell.pos);
            if d > splash_radius {
                continue;
            }
            // Linear falloff: full damage at centre, zero at the edge. `round` keeps the
            // integer HP loss honest at the boundaries.
            let frac = 1.0 - (d / splash_radius);
            let raw_dmg = (max_splash_damage as f32 * frac).round() as u32;
            // Defender's `reinforced_hull` scales the damage actually subtracted.
            let dmg = powerups::apply_incoming_damage_reduction(
                raw_dmg,
                &ship.powerups,
                &powerup_config,
                tick,
            );
            if dmg == 0 {
                continue;
            }
            ship.hp = ship.hp.saturating_sub(dmg);
            // Counter-battery trace: first hit while armed locks in the shooter for the
            // next `counter_battery_reveal_ticks` reveal frames. Self-hits and hits from
            // an unknown source don't trigger a trace (no useful info).
            if ship.powerups.trace_armed_until > tick
                && ship.powerups.trace_pending_reveals == 0
                && shell.source_ship != *id
            {
                ship.powerups.trace_attacker = Some(shell.source_ship.clone());
                ship.powerups.trace_pending_reveals = powerup_config.counter_battery_reveal_ticks;
                // Trace fires once per arming — disarm immediately so a second incoming
                // hit doesn't overwrite the attacker mid-reveal-sequence.
                ship.powerups.trace_armed_until = 0;
            }
            events.push(CombatEvent::Hit {
                ship_id: id.clone(),
                amount: dmg,
                pos: shell.pos,
                source: shell.source_ship.clone(),
            });
            if ship.hp == 0 {
                ship.alive = false;
                events.push(CombatEvent::Death {
                    ship_id: id.clone(),
                    source: shell.source_ship.clone(),
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
        let mut w = World::new(1000.0, 1000.0, crate::sim::SimConfig::default());
        for s in ships {
            w.insert_ship(s);
        }
        w
    }

    fn ship_at(id: &str, pos: Vec2) -> Ship {
        Ship::new_at(id.into(), format!("b_{id}"), pos, 0.0)
    }

    #[test]
    fn fire_at_bearing_90_range_200_spawns_shell_with_expected_velocity_and_ttl() {
        // Acceptance check from projectplan §6.1.
        let mut world = world_with(vec![ship_at("s_1", Vec2::new(500.0, 500.0))]);
        let result = fire(&mut world, &"s_1".into(), 90.0, 200.0);
        assert!(result.is_ok(), "fire should succeed: {result:?}");

        assert_eq!(world.shells.len(), 1);
        let s = &world.shells[0];
        assert_eq!(s.source_ship, "s_1");
        assert_eq!(s.id_index, 0);
        let expected_ttl = (200.0_f32 / (constants::SHELL_SPEED * constants::DT)).ceil() as u32;
        assert_eq!(
            s.ttl_ticks,
            expected_ttl,
            "ttl = ceil(range / (speed * dt)) = ceil(200 / {}) = {expected_ttl}",
            constants::SHELL_SPEED * constants::DT,
        );
        // Bearing 90° → east → vx = SHELL_SPEED, vy = 0.
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
        let expected_ttl =
            (constants::MAX_SHELL_RANGE / (constants::SHELL_SPEED * constants::DT)).ceil() as u32;
        assert_eq!(s.ttl_ticks, expected_ttl);
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
        // and lands the expected damage. With a 70 unit/s shell and 0.1s dt, a requested
        // range of 200 yields ttl = ceil(200 / 7) = 29 ticks and an actual flight of 203
        // units. Place s_2 at the exact impact for a centre splash.
        let range = 200.0_f32;
        let flight = (range / (constants::SHELL_SPEED * constants::DT)).ceil()
            * constants::SHELL_SPEED
            * constants::DT;
        let mut world = world_with(vec![
            ship_at("s_1", Vec2::new(500.0, 500.0)),
            ship_at("s_2", Vec2::new(500.0 + flight, 500.0)),
        ]);
        fire(&mut world, &"s_1".into(), 90.0, range).expect("fire");
        // Step until the shell explodes; the final step's events are what we assert on.
        let mut last_events = Vec::new();
        while !world.shells.is_empty() {
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
        // Place s_2 half a splash radius beyond the actual impact point so frac ≈ 0.5 →
        // dmg ≈ MAX_SPLASH_DAMAGE / 2.
        let range = 200.0_f32;
        let flight = (range / (constants::SHELL_SPEED * constants::DT)).ceil()
            * constants::SHELL_SPEED
            * constants::DT;
        let impact_x = 500.0 + flight;
        let half_splash = constants::SPLASH_RADIUS * 0.5;
        let mut world = world_with(vec![
            ship_at("s_1", Vec2::new(500.0, 500.0)),
            ship_at("s_2", Vec2::new(impact_x + half_splash, 500.0)),
        ]);
        fire(&mut world, &"s_1".into(), 90.0, range).expect("fire");
        while !world.shells.is_empty() {
            step_shells(&mut world);
        }
        let hp_loss = constants::HULL_HP - world.ships.get("s_2").unwrap().hp;
        let half = constants::MAX_SPLASH_DAMAGE / 2;
        // Allow ±2 HP slack for f32 rounding around the half-distance boundary.
        assert!(
            (half.saturating_sub(2)..=half + 2).contains(&hp_loss),
            "expected ~half ({half}) splash damage, got {hp_loss}",
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
        let range = 200.0_f32;
        let flight = (range / (constants::SHELL_SPEED * constants::DT)).ceil()
            * constants::SHELL_SPEED
            * constants::DT;
        let mut world = world_with(vec![
            ship_at("s_1", Vec2::new(500.0, 500.0)),
            ship_at("s_2", Vec2::new(500.0 + flight, 500.0)),
        ]);
        // Knock s_2 to 1 HP so a single splash kills it.
        world.ships.get_mut("s_2").unwrap().hp = 1;
        fire(&mut world, &"s_1".into(), 90.0, range).expect("fire");
        let mut last = Vec::new();
        while !world.shells.is_empty() {
            last = step_shells(&mut world);
        }
        assert!(!world.ships.get("s_2").unwrap().alive);
        assert_eq!(world.ships.get("s_2").unwrap().hp, 0);
        let died = last
            .iter()
            .any(|e| matches!(e, CombatEvent::Death { ship_id, .. } if ship_id == "s_2"));
        assert!(died, "death event missing: {last:?}");
    }

    #[test]
    fn reinforced_hull_reduces_damage_from_direct_hit() {
        // s_1 fires a tiny-range shot at s_2; without reinforced hull s_2 loses 25 hp at
        // the centre. With reinforced hull active, damage scales by 0.4 → ~10 hp.
        let mut s1 = ship_at("s_1", Vec2::new(500.0, 500.0));
        let mut s2 = ship_at("s_2", Vec2::new(500.0, 500.0));
        s2.powerups.selected = vec![PowerupId::ReinforcedHull];
        s2.powerups.reinforced_hull_expires_at = 100;
        let mut world = world_with(vec![s1.clone(), s2]);
        // Tiny range — shell explodes at firer.
        fire(&mut world, &"s_1".into(), 0.0, 1.0).expect("fire");
        while !world.shells.is_empty() {
            step_shells(&mut world);
        }
        let hp_loss = constants::HULL_HP - world.ships.get("s_2").unwrap().hp;
        assert!(
            hp_loss < constants::MAX_SPLASH_DAMAGE,
            "reinforced hull should reduce centre-splash damage; hp_loss={hp_loss}"
        );
        // Now without reinforced hull, the same setup deals full splash damage.
        s1.id = "s_3".into();
        s1.bot_id = "b_3".into();
        let mut control_world = world_with(vec![s1, ship_at("s_4", Vec2::new(500.0, 500.0))]);
        fire(&mut control_world, &"s_3".into(), 0.0, 1.0).expect("fire");
        while !control_world.shells.is_empty() {
            step_shells(&mut control_world);
        }
        let control_loss = constants::HULL_HP - control_world.ships.get("s_4").unwrap().hp;
        assert!(
            control_loss > hp_loss,
            "control without reinforced hull should lose more hp ({control_loss} vs {hp_loss})"
        );
    }

    #[test]
    fn counter_battery_trace_records_attacker_on_first_hit() {
        let mut s1 = ship_at("s_1", Vec2::new(500.0, 500.0));
        let mut s2 = ship_at("s_2", Vec2::new(500.0, 500.0));
        s2.powerups.selected = vec![PowerupId::CounterBatteryTrace];
        s2.powerups.trace_armed_until = 100;
        let mut world = world_with(vec![s1.clone(), s2]);
        fire(&mut world, &"s_1".into(), 0.0, 1.0).expect("fire");
        while !world.shells.is_empty() {
            step_shells(&mut world);
        }
        let trace = &world.ships.get("s_2").unwrap().powerups;
        assert_eq!(trace.trace_attacker.as_deref(), Some("s_1"));
        assert!(
            trace.trace_pending_reveals > 0,
            "should have queued reveals"
        );
        // Trace consumed — second hit doesn't overwrite the attacker mid-reveal.
        s1.id = "s_3".into();
        s1.bot_id = "b_3".into();
        s1.gun_cooldown = 0;
        world.ships.insert(s1.id.clone(), s1);
        // Make sure s_2 has hp left.
        world.ships.get_mut("s_2").unwrap().hp = 80;
        fire(&mut world, &"s_3".into(), 0.0, 1.0).expect("fire");
        while !world.shells.is_empty() {
            step_shells(&mut world);
        }
        assert_eq!(
            world
                .ships
                .get("s_2")
                .unwrap()
                .powerups
                .trace_attacker
                .as_deref(),
            Some("s_1"),
            "second hit must not overwrite mid-reveal attacker"
        );
    }

    #[test]
    fn firing_breaks_silent_running() {
        let mut s1 = ship_at("s_1", Vec2::new(500.0, 500.0));
        s1.powerups.selected = vec![PowerupId::SilentRunning];
        s1.powerups.silent_running_expires_at = 100;
        let mut world = world_with(vec![s1]);
        assert!(world
            .ships
            .get("s_1")
            .unwrap()
            .powerups
            .is_active(PowerupId::SilentRunning, 0));
        fire(&mut world, &"s_1".into(), 0.0, 100.0).expect("fire");
        // Silent running expires at tick 0 (world.tick is still 0), so is_active should
        // return false now.
        assert!(!world
            .ships
            .get("s_1")
            .unwrap()
            .powerups
            .is_active(PowerupId::SilentRunning, 0));
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
