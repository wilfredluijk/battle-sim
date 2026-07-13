//! Per-match one-off powerups. A bot picks two distinct powerups before the match starts
//! and may activate each one once during the match. Effects are time-bounded and tied to
//! `world.tick` so the simulation stays deterministic and replays bit-faithful.
//!
//! Determinism contract (see `CLAUDE.md`): no wall-clock reads, no `HashMap` iteration,
//! and every effect-state field is integer- or `f32`-typed and keyed on `world.tick`.
//! Effect helpers here are the only branch on a powerup id — combat, physics, and sensors
//! call into these helpers rather than matching on powerup names themselves.

use std::collections::BTreeSet;

use glam::Vec2;
use rand::Rng;
use rand_pcg::Pcg64;
use serde::{Deserialize, Serialize};

use super::config::PowerupConfig;
use super::constants::DT;
use super::world::{Decoy, ShipId, SmokeCloud, World};

/// The full set of powerups available to bots. Snake-case serialization matches the wire
/// protocol — pull `powerup.as_str()` to get the on-the-wire id.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PowerupId {
    Overdrive,
    ReinforcedHull,
    RepairDrones,
    SmokeScreen,
    RapidFire,
    HeavyShell,
    LongRangeSalvo,
    AwacsScan,
    SilentRunning,
    CounterBatteryTrace,
    EmpBurst,
    DecoyFlare,
}

impl PowerupId {
    /// The on-the-wire identifier (snake_case). Round-trips through `from_str`.
    pub fn as_str(self) -> &'static str {
        match self {
            PowerupId::Overdrive => "overdrive",
            PowerupId::ReinforcedHull => "reinforced_hull",
            PowerupId::RepairDrones => "repair_drones",
            PowerupId::SmokeScreen => "smoke_screen",
            PowerupId::RapidFire => "rapid_fire",
            PowerupId::HeavyShell => "heavy_shell",
            PowerupId::LongRangeSalvo => "long_range_salvo",
            PowerupId::AwacsScan => "awacs_scan",
            PowerupId::SilentRunning => "silent_running",
            PowerupId::CounterBatteryTrace => "counter_battery_trace",
            PowerupId::EmpBurst => "emp_burst",
            PowerupId::DecoyFlare => "decoy_flare",
        }
    }

    /// Parse a wire id back into the enum. Returns `None` for unknown values so the room
    /// can emit a typed `powerup_unknown` error instead of crashing.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "overdrive" => Some(PowerupId::Overdrive),
            "reinforced_hull" => Some(PowerupId::ReinforcedHull),
            "repair_drones" => Some(PowerupId::RepairDrones),
            "smoke_screen" => Some(PowerupId::SmokeScreen),
            "rapid_fire" => Some(PowerupId::RapidFire),
            "heavy_shell" => Some(PowerupId::HeavyShell),
            "long_range_salvo" => Some(PowerupId::LongRangeSalvo),
            "awacs_scan" => Some(PowerupId::AwacsScan),
            "silent_running" => Some(PowerupId::SilentRunning),
            "counter_battery_trace" => Some(PowerupId::CounterBatteryTrace),
            "emp_burst" => Some(PowerupId::EmpBurst),
            "decoy_flare" => Some(PowerupId::DecoyFlare),
            _ => None,
        }
    }

    /// All powerups, in canonical (declaration) order. Used by the catalog the server
    /// advertises in `welcome` and to enumerate effect state for the per-bot `tick`
    /// payload.
    pub fn all() -> &'static [PowerupId] {
        &[
            PowerupId::Overdrive,
            PowerupId::ReinforcedHull,
            PowerupId::RepairDrones,
            PowerupId::SmokeScreen,
            PowerupId::RapidFire,
            PowerupId::HeavyShell,
            PowerupId::LongRangeSalvo,
            PowerupId::AwacsScan,
            PowerupId::SilentRunning,
            PowerupId::CounterBatteryTrace,
            PowerupId::EmpBurst,
            PowerupId::DecoyFlare,
        ]
    }
}

