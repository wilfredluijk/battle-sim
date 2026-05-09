//! Sensor filtering — what each ship can see of the world. Determinism contract: every
//! random draw goes through the room's seeded `Pcg64`. Iteration over ships is by
//! `ShipId` (BTreeMap order) so two replays with the same seed compute identical noise
//! offsets and contact counts.
//!
//! This module deliberately speaks in `glam::Vec2` and a sim-local `Contact` type. The
//! room translates these into `protocol::Contact` (assigning the per-tick `id` strings)
//! before they cross the wire — keeps `sim/` free of protocol imports per CLAUDE.md.

use std::collections::BTreeSet;

use glam::Vec2;
use rand::Rng;
use rand_pcg::Pcg64;

use super::constants::{
    ACTIVE_RADAR_NOISE, ACTIVE_RADAR_RANGE, PASSIVE_BEARING_NOISE_DEG,
    PASSIVE_CONTACT_PLACEHOLDER_DISTANCE, PASSIVE_HEAR_ACTIVE_RANGE, PASSIVE_HEAR_NEARBY_RANGE,
};
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

/// Passive listening from `viewer_id` at `viewer_pos`. Detects:
/// - Any ship in `active_pingers` within `PASSIVE_HEAR_ACTIVE_RANGE` (loud sweep), and
/// - Any ship at all within `PASSIVE_HEAR_NEARBY_RANGE` (engine noise close-by).
///
/// Returned contacts are bearing-only: `range = None`, and `pos` is a placeholder
/// projection out to `PASSIVE_CONTACT_PLACEHOLDER_DISTANCE` along the noisy bearing so
/// the wire frame stays consistent. One RNG draw per detected ship (the bearing noise).
pub fn passive_contacts(
    viewer_id: &ShipId,
    viewer_pos: Vec2,
    world: &World,
    active_pingers: &BTreeSet<ShipId>,
    rng: &mut Pcg64,
) -> Vec<Contact> {
    let mut out = Vec::new();
    for (id, ship) in &world.ships {
        if id == viewer_id || !ship.alive {
            continue;
        }
        let to = ship.pos - viewer_pos;
        let dist = to.length();
        let pinging = active_pingers.contains(id);
        let detected =
            dist <= PASSIVE_HEAR_NEARBY_RANGE || (pinging && dist <= PASSIVE_HEAR_ACTIVE_RANGE);
        if !detected {
            continue;
        }
        let true_bearing = compass_deg(to);
        let noise: f32 = rng.gen_range(-PASSIVE_BEARING_NOISE_DEG..=PASSIVE_BEARING_NOISE_DEG);
        let bearing = (true_bearing + noise).rem_euclid(360.0);
        let radians = bearing.to_radians();
        let placeholder = viewer_pos
            + Vec2::new(radians.sin(), -radians.cos()) * PASSIVE_CONTACT_PLACEHOLDER_DISTANCE;
        out.push(Contact {
            kind: ContactKind::Ship,
            pos: placeholder,
            bearing_deg: bearing,
            range: None,
            confidence: if pinging { 0.85 } else { 0.5 },
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

    // ----- Passive listening (Phase 5.2) ----------------------------------

    #[test]
    fn silent_ship_at_400_units_invisible_to_passive_listener() {
        // Acceptance check from projectplan §5.2: silent ship at 400 units invisible;
        // same ship pinging is visible.
        let mut world = World::new(2000.0, 2000.0);
        world.insert_ship(ship("s_1", 500.0, 500.0));
        world.insert_ship(ship("s_2", 900.0, 500.0)); // 400 units east
        let mut rng = Pcg64::seed_from_u64(11);

        // Silent: not in pingers set → invisible (400 > 150 nearby threshold).
        let silent = BTreeSet::<ShipId>::new();
        let contacts = passive_contacts(
            &"s_1".into(),
            Vec2::new(500.0, 500.0),
            &world,
            &silent,
            &mut rng,
        );
        assert!(
            contacts.is_empty(),
            "silent ship at 400u should be invisible: {contacts:?}"
        );

        // Pinging: included in pingers → visible (400 < 500 active threshold).
        let mut pingers = BTreeSet::<ShipId>::new();
        pingers.insert("s_2".into());
        let contacts = passive_contacts(
            &"s_1".into(),
            Vec2::new(500.0, 500.0),
            &world,
            &pingers,
            &mut rng,
        );
        assert_eq!(contacts.len(), 1, "pinging ship at 400u should be heard");
        assert!(contacts[0].range.is_none(), "passive must be bearing-only");
    }

    #[test]
    fn nearby_silent_ship_within_150_is_audible() {
        let mut world = World::new(1000.0, 1000.0);
        world.insert_ship(ship("s_1", 500.0, 500.0));
        world.insert_ship(ship("s_2", 600.0, 500.0)); // 100 units east, silent
        let mut rng = Pcg64::seed_from_u64(11);

        let silent = BTreeSet::<ShipId>::new();
        let contacts = passive_contacts(
            &"s_1".into(),
            Vec2::new(500.0, 500.0),
            &world,
            &silent,
            &mut rng,
        );
        assert_eq!(contacts.len(), 1, "ship at 100u (< 150) should be heard");
    }

    #[test]
    fn pinging_ship_beyond_500_is_inaudible() {
        let mut world = World::new(2000.0, 2000.0);
        world.insert_ship(ship("s_1", 100.0, 100.0));
        world.insert_ship(ship("s_2", 800.0, 100.0)); // 700 units east, pinging
        let mut pingers = BTreeSet::<ShipId>::new();
        pingers.insert("s_2".into());
        let mut rng = Pcg64::seed_from_u64(11);

        let contacts = passive_contacts(
            &"s_1".into(),
            Vec2::new(100.0, 100.0),
            &world,
            &pingers,
            &mut rng,
        );
        assert!(
            contacts.is_empty(),
            "pinger beyond 500u should be silent: {contacts:?}"
        );
    }

    #[test]
    fn passive_bearing_noise_is_bounded_by_five_degrees() {
        let mut world = World::new(1000.0, 1000.0);
        world.insert_ship(ship("s_1", 500.0, 500.0));
        world.insert_ship(ship("s_2", 600.0, 500.0)); // east, true bearing 90°
        let silent = BTreeSet::<ShipId>::new();
        let mut rng = Pcg64::seed_from_u64(7);

        for _ in 0..500 {
            let contacts = passive_contacts(
                &"s_1".into(),
                Vec2::new(500.0, 500.0),
                &world,
                &silent,
                &mut rng,
            );
            assert_eq!(contacts.len(), 1);
            let bearing = contacts[0].bearing_deg;
            // True bearing is 90; noise within ±5°. Wrapping isn't a concern for 90±5.
            let dev = (bearing - 90.0).abs();
            assert!(
                dev <= PASSIVE_BEARING_NOISE_DEG + 1e-4,
                "bearing {bearing} > 5° off from 90"
            );
        }
    }

    #[test]
    fn passive_contacts_are_deterministic_under_same_seed() {
        let mut world = World::new(1000.0, 1000.0);
        world.insert_ship(ship("s_1", 500.0, 500.0));
        world.insert_ship(ship("s_2", 600.0, 500.0));
        world.insert_ship(ship("s_3", 510.0, 600.0));
        let pingers = BTreeSet::<ShipId>::new();

        let mut rng_a = Pcg64::seed_from_u64(99);
        let mut rng_b = Pcg64::seed_from_u64(99);
        let viewer = Vec2::new(500.0, 500.0);
        let a = passive_contacts(&"s_1".into(), viewer, &world, &pingers, &mut rng_a);
        let b = passive_contacts(&"s_1".into(), viewer, &world, &pingers, &mut rng_b);
        assert_eq!(a, b);
    }
}
