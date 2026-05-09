//! Sensor filtering — what each ship can see of the world. Determinism contract: every
//! random draw goes through the room's seeded `Pcg64`. Iteration over ships is by
//! `ShipId` (BTreeMap order) so two replays with the same seed compute identical noise
//! offsets and contact counts.
//!
//! This module deliberately speaks in `glam::Vec2` and a sim-local `Contact` type. The
//! room translates these into `protocol::Contact` (assigning the per-tick `id` strings)
//! before they cross the wire — keeps `sim/` free of protocol imports per CLAUDE.md.

use glam::Vec2;
use rand::Rng;
use rand_pcg::Pcg64;

use super::constants::{ACTIVE_RADAR_NOISE, ACTIVE_RADAR_RANGE};
use super::world::{ShipId, World};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactKind {
    Ship,
    Shell,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Contact {
    pub kind: ContactKind,
    /// Reported position (with sensor noise applied where appropriate).
    pub pos: Vec2,
    /// Compass bearing from viewer to target (0° = north, 90° = east).
    pub bearing_deg: f32,
    /// Range from viewer; `None` for bearing-only sensors (passive).
    pub range: Option<f32>,
    pub confidence: f32,
}

/// Active-radar sweep from the perspective of `viewer_id` standing at `viewer_pos`. Sees
/// every other alive ship inside `ACTIVE_RADAR_RANGE`; reports position with seeded
/// uniform ±`ACTIVE_RADAR_NOISE` noise, bearing from true relative position, range from
/// true distance.
///
/// Two RNG draws are made per detected ship (x noise, then y noise). Calling order is
/// stable BotId-by-BotId because `World::ships` is a `BTreeMap`.
pub fn active_contacts(
    viewer_id: &ShipId,
    viewer_pos: Vec2,
    world: &World,
    rng: &mut Pcg64,
) -> Vec<Contact> {
    let mut out = Vec::new();
    for (id, ship) in &world.ships {
        if id == viewer_id || !ship.alive {
            continue;
        }
        let to = ship.pos - viewer_pos;
        let dist = to.length();
        if dist > ACTIVE_RADAR_RANGE {
            continue;
        }
        let nx: f32 = rng.gen_range(-ACTIVE_RADAR_NOISE..=ACTIVE_RADAR_NOISE);
        let ny: f32 = rng.gen_range(-ACTIVE_RADAR_NOISE..=ACTIVE_RADAR_NOISE);
        out.push(Contact {
            kind: ContactKind::Ship,
            pos: ship.pos + Vec2::new(nx, ny),
            bearing_deg: compass_deg(to),
            range: Some(dist),
            confidence: 1.0,
        });
    }
    out
}

/// Compass bearing of vector `v` (0° = north / -y, 90° = east / +x). Result in `[0, 360)`.
fn compass_deg(v: Vec2) -> f32 {
    let deg = v.x.atan2(-v.y).to_degrees();
    if deg < 0.0 {
        deg + 360.0
    } else {
        deg
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::world::Ship;
    use rand::SeedableRng;

    fn ship(id: &str, x: f32, y: f32) -> Ship {
        Ship::new_at(id.into(), format!("b_{id}"), Vec2::new(x, y), 0.0)
    }

    #[test]
    fn two_ships_within_350_units_each_see_one_contact_when_active() {
        // Acceptance check from projectplan §5.1.
        let mut world = World::new(1000.0, 1000.0);
        world.insert_ship(ship("s_1", 500.0, 500.0));
        world.insert_ship(ship("s_2", 700.0, 500.0)); // 200 units east
        let mut rng = Pcg64::seed_from_u64(42);

        let from1 = active_contacts(&"s_1".into(), Vec2::new(500.0, 500.0), &world, &mut rng);
        let from2 = active_contacts(&"s_2".into(), Vec2::new(700.0, 500.0), &world, &mut rng);

        assert_eq!(from1.len(), 1, "s_1 should see exactly one contact (s_2)");
        assert_eq!(from2.len(), 1, "s_2 should see exactly one contact (s_1)");
        let r = from1[0].range.expect("active range present");
        assert!((r - 200.0).abs() < 1e-3, "range was {r}");
        assert_eq!(from1[0].kind, ContactKind::Ship);
    }

    #[test]
    fn ships_outside_radar_range_are_invisible() {
        let mut world = World::new(2000.0, 2000.0);
        world.insert_ship(ship("s_1", 100.0, 100.0));
        world.insert_ship(ship("s_2", 800.0, 100.0)); // 700 units away (> 350)
        let mut rng = Pcg64::seed_from_u64(42);

        let contacts = active_contacts(&"s_1".into(), Vec2::new(100.0, 100.0), &world, &mut rng);
        assert!(
            contacts.is_empty(),
            "out-of-range ship leaked: {contacts:?}"
        );
    }

    #[test]
    fn dead_ships_do_not_appear_as_contacts() {
        let mut world = World::new(1000.0, 1000.0);
        world.insert_ship(ship("s_1", 500.0, 500.0));
        let mut s2 = ship("s_2", 600.0, 500.0);
        s2.alive = false;
        world.insert_ship(s2);
        let mut rng = Pcg64::seed_from_u64(42);

        let contacts = active_contacts(&"s_1".into(), Vec2::new(500.0, 500.0), &world, &mut rng);
        assert!(contacts.is_empty(), "dead ship leaked: {contacts:?}");
    }

    #[test]
    fn position_noise_is_bounded_by_two_units() {
        let mut world = World::new(1000.0, 1000.0);
        world.insert_ship(ship("s_1", 500.0, 500.0));
        world.insert_ship(ship("s_2", 600.0, 500.0));
        let mut rng = Pcg64::seed_from_u64(7);

        // Hit the function many times; check every reported position is within ±2 of truth.
        let true_pos = Vec2::new(600.0, 500.0);
        for _ in 0..500 {
            let contacts =
                active_contacts(&"s_1".into(), Vec2::new(500.0, 500.0), &world, &mut rng);
            assert_eq!(contacts.len(), 1);
            let dx = (contacts[0].pos.x - true_pos.x).abs();
            let dy = (contacts[0].pos.y - true_pos.y).abs();
            assert!(
                dx <= ACTIVE_RADAR_NOISE + 1e-6,
                "x noise {dx} out of bounds"
            );
            assert!(
                dy <= ACTIVE_RADAR_NOISE + 1e-6,
                "y noise {dy} out of bounds"
            );
        }
    }

    #[test]
    fn same_seed_produces_identical_contacts() {
        let mut world = World::new(1000.0, 1000.0);
        world.insert_ship(ship("s_1", 500.0, 500.0));
        world.insert_ship(ship("s_2", 600.0, 500.0));
        world.insert_ship(ship("s_3", 500.0, 700.0));

        let mut rng_a = Pcg64::seed_from_u64(99);
        let mut rng_b = Pcg64::seed_from_u64(99);
        let viewer = Vec2::new(500.0, 500.0);
        let a = active_contacts(&"s_1".into(), viewer, &world, &mut rng_a);
        let b = active_contacts(&"s_1".into(), viewer, &world, &mut rng_b);
        assert_eq!(a, b, "same seed must yield byte-identical contacts");
    }

    #[test]
    fn bearing_is_compass_from_viewer_to_target() {
        let mut world = World::new(1000.0, 1000.0);
        world.insert_ship(ship("s_1", 500.0, 500.0));
        // Place targets in each cardinal direction from the viewer.
        world.insert_ship(ship("s_e", 600.0, 500.0)); // east → 90°
        world.insert_ship(ship("s_n", 500.0, 400.0)); // north → 0°
        let mut rng = Pcg64::seed_from_u64(1);

        let mut contacts =
            active_contacts(&"s_1".into(), Vec2::new(500.0, 500.0), &world, &mut rng);
        // Sort by range so we have a stable order independent of BTreeMap iteration.
        contacts.sort_by(|a, b| a.bearing_deg.partial_cmp(&b.bearing_deg).unwrap());
        // Sorted: 0° (north), 90° (east).
        assert!((contacts[0].bearing_deg - 0.0).abs() < 1e-3);
        assert!((contacts[1].bearing_deg - 90.0).abs() < 1e-3);
    }
}