/// Per-ship powerup state. Every field is `0` / `None` when no effect is active; an effect
/// is "active at tick `t`" iff its `*_expires_at` field is `> t`. Stored on `Ship` so the
/// simulation can read it without going through the room.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PowerupState {
    /// The (at most two) powerups this bot picked for the match. Order is the bot's pick
    /// order — the room rejects more than two and duplicates at selection time.
    pub selected: Vec<PowerupId>,
    /// Powerups that have already been activated this match. Each picked powerup can fire
    /// at most once.
    pub used: BTreeSet<PowerupId>,

    // --- Per-effect expiry / state (all keyed on world.tick) ---------------
    pub overdrive_expires_at: u64,
    pub reinforced_hull_expires_at: u64,
    pub repair_drones_expires_at: u64,
    pub rapid_fire_expires_at: u64,
    /// "Buff window" for shells the ship fires while active. The buff is baked into the
    /// `Shell` at fire time, so the window expiring mid-flight does not de-buff in-flight
    /// shells.
    pub heavy_shell_expires_at: u64,
    /// Same shape as `heavy_shell_expires_at` — applied to outgoing shells at fire time.
    pub long_range_expires_at: u64,
    pub awacs_expires_at: u64,
    pub silent_running_expires_at: u64,
    /// Window during which incoming hits arm/refresh the trace reveal. Non-consuming: every
    /// hit while `world.tick < trace_armed_until` (re)starts the reveal track below.
    pub trace_armed_until: u64,
    /// Tick until which the bot keeps receiving a synthetic precise contact for
    /// `trace_attacker`. Refreshed (not decremented) on each hit during the armed window.
    pub trace_reveal_until: u64,
    /// Ship that last hit this one during the armed window. Drives the reveal track.
    pub trace_attacker: Option<ShipId>,
    /// EMP *debuff* window: this ship has been hit by an enemy `EmpBurst`. While
    /// `world.tick < emp_debuff_until`, the ship's gun cooldown is multiplied by
    /// `emp_gun_cooldown_mult` (stacks with rapid_fire) and its active radar returns no
    /// contacts (passive sensors still work). This is a state *inflicted by an opponent* —
    /// deliberately distinct from `emp_self_expires_at` so a victim's `powerup_status` never
    /// reports its own (unused) `emp_burst` as active just because it got EMP'd.
    pub emp_debuff_until: u64,
    /// This ship's *own* `EmpBurst` activation window, used only for `powerup_status`
    /// reporting (so the activator sees a countdown like every other powerup). `EmpBurst` is
    /// offensive — it debuffs enemies, not the caster — so this window has no gameplay effect
    /// on the caster; the debuff lives in `emp_debuff_until` on each target.
    pub emp_self_expires_at: u64,
}

impl PowerupState {
    /// Drop all transient state. Called at round reset; selections are preserved per the
    /// design (bots keep the loadout they picked at match start), used-list is cleared so
    /// each powerup is fresh again.
    pub fn reset_for_round(&mut self) {
        self.used.clear();
        self.overdrive_expires_at = 0;
        self.reinforced_hull_expires_at = 0;
        self.repair_drones_expires_at = 0;
        self.rapid_fire_expires_at = 0;
        self.heavy_shell_expires_at = 0;
        self.long_range_expires_at = 0;
        self.awacs_expires_at = 0;
        self.silent_running_expires_at = 0;
        self.trace_armed_until = 0;
        self.trace_reveal_until = 0;
        self.trace_attacker = None;
        self.emp_debuff_until = 0;
        self.emp_self_expires_at = 0;
    }

    /// Whether `id` is currently active for this ship at `tick`.
    pub fn is_active(&self, id: PowerupId, tick: u64) -> bool {
        let expires = match id {
            PowerupId::Overdrive => self.overdrive_expires_at,
            PowerupId::ReinforcedHull => self.reinforced_hull_expires_at,
            PowerupId::RepairDrones => self.repair_drones_expires_at,
            PowerupId::RapidFire => self.rapid_fire_expires_at,
            PowerupId::HeavyShell => self.heavy_shell_expires_at,
            PowerupId::LongRangeSalvo => self.long_range_expires_at,
            PowerupId::AwacsScan => self.awacs_expires_at,
            PowerupId::SilentRunning => self.silent_running_expires_at,
            PowerupId::CounterBatteryTrace => self.trace_armed_until,
            // The *own-activation* window (not the enemy-inflicted debuff, which lives in
            // `emp_debuff_until` and is read via `is_emp_debuffed`).
            PowerupId::EmpBurst => self.emp_self_expires_at,
            // Smoke screen / decoy flare are world-level, not ship-level — they're "active"
            // for the activating bot's purposes once placed, but the live entity lives on
            // World, not Ship. The room reads them off the world when building tick state.
            PowerupId::SmokeScreen | PowerupId::DecoyFlare => 0,
        };
        expires > tick
    }

