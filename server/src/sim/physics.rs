//! Movement integration. Everything here is pure: it reads commanded `throttle` /
//! `rudder` off each ship and advances `pos` / `heading_deg` / `speed` by one fixed
//! timestep `DT`. No wall-clock reads, no global state.

use glam::Vec2;

use super::constants::{
    ACCELERATION, DT, MAX_FORWARD_SPEED, MAX_REVERSE_SPEED, TURN_RATE_DEG_PER_S, WALL_BUMP_DAMAGE,
};
use super::world::{Ship, World};

/// Advance every alive ship in the world by one tick.
pub fn step_world(world: &mut World) {
    let (width, height) = (world.width, world.height);
    for ship in world.ships.values_mut() {
        if !ship.alive {
            continue;
        }
        step_ship(ship, width, height);
        if ship.gun_cooldown > 0 {
            ship.gun_cooldown -= 1;
        }
    }
}

/// Integrate one ship by one tick. Order: speed → heading → position → wall clamp.
pub fn step_ship(ship: &mut Ship, width: f32, height: f32) {
    // 1. Speed: drift toward target dictated by throttle, capped by acceleration.
    let target = target_speed(ship.throttle);
    let max_step = ACCELERATION * DT;
    let delta = target - ship.speed;
    if delta.abs() <= max_step {
        ship.speed = target;
    } else {
        ship.speed += delta.signum() * max_step;
    }

    // 2. Heading: turn rate scales linearly with |speed| / max_forward.
    let turn_rate = TURN_RATE_DEG_PER_S * ship.rudder * (ship.speed.abs() / MAX_FORWARD_SPEED);
    ship.heading_deg = wrap_deg(ship.heading_deg + turn_rate * DT);

    // 3. Position: advance along heading vector.
    let direction = heading_to_unit_vec(ship.heading_deg);
    ship.pos += direction * ship.speed * DT;

    // 4. Walls: clamp, stop, and bump for damage.
    let clamped = ship.pos.clamp(Vec2::ZERO, Vec2::new(width, height));
    if clamped != ship.pos {
        ship.pos = clamped;
        ship.speed = 0.0;
        ship.hp = ship.hp.saturating_sub(WALL_BUMP_DAMAGE);
        if ship.hp == 0 {
            ship.alive = false;
        }
    }
}

/// Convert a throttle in `[-1, 1]` into the desired scalar speed.
fn target_speed(throttle: f32) -> f32 {
    let t = throttle.clamp(-1.0, 1.0);
    if t >= 0.0 {
        t * MAX_FORWARD_SPEED
    } else {
        t * MAX_REVERSE_SPEED
    }
}

/// Compass-heading to unit vector. 0° = north (-y), 90° = east (+x).
fn heading_to_unit_vec(heading_deg: f32) -> Vec2 {
    let r = heading_deg.to_radians();
    Vec2::new(r.sin(), -r.cos())
}