    /// Whether this ship is currently EMP-*debuffed* by an enemy `EmpBurst` at `tick`. This
    /// is the gameplay effect (gun-cooldown penalty + active-radar blackout); it is separate
    /// from `is_active(EmpBurst, …)`, which reports the ship's *own* powerup for status.
    pub fn is_emp_debuffed(&self, tick: u64) -> bool {
        self.emp_debuff_until > tick
    }

    /// Ticks remaining until `id` expires, or `0` if not active.
    pub fn ticks_remaining(&self, id: PowerupId, tick: u64) -> u32 {
        let expires = match id {
            PowerupId::Overdrive => self.overdrive_expires_at,
            PowerupId::ReinforcedHull => self.reinforced_hull_expires_at,
            PowerupId::RepairDrones => self.repair_drones_expires_at,
            PowerupId::RapidFire => self.rapid_fire_expires_at,
            PowerupId::HeavyShell => self.heavy_shell_expires_at,
            PowerupId::LongRangeSalvo => self.long_range_expires_at,
            PowerupId::AwacsScan => self.awacs_expires_at,
            PowerupId::SilentRunning => self.silent_running_expires_at,
            PowerupId::CounterBatteryTrace => self.trace_armed_until,
            PowerupId::EmpBurst => self.emp_self_expires_at,
            PowerupId::SmokeScreen | PowerupId::DecoyFlare => 0,
        };
        if expires > tick {
            (expires - tick) as u32
        } else {
            0
        }
    }
}

/// Reasons an activation request can be refused. Translated into a typed `error` by the
/// room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationError {
    /// The ship was not registered in the world (e.g. mid-disconnect).
    UnknownShip,
    /// The ship is dead — corpses don't activate powerups.
    ShipDead,
    /// The bot never picked this powerup for the match.
    NotSelected,
    /// The bot picked this powerup but already activated it earlier in this match.
    AlreadyUsed,
}

impl ActivationError {
    pub fn as_str(&self) -> &'static str {
        match self {
            ActivationError::UnknownShip => "unknown ship",
            ActivationError::ShipDead => "ship is dead",
            ActivationError::NotSelected => "powerup not in this bot's loadout",
            ActivationError::AlreadyUsed => "powerup already activated this match",
        }
    }
}

/// Activate `id` for `ship_id`. Applies the relevant effect, marks the powerup as used,
/// and (for AoE powerups) mutates the world. Returns `Ok(())` on success.
///
/// `rng` is the room's seeded `Pcg64`. Only `decoy_flare` draws from it (one draw, for the
/// jittered spawn distance); keeping every other arm draw-free preserves the deterministic
/// per-tick RNG stream the sensor pass relies on.
pub fn activate(
    world: &mut World,
    ship_id: &ShipId,
    id: PowerupId,
    rng: &mut Pcg64,
) -> Result<(), ActivationError> {
    let tick = world.tick;
    let config = world.config.powerups;
    let hull_hp = world.config.hull_hp;

    // Validate the activator first without taking long-lived borrows on `world.ships`.
    let activator_pos;
    let activator_heading;
    let activator_speed;
    {
        let ship = world
            .ships
            .get(ship_id)
            .ok_or(ActivationError::UnknownShip)?;
        if !ship.alive {
            return Err(ActivationError::ShipDead);
        }
        if !ship.powerups.selected.contains(&id) {
            return Err(ActivationError::NotSelected);
        }
        if ship.powerups.used.contains(&id) {
            return Err(ActivationError::AlreadyUsed);
        }
        activator_pos = ship.pos;
        activator_heading = ship.heading_deg;
        activator_speed = ship.speed;
    }

    // World-level effects (read positions of other ships first, then take a single mutable
    // borrow on the activator to update its state).
    match id {
        PowerupId::EmpBurst => {
            // Snapshot enemies in range in BotId-stable (BTreeMap) order. We mutate other
            // ships' powerup state — keep the activator out of the loop.
            let activator_bot_id = world
                .ships
                .get(ship_id)
                .expect("activator present")
                .bot_id
                .clone();
            let radius = config.emp_burst_radius;
            let targets: Vec<ShipId> = world
                .ships
                .iter()
                .filter(|(_, ship)| {
                    ship.alive
                        && ship.bot_id != activator_bot_id
                        && ship.pos.distance(activator_pos) <= radius
                })
                .map(|(id, _)| id.clone())
                .collect();
            let emp_expires = tick + config.emp_burst_duration_ticks as u64;
            for target in &targets {
                if let Some(target_ship) = world.ships.get_mut(target) {
                    // Inflict the debuff on the *target*, never the caster.
                    if target_ship.powerups.emp_debuff_until < emp_expires {
                        target_ship.powerups.emp_debuff_until = emp_expires;
                    }
                }
            }
            // Record the caster's own activation window purely for `powerup_status` (the
            // caster is never EMP-debuffed by its own burst).
            if let Some(caster) = world.ships.get_mut(ship_id) {
                caster.powerups.emp_self_expires_at = emp_expires;
            }
        }
        PowerupId::SmokeScreen => {
            world.smoke_clouds.push(SmokeCloud {
                pos: activator_pos,
                radius: config.smoke_screen_radius,
                expires_at: tick + config.smoke_screen_duration_ticks as u64,
            });
        }
        PowerupId::DecoyFlare => {
            // Project a phantom a seeded-jittered distance along the activator's current
            // heading, then have it cruise at the activator's heading/speed so it can't be
            // filtered out as a motionless contact. Compass heading: 0° = north (-y), 90° = east (+x).
            // This is the only RNG draw `activate` makes — keep it a single, unconditional draw.
            let r = activator_heading.to_radians();
            let dir = Vec2::new(r.sin(), -r.cos());
            let dist =
                rng.gen_range(config.decoy_flare_distance_min..=config.decoy_flare_distance_max);
            let pos = activator_pos + dir * dist;
            let vel = dir * activator_speed;
            let fake_id = world.next_decoy_index;
            world.next_decoy_index = world.next_decoy_index.wrapping_add(1);
            world.decoys.push(Decoy {
                fake_id,
                owner: ship_id.clone(),
                pos,
                heading_deg: activator_heading,
                vel,
                expires_at: tick + config.decoy_flare_duration_ticks as u64,
            });
        }
        _ => {}
    }

    // Per-ship effect bookkeeping on the activator.
    let ship = world
        .ships
        .get_mut(ship_id)
        .expect("activator still present");
    match id {
        PowerupId::Overdrive => {
            ship.powerups.overdrive_expires_at = tick + config.overdrive_duration_ticks as u64;
        }
        PowerupId::ReinforcedHull => {
            ship.powerups.reinforced_hull_expires_at =
                tick + config.reinforced_hull_duration_ticks as u64;
        }
        PowerupId::RepairDrones => {
            // Front-loaded burst on activation, then per-tick regen via step_tick_maintenance.
            ship.hp = ship
                .hp
                .saturating_add(config.repair_drones_instant_hp)
                .min(hull_hp);
            ship.powerups.repair_drones_expires_at =
                tick + config.repair_drones_duration_ticks as u64;
        }
        PowerupId::RapidFire => {
            ship.powerups.rapid_fire_expires_at = tick + config.rapid_fire_duration_ticks as u64;
        }
        PowerupId::HeavyShell => {
            ship.powerups.heavy_shell_expires_at = tick + config.heavy_shell_duration_ticks as u64;
        }
        PowerupId::LongRangeSalvo => {
            ship.powerups.long_range_expires_at = tick + config.long_range_duration_ticks as u64;
        }
        PowerupId::AwacsScan => {
            ship.powerups.awacs_expires_at = tick + config.awacs_duration_ticks as u64;
        }
        PowerupId::SilentRunning => {
            ship.powerups.silent_running_expires_at =
                tick + config.silent_running_duration_ticks as u64;
        }
        PowerupId::CounterBatteryTrace => {
            ship.powerups.trace_armed_until = tick + config.counter_battery_arm_ticks as u64;
            ship.powerups.trace_reveal_until = 0;
            ship.powerups.trace_attacker = None;
        }
        // AoE effects already mutated the world above; mark used and exit.
        PowerupId::EmpBurst | PowerupId::SmokeScreen | PowerupId::DecoyFlare => {}
    }
    ship.powerups.used.insert(id);
    Ok(())
}

/// End-of-tick maintenance: regen HP for ships with `repair_drones` active, advance cruising
/// decoys, and garbage-collect expired smoke clouds and decoys. Call after physics+combat but
/// before the per-bot tick payload is built so the bots see fresh state.
pub fn step_tick_maintenance(world: &mut World) {
    let tick = world.tick;
    let hp_per_tick = world.config.powerups.repair_drones_hp_per_tick;
    let max_hp = world.config.hull_hp;
    for ship in world.ships.values_mut() {
        if !ship.alive {
            continue;
        }
        if ship.powerups.repair_drones_expires_at > tick && hp_per_tick > 0 {
            ship.hp = ship.hp.saturating_add(hp_per_tick).min(max_hp);
        }
    }
    // Decoys cruise at their inherited velocity (fixed dt; no wall clock).
    for decoy in world.decoys.iter_mut() {
        decoy.pos += decoy.vel * DT;
    }
    world.smoke_clouds.retain(|c| c.expires_at > tick);
    world.decoys.retain(|d| d.expires_at > tick);
}