fn wrap_deg(d: f32) -> f32 {
    let m = d.rem_euclid(360.0);
    // rem_euclid on f32 can return very small negative values from rounding; pin to [0, 360).
    if m < 0.0 {
        m + 360.0
    } else {
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::constants;
    use crate::sim::world::Ship;

    const W: f32 = 1000.0;
    const H: f32 = 1000.0;

    fn ship_at(pos: Vec2, heading: f32) -> Ship {
        Ship::new_at("s_test".into(), "b_test".into(), pos, heading)
    }

    #[test]
    fn full_throttle_reaches_max_forward_speed() {
        let mut s = ship_at(Vec2::new(500.0, 500.0), 0.0);
        s.throttle = 1.0;
        // 6.0 / (1.5 * 0.1) = 40 ticks to reach max; iterate a bit more for slack.
        for _ in 0..50 {
            step_ship(&mut s, W, H);
        }
        assert!(
            (s.speed - MAX_FORWARD_SPEED).abs() < 1e-4,
            "speed = {} expected ~{MAX_FORWARD_SPEED}",
            s.speed
        );
    }

    #[test]
    fn full_reverse_throttle_reaches_max_reverse_speed() {
        let mut s = ship_at(Vec2::new(500.0, 500.0), 0.0);
        s.throttle = -1.0;
        for _ in 0..50 {
            step_ship(&mut s, W, H);
        }
        assert!(
            (s.speed + MAX_REVERSE_SPEED).abs() < 1e-4,
            "speed = {} expected ~{}",
            s.speed,
            -MAX_REVERSE_SPEED
        );
    }

    #[test]
    fn full_rudder_at_top_speed_turns_at_spec_rate() {
        let mut s = ship_at(Vec2::new(500.0, 500.0), 0.0);
        s.speed = MAX_FORWARD_SPEED;
        s.throttle = 1.0; // hold the speed
        s.rudder = 1.0;
        let h0 = s.heading_deg;
        step_ship(&mut s, W, H);
        // Expected: TURN_RATE_DEG_PER_S * DT per tick at top speed.
        let expected = constants::TURN_RATE_DEG_PER_S * constants::DT;
        let delta = s.heading_deg - h0;
        assert!(
            (delta - expected).abs() < 1e-4,
            "heading delta = {delta}, expected {expected}"
        );
    }

    #[test]
    fn stationary_ship_barely_turns() {
        // §5.3: turn rate scales linearly with speed.
        let mut s = ship_at(Vec2::new(500.0, 500.0), 0.0);
        s.rudder = 1.0; // throttle stays 0, speed 0
        for _ in 0..10 {
            step_ship(&mut s, W, H);
        }
        assert_eq!(s.heading_deg, 0.0, "stationary ship should not rotate");
    }

    #[test]
    fn east_heading_advances_positive_x() {
        let mut s = ship_at(Vec2::new(500.0, 500.0), 90.0);
        s.speed = MAX_FORWARD_SPEED;
        s.throttle = 1.0;
        step_ship(&mut s, W, H);
        let step = constants::MAX_FORWARD_SPEED * constants::DT;
        assert!((s.pos.x - (500.0 + step)).abs() < 1e-4, "x = {}", s.pos.x);
        assert!((s.pos.y - 500.0).abs() < 1e-4, "y = {}", s.pos.y);
    }

    #[test]
    fn north_heading_advances_negative_y() {
        let mut s = ship_at(Vec2::new(500.0, 500.0), 0.0);
        s.speed = MAX_FORWARD_SPEED;
        s.throttle = 1.0;
        step_ship(&mut s, W, H);
        let step = constants::MAX_FORWARD_SPEED * constants::DT;
        assert!((s.pos.y - (500.0 - step)).abs() < 1e-4, "y = {}", s.pos.y);
    }

    #[test]
    fn wall_collision_clamps_and_damages() {
        let mut s = ship_at(Vec2::new(999.5, 500.0), 90.0); // east, near east wall
        s.speed = MAX_FORWARD_SPEED;
        s.throttle = 1.0;
        let hp0 = s.hp;
        step_ship(&mut s, W, H);
        assert!(
            (s.pos.x - W).abs() < 1e-4,
            "expected clamp to wall, x = {}",
            s.pos.x
        );
        assert_eq!(s.speed, 0.0, "wall hit should stop the ship");
        assert!(s.hp < hp0, "wall hit should deal damage");
        assert_eq!(s.hp, hp0 - constants::WALL_BUMP_DAMAGE);
    }

    #[test]
    fn wall_collision_north_wall_clamps_y_to_zero() {
        let mut s = ship_at(Vec2::new(500.0, 0.5), 0.0); // north, near north wall
        s.speed = MAX_FORWARD_SPEED;
        s.throttle = 1.0;
        step_ship(&mut s, W, H);
        assert!(s.pos.y.abs() < 1e-4, "y should clamp to 0, got {}", s.pos.y);
        assert_eq!(s.speed, 0.0);
    }

    #[test]
    fn dead_ships_are_skipped_by_step_world() {
        let mut world = World::new(W, H);
        let mut alive = ship_at(Vec2::new(500.0, 500.0), 90.0);
        alive.id = "s_alive".into();
        alive.throttle = 1.0;
        alive.speed = MAX_FORWARD_SPEED;

        let mut dead = ship_at(Vec2::new(100.0, 100.0), 0.0);
        dead.id = "s_dead".into();
        dead.alive = false;
        dead.throttle = 1.0;
        let dead_pos = dead.pos;

        world.insert_ship(alive);
        world.insert_ship(dead);

        step_world(&mut world);

        let dead = world.ships.get("s_dead").unwrap();
        assert_eq!(dead.pos, dead_pos, "dead ship should not move");

        let alive = world.ships.get("s_alive").unwrap();
        assert!(alive.pos.x > 500.0, "alive ship should have moved east");
    }
}