// ---------------------------------------------------------------------------
// Effect helpers used by physics / combat / sensors.
// ---------------------------------------------------------------------------

/// Effective max forward speed for a ship at the current tick. Reads `world.config` and
/// the ship's powerup state; never the wall clock.
pub fn effective_max_forward_speed(
    base: f32,
    state: &PowerupState,
    config: &PowerupConfig,
    tick: u64,
) -> f32 {
    if state.is_active(PowerupId::Overdrive, tick) {
        base * config.overdrive_speed_mult
    } else {
        base
    }
}

pub fn effective_acceleration(
    base: f32,
    state: &PowerupState,
    config: &PowerupConfig,
    tick: u64,
) -> f32 {
    if state.is_active(PowerupId::Overdrive, tick) {
        base * config.overdrive_accel_mult
    } else {
        base
    }
}

pub fn effective_turn_rate(
    base: f32,
    state: &PowerupState,
    config: &PowerupConfig,
    tick: u64,
) -> f32 {
    if state.is_active(PowerupId::Overdrive, tick) {
        base * config.overdrive_turn_mult
    } else {
        base
    }
}

/// Effective gun cooldown for the firing ship right now. Rapid fire and EMP both apply
/// multiplicatively — EMP slows you down even if you're rapid-firing through it. Result
/// is clamped to at least 1 tick so a poorly-tuned config can't yield a 0-cooldown gun.
pub fn effective_gun_cooldown_ticks(
    base: u32,
    state: &PowerupState,
    config: &PowerupConfig,
    tick: u64,
) -> u32 {
    let mut effective = base as f32;
    if state.is_active(PowerupId::RapidFire, tick) {
        effective *= config.rapid_fire_cooldown_mult;
    }
    if state.is_emp_debuffed(tick) {
        effective *= config.emp_gun_cooldown_mult;
    }
    let rounded = effective.round() as i64;
    rounded.max(1) as u32
}

/// Splash radius for a shell that was fired *with* `heavy_shell` active.
pub fn buffed_splash_radius(base: f32, config: &PowerupConfig) -> f32 {
    base * config.heavy_shell_splash_mult
}

/// Max splash damage for a shell that was fired *with* `heavy_shell` active.
pub fn buffed_splash_damage(base: u32, config: &PowerupConfig) -> u32 {
    let scaled = (base as f32 * config.heavy_shell_damage_mult).round() as i64;
    scaled.max(0) as u32
}

/// Shell speed for a shell that was fired *with* `long_range_salvo` active.
pub fn buffed_shell_speed(base: f32, config: &PowerupConfig) -> f32 {
    base * config.long_range_speed_mult
}

/// Max shell range for a shell that was fired *with* `long_range_salvo` active.
pub fn buffed_max_shell_range(base: f32, config: &PowerupConfig) -> f32 {
    base * config.long_range_range_mult
}

/// Apply reinforced-hull damage reduction. Returns the (possibly reduced) damage value to
/// actually subtract from HP. `round` rather than `floor` to keep the boundary cases
/// honest (1 hp at 0.45× still costs 0 hp by design).
pub fn apply_incoming_damage_reduction(
    raw_damage: u32,
    state: &PowerupState,
    config: &PowerupConfig,
    tick: u64,
) -> u32 {
    if state.is_active(PowerupId::ReinforcedHull, tick) {
        let scaled = (raw_damage as f32 * config.reinforced_hull_damage_mult).round() as i64;
        scaled.max(0) as u32
    } else {
        raw_damage
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::config::SimConfig;
    use crate::sim::world::Ship;
    use rand::SeedableRng;

    fn world_with(ships: Vec<Ship>) -> World {
        let mut w = World::new(1000.0, 1000.0, SimConfig::default());
        for s in ships {
            w.insert_ship(s);
        }
        w
    }

    /// Seeded RNG for tests that call `activate` (only `decoy_flare` actually draws).
    fn test_rng() -> Pcg64 {
        Pcg64::seed_from_u64(42)
    }

    fn ship_with_loadout(id: &str, loadout: &[PowerupId]) -> Ship {
        let mut s = Ship::new_at(id.into(), format!("b_{id}"), Vec2::new(500.0, 500.0), 0.0);
        s.powerups.selected = loadout.to_vec();
        s
    }

    #[test]
    fn powerup_id_roundtrips_through_wire_string() {
        for id in PowerupId::all() {
            let s = id.as_str();
            assert_eq!(PowerupId::parse(s), Some(*id));
        }
        assert_eq!(PowerupId::parse("nuke"), None);
    }

    #[test]
    fn activation_rejected_when_not_selected() {
        let mut world = world_with(vec![ship_with_loadout(
            "s_1",
            &[PowerupId::Overdrive, PowerupId::RapidFire],
        )]);
        let err = activate(
            &mut world,
            &"s_1".into(),
            PowerupId::SmokeScreen,
            &mut test_rng(),
        )
        .unwrap_err();
        assert_eq!(err, ActivationError::NotSelected);
    }

    #[test]
    fn activation_rejected_when_already_used() {
        let mut world = world_with(vec![ship_with_loadout(
            "s_1",
            &[PowerupId::Overdrive, PowerupId::RapidFire],
        )]);
        activate(
            &mut world,
            &"s_1".into(),
            PowerupId::Overdrive,
            &mut test_rng(),
        )
        .expect("first activate");
        let err = activate(
            &mut world,
            &"s_1".into(),
            PowerupId::Overdrive,
            &mut test_rng(),
        )
        .unwrap_err();
        assert_eq!(err, ActivationError::AlreadyUsed);
    }

    #[test]
    fn overdrive_sets_expiry_and_decays() {
        let mut world = world_with(vec![ship_with_loadout(
            "s_1",
            &[PowerupId::Overdrive, PowerupId::RapidFire],
        )]);
        activate(
            &mut world,
            &"s_1".into(),
            PowerupId::Overdrive,
            &mut test_rng(),
        )
        .expect("activate");
        let dur = world.config.powerups.overdrive_duration_ticks as u64;
        let state = &world.ships.get("s_1").unwrap().powerups;
        assert_eq!(state.overdrive_expires_at, dur);
        assert!(state.is_active(PowerupId::Overdrive, 0));
        // At tick = expiry, no longer active.
        assert!(!state.is_active(PowerupId::Overdrive, dur));
    }

    #[test]
    fn smoke_screen_spawns_world_cloud() {
        let mut world = world_with(vec![ship_with_loadout(
            "s_1",
            &[PowerupId::SmokeScreen, PowerupId::Overdrive],
        )]);
        activate(
            &mut world,
            &"s_1".into(),
            PowerupId::SmokeScreen,
            &mut test_rng(),
        )
        .expect("activate");
        assert_eq!(world.smoke_clouds.len(), 1);
        let cloud = &world.smoke_clouds[0];
        assert_eq!(cloud.pos, Vec2::new(500.0, 500.0));
        assert_eq!(cloud.radius, world.config.powerups.smoke_screen_radius);
    }

    #[test]
    fn emp_burst_marks_enemies_in_range_only() {
        let mut s1 = ship_with_loadout("s_1", &[PowerupId::EmpBurst, PowerupId::Overdrive]);
        let mut s2 = Ship::new_at("s_2".into(), "b_2".into(), Vec2::new(550.0, 500.0), 0.0);
        let mut s3 = Ship::new_at("s_3".into(), "b_3".into(), Vec2::new(800.0, 500.0), 0.0);
        s1.pos = Vec2::new(500.0, 500.0);
        s2.alive = true;
        s3.alive = true;
        let mut world = world_with(vec![s1, s2, s3]);
        activate(
            &mut world,
            &"s_1".into(),
            PowerupId::EmpBurst,
            &mut test_rng(),
        )
        .expect("activate");
        let dur = world.config.powerups.emp_burst_duration_ticks as u64;
        // In-range enemy is debuffed; out-of-range enemy is not.
        assert_eq!(
            world.ships.get("s_2").unwrap().powerups.emp_debuff_until,
            dur
        );
        assert_eq!(world.ships.get("s_3").unwrap().powerups.emp_debuff_until, 0);
        // Activator does not debuff itself...
        assert_eq!(world.ships.get("s_1").unwrap().powerups.emp_debuff_until, 0);
        assert!(!world.ships.get("s_1").unwrap().powerups.is_emp_debuffed(0));
        // ...but records its own activation window for status reporting.
        assert_eq!(
            world.ships.get("s_1").unwrap().powerups.emp_self_expires_at,
            dur
        );
    }

    #[test]
    fn emp_debuff_does_not_mark_victims_own_powerup_active() {
        // F-12: a victim holding an *unused* emp_burst that gets EMP'd must not report its own
        // powerup as active. The debuff (radar/gun) reads emp_debuff_until; powerup_status
        // reads is_active(EmpBurst)/ticks_remaining which key off emp_self_expires_at.
        let mut victim = ship_with_loadout("s_v", &[PowerupId::EmpBurst, PowerupId::Overdrive]);
        victim.powerups.emp_debuff_until = 40; // hit by an enemy EMP through tick 40
                                               // The victim never activated its own emp_burst.
        assert_eq!(victim.powerups.emp_self_expires_at, 0);
        // Own powerup reporting: not active, no ticks remaining.
        assert!(!victim.powerups.is_active(PowerupId::EmpBurst, 0));
        assert_eq!(victim.powerups.ticks_remaining(PowerupId::EmpBurst, 0), 0);
        // But the debuff is in effect (radar blackout / gun penalty).
        assert!(victim.powerups.is_emp_debuffed(0));
        assert!(victim.powerups.is_emp_debuffed(39));
        assert!(!victim.powerups.is_emp_debuffed(40));
    }

    #[test]
    fn dead_ship_cannot_activate() {
        let mut world = world_with(vec![ship_with_loadout(
            "s_1",
            &[PowerupId::Overdrive, PowerupId::RapidFire],
        )]);
        world.ships.get_mut("s_1").unwrap().alive = false;
        let err = activate(
            &mut world,
            &"s_1".into(),
            PowerupId::Overdrive,
            &mut test_rng(),
        )
        .unwrap_err();
        assert_eq!(err, ActivationError::ShipDead);
    }

    #[test]
    fn repair_drones_regenerates_hp_only_while_active() {
        let mut world = world_with(vec![ship_with_loadout(
            "s_1",
            &[PowerupId::RepairDrones, PowerupId::Overdrive],
        )]);
        world.ships.get_mut("s_1").unwrap().hp = 50;
        activate(
            &mut world,
            &"s_1".into(),
            PowerupId::RepairDrones,
            &mut test_rng(),
        )
        .expect("activate");
        let instant = world.config.powerups.repair_drones_instant_hp;
        let per_tick = world.config.powerups.repair_drones_hp_per_tick;
        let dur = world.config.powerups.repair_drones_duration_ticks as u64;
        // Instant burst lands immediately on activation.
        assert_eq!(world.ships.get("s_1").unwrap().hp, 50 + instant);
        // Advance one tick: hp regens by `per_tick` on top of the burst.
        world.tick = 1;
        step_tick_maintenance(&mut world);
        assert_eq!(world.ships.get("s_1").unwrap().hp, 50 + instant + per_tick);
        // Past expiry: no further regen.
        world.tick = dur + 5;
        let before = world.ships.get("s_1").unwrap().hp;
        step_tick_maintenance(&mut world);
        assert_eq!(world.ships.get("s_1").unwrap().hp, before);
    }

    #[test]
    fn smoke_and_decoys_expire_via_maintenance() {
        let mut world = world_with(vec![ship_with_loadout(
            "s_1",
            &[PowerupId::SmokeScreen, PowerupId::DecoyFlare],
        )]);
        activate(
            &mut world,
            &"s_1".into(),
            PowerupId::SmokeScreen,
            &mut test_rng(),
        )
        .expect("smoke");
        activate(
            &mut world,
            &"s_1".into(),
            PowerupId::DecoyFlare,
            &mut test_rng(),
        )
        .expect("decoy");
        assert_eq!(world.smoke_clouds.len(), 1);
        assert_eq!(world.decoys.len(), 1);
        // Tick well past both expiries.
        world.tick = world.config.powerups.smoke_screen_duration_ticks as u64 + 10;
        step_tick_maintenance(&mut world);
        assert!(world.smoke_clouds.is_empty());
        world.tick = world.config.powerups.decoy_flare_duration_ticks as u64 + 10;
        step_tick_maintenance(&mut world);
        assert!(world.decoys.is_empty());
    }

    #[test]
    fn rapid_fire_and_emp_stack_multiplicatively_on_cooldown() {
        let cfg = PowerupConfig::default();
        let state = PowerupState {
            selected: vec![PowerupId::RapidFire, PowerupId::EmpBurst],
            // Activate rapid_fire and inflict an EMP debuff "by hand" to test the cooldown
            // helper in isolation (the gun penalty keys off the debuff window).
            rapid_fire_expires_at: 100,
            emp_debuff_until: 100,
            ..Default::default()
        };
        let base = 15;
        let combined = effective_gun_cooldown_ticks(base, &state, &cfg, 0);
        // 15 * 0.5 * 2.0 = 15. Helper rounds and clamps; expect exactly 15.
        assert_eq!(combined, 15);
    }

    #[test]
    fn long_range_buffs_outgoing_shells_only() {
        use crate::sim::combat;
        let mut world = world_with(vec![ship_with_loadout(
            "s_1",
            &[PowerupId::LongRangeSalvo, PowerupId::Overdrive],
        )]);
        // No buff: shell speed and TTL reflect the base config.
        combat::fire(&mut world, &"s_1".into(), 90.0, 200.0).expect("fire");
        let unbuffed_speed = world.shells[0].vel.length();
        let unbuffed_ttl = world.shells[0].ttl_ticks;
        world.shells.clear();
        // Cooldown / ammo reset so a second fire goes through.
        world.ships.get_mut("s_1").unwrap().gun_cooldown = 0;
        // Activate long_range_salvo and fire — speed should now be boosted.
        activate(
            &mut world,
            &"s_1".into(),
            PowerupId::LongRangeSalvo,
            &mut test_rng(),
        )
        .expect("activate");
        combat::fire(&mut world, &"s_1".into(), 90.0, 200.0).expect("fire");
        let buffed_speed = world.shells[0].vel.length();
        let buffed_ttl = world.shells[0].ttl_ticks;
        assert!(
            buffed_speed > unbuffed_speed + 1.0,
            "long_range should boost shell speed: {unbuffed_speed} -> {buffed_speed}"
        );
        // TTL = ceil(range / (speed * DT)) — buffed speed yields *shorter* TTL for a fixed
        // request range, but the *max* range is also boosted, so a max-range shot flies
        // farther. The simple check: TTL for a 200u shot at higher speed is no greater.
        assert!(
            buffed_ttl <= unbuffed_ttl,
            "buffed TTL should not exceed unbuffed for the same range"
        );
    }

    #[test]
    fn heavy_shell_widens_splash_radius_and_increases_damage() {
        use crate::sim::combat;
        // Splash radius is 15 by default; heavy_shell widens it to 15 * 1.5 = 22.5. Place the
        // target so it sits *inside* the heavy radius but *outside* the normal one, so a
        // heavy shot clips it where a normal shot would do nothing. Splash centre is the
        // impact point, ~203u east of the shooter for a 200u request.
        let mut world = world_with(vec![
            ship_with_loadout("s_1", &[PowerupId::HeavyShell, PowerupId::ReinforcedHull]),
            Ship::new_at("s_2".into(), "b_2".into(), Vec2::new(0.0, 0.0), 0.0),
        ]);
        world.ships.get_mut("s_1").unwrap().pos = Vec2::new(500.0, 500.0);
        // After a 200u request, impact is ~203u east. Place s_2 at impact + 18u east.
        let impact_x = 500.0
            + (200.0_f32 / (world.config.shell_speed * crate::sim::constants::DT)).ceil()
                * world.config.shell_speed
                * crate::sim::constants::DT;
        world.ships.get_mut("s_2").unwrap().pos = Vec2::new(impact_x + 18.0, 500.0);
        // Sanity: 18 > 15 (normal splash) and 18 < 22.5 (heavy splash).
        activate(
            &mut world,
            &"s_1".into(),
            PowerupId::HeavyShell,
            &mut test_rng(),
        )
        .expect("heavy");
        combat::fire(&mut world, &"s_1".into(), 90.0, 200.0).expect("fire");
        while !world.shells.is_empty() {
            combat::step_shells(&mut world);
        }
        // Heavy shell damage at 18/22.5 of the way out: frac = 1 - 18/22.5 = 0.2; base damage
        // is buffed to 25 * 1.3 = 32.5 → 33; 0.2 * 33 ≈ 6.6 → 7 hp lost.
        let s2_hp = world.ships.get("s_2").unwrap().hp;
        assert!(
            s2_hp < crate::sim::constants::HULL_HP,
            "target outside normal splash but inside heavy splash should be hit; hp={s2_hp}"
        );
    }

    #[test]
    fn reinforced_hull_scales_incoming_damage() {
        let cfg = PowerupConfig::default();
        let mut state = PowerupState {
            selected: vec![PowerupId::ReinforcedHull],
            reinforced_hull_expires_at: 100,
            ..Default::default()
        };
        let reduced = apply_incoming_damage_reduction(25, &state, &cfg, 0);
        // 25 * 0.45 = 11.25, rounds to 11.
        assert_eq!(reduced, 11);
        // Without the effect, raw damage passes through.
        state.reinforced_hull_expires_at = 0;
        assert_eq!(apply_incoming_damage_reduction(25, &state, &cfg, 5), 25);
    }
}
