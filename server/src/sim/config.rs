//! Per-match simulation parameters.
//!
//! Historically every tunable lived as a `const` in [`super::constants`]. Those constants
//! are still the single source of *default* values, but a match now carries its own
//! [`SimConfig`] so an operator can rebalance ship health, shell speed, sensor ranges,
//! etc. from the pre-match screen.
//!
//! Determinism contract (see `CLAUDE.md`): the config is frozen when the match starts and
//! never mutated while `Running`. It is recorded in the replay header so a replay rebuilds
//! the simulation with the exact parameters the live run used. The fixed physics timestep
//! `DT` is deliberately *not* configurable — it paces nothing and rebalances everything.

use serde::{Deserialize, Serialize};

use super::constants;

/// Tunable balance parameters for a single match. All fields default to the values in
/// [`super::constants`]; a fresh `SimConfig::default()` reproduces the legacy behaviour
/// byte-for-byte.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SimConfig {
    // --- Ship ---------------------------------------------------------------
    pub max_forward_speed: f32,
    pub max_reverse_speed: f32,
    pub acceleration: f32,
    pub turn_rate_deg_per_s: f32,
    pub hull_hp: u32,
    pub max_ammo: u32,
    pub gun_cooldown_ticks: u32,
    pub hit_radius: f32,
    // --- Weapons ------------------------------------------------------------
    pub shell_speed: f32,
    pub max_shell_range: f32,
    pub splash_radius: f32,
    pub max_splash_damage: u32,
    // --- Sensors ------------------------------------------------------------
    pub active_radar_range: f32,
    pub active_radar_noise: f32,
    pub passive_hear_active_range: f32,
    pub passive_hear_nearby_range: f32,
    pub passive_bearing_noise_deg: f32,
    // --- World --------------------------------------------------------------
    pub wall_bump_damage: u32,
    // --- Powerups -----------------------------------------------------------
    /// Per-powerup tuning. See `docs/POWERUPS.md` for the catalog.
    #[serde(default)]
    pub powerups: PowerupConfig,
}

/// Tuning for the one-off powerups bots can pick before a match. Every field has a sensible
/// default (see [`super::constants`]) so a `PowerupConfig::default()` matches the published
/// behaviour. Operators can rebalance individual powerups via `PUT /api/room/config`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PowerupConfig {
    // Overdrive
    pub overdrive_duration_ticks: u32,
    pub overdrive_speed_mult: f32,
    pub overdrive_accel_mult: f32,
    pub overdrive_turn_mult: f32,
    // Reinforced hull
    pub reinforced_hull_duration_ticks: u32,
    pub reinforced_hull_damage_mult: f32,
    // Repair drones
    pub repair_drones_duration_ticks: u32,
    pub repair_drones_hp_per_tick: u32,
    // Smoke screen
    pub smoke_screen_duration_ticks: u32,
    pub smoke_screen_radius: f32,
    // Rapid fire
    pub rapid_fire_duration_ticks: u32,
    pub rapid_fire_cooldown_mult: f32,
    // Heavy shell
    pub heavy_shell_duration_ticks: u32,
    pub heavy_shell_splash_mult: f32,
    pub heavy_shell_damage_mult: f32,
    // Long-range salvo
    pub long_range_duration_ticks: u32,
    pub long_range_range_mult: f32,
    pub long_range_speed_mult: f32,
    // AWACS scan
    pub awacs_duration_ticks: u32,
    pub awacs_range_mult: f32,
    // Silent running
    pub silent_running_duration_ticks: u32,
    pub silent_running_active_range_mult: f32,
    // Counter-battery trace
    pub counter_battery_arm_ticks: u32,
    pub counter_battery_reveal_ticks: u8,
    // EMP burst
    pub emp_burst_duration_ticks: u32,
    pub emp_burst_radius: f32,
    pub emp_gun_cooldown_mult: f32,
    // Decoy flare
    pub decoy_flare_duration_ticks: u32,
    pub decoy_flare_distance: f32,
}

impl Default for PowerupConfig {
    fn default() -> Self {
        Self {
            overdrive_duration_ticks: constants::OVERDRIVE_DURATION_TICKS,
            overdrive_speed_mult: constants::OVERDRIVE_SPEED_MULT,
            overdrive_accel_mult: constants::OVERDRIVE_ACCEL_MULT,
            overdrive_turn_mult: constants::OVERDRIVE_TURN_MULT,
            reinforced_hull_duration_ticks: constants::REINFORCED_HULL_DURATION_TICKS,
            reinforced_hull_damage_mult: constants::REINFORCED_HULL_DAMAGE_MULT,
            repair_drones_duration_ticks: constants::REPAIR_DRONES_DURATION_TICKS,
            repair_drones_hp_per_tick: constants::REPAIR_DRONES_HP_PER_TICK,
            smoke_screen_duration_ticks: constants::SMOKE_SCREEN_DURATION_TICKS,
            smoke_screen_radius: constants::SMOKE_SCREEN_RADIUS,
            rapid_fire_duration_ticks: constants::RAPID_FIRE_DURATION_TICKS,
            rapid_fire_cooldown_mult: constants::RAPID_FIRE_COOLDOWN_MULT,
            heavy_shell_duration_ticks: constants::HEAVY_SHELL_DURATION_TICKS,
            heavy_shell_splash_mult: constants::HEAVY_SHELL_SPLASH_MULT,
            heavy_shell_damage_mult: constants::HEAVY_SHELL_DAMAGE_MULT,
            long_range_duration_ticks: constants::LONG_RANGE_DURATION_TICKS,
            long_range_range_mult: constants::LONG_RANGE_RANGE_MULT,
            long_range_speed_mult: constants::LONG_RANGE_SPEED_MULT,
            awacs_duration_ticks: constants::AWACS_DURATION_TICKS,
            awacs_range_mult: constants::AWACS_RANGE_MULT,
            silent_running_duration_ticks: constants::SILENT_RUNNING_DURATION_TICKS,
            silent_running_active_range_mult: constants::SILENT_RUNNING_ACTIVE_RANGE_MULT,
            counter_battery_arm_ticks: constants::COUNTER_BATTERY_ARM_TICKS,
            counter_battery_reveal_ticks: constants::COUNTER_BATTERY_REVEAL_TICKS,
            emp_burst_duration_ticks: constants::EMP_BURST_DURATION_TICKS,
            emp_burst_radius: constants::EMP_BURST_RADIUS,
            emp_gun_cooldown_mult: constants::EMP_GUN_COOLDOWN_MULT,
            decoy_flare_duration_ticks: constants::DECOY_FLARE_DURATION_TICKS,
            decoy_flare_distance: constants::DECOY_FLARE_DISTANCE,
        }
    }
}

impl PowerupConfig {
    /// Validate that durations are non-zero and multipliers/radii are finite. Generous
    /// bounds — the goal is to reject NaN/zero, not to enforce balance.
    fn validate(&self) -> Result<(), String> {
        check_count(
            "powerups.overdrive_duration_ticks",
            self.overdrive_duration_ticks,
            1,
            1_000_000,
        )?;
        check_positive(
            "powerups.overdrive_speed_mult",
            self.overdrive_speed_mult,
            1_000.0,
        )?;
        check_positive(
            "powerups.overdrive_accel_mult",
            self.overdrive_accel_mult,
            1_000.0,
        )?;
        check_positive(
            "powerups.overdrive_turn_mult",
            self.overdrive_turn_mult,
            1_000.0,
        )?;
        check_count(
            "powerups.reinforced_hull_duration_ticks",
            self.reinforced_hull_duration_ticks,
            1,
            1_000_000,
        )?;
        check_non_negative(
            "powerups.reinforced_hull_damage_mult",
            self.reinforced_hull_damage_mult,
            1_000.0,
        )?;
        check_count(
            "powerups.repair_drones_duration_ticks",
            self.repair_drones_duration_ticks,
            1,
            1_000_000,
        )?;
        check_count(
            "powerups.repair_drones_hp_per_tick",
            self.repair_drones_hp_per_tick,
            0,
            1_000_000,
        )?;
        check_count(
            "powerups.smoke_screen_duration_ticks",
            self.smoke_screen_duration_ticks,
            1,
            1_000_000,
        )?;
        check_positive(
            "powerups.smoke_screen_radius",
            self.smoke_screen_radius,
            1_000_000.0,
        )?;
        check_count(
            "powerups.rapid_fire_duration_ticks",
            self.rapid_fire_duration_ticks,
            1,
            1_000_000,
        )?;
        check_positive(
            "powerups.rapid_fire_cooldown_mult",
            self.rapid_fire_cooldown_mult,
            1_000.0,
        )?;
        check_count(
            "powerups.heavy_shell_duration_ticks",
            self.heavy_shell_duration_ticks,
            1,
            1_000_000,
        )?;
        check_positive(
            "powerups.heavy_shell_splash_mult",
            self.heavy_shell_splash_mult,
            1_000.0,
        )?;
        check_positive(
            "powerups.heavy_shell_damage_mult",
            self.heavy_shell_damage_mult,
            1_000.0,
        )?;
        check_count(
            "powerups.long_range_duration_ticks",
            self.long_range_duration_ticks,
            1,
            1_000_000,
        )?;
        check_positive(
            "powerups.long_range_range_mult",
            self.long_range_range_mult,
            1_000.0,
        )?;
        check_positive(
            "powerups.long_range_speed_mult",
            self.long_range_speed_mult,
            1_000.0,
        )?;
        check_count(
            "powerups.awacs_duration_ticks",
            self.awacs_duration_ticks,
            1,
            1_000_000,
        )?;
        check_positive("powerups.awacs_range_mult", self.awacs_range_mult, 1_000.0)?;
        check_count(
            "powerups.silent_running_duration_ticks",
            self.silent_running_duration_ticks,
            1,
            1_000_000,
        )?;
        check_non_negative(
            "powerups.silent_running_active_range_mult",
            self.silent_running_active_range_mult,
            1_000.0,
        )?;
        check_count(
            "powerups.counter_battery_arm_ticks",
            self.counter_battery_arm_ticks,
            1,
            1_000_000,
        )?;
        if self.counter_battery_reveal_ticks == 0 {
            return Err("powerups.counter_battery_reveal_ticks must be at least 1".into());
        }
        check_count(
            "powerups.emp_burst_duration_ticks",
            self.emp_burst_duration_ticks,
            1,
            1_000_000,
        )?;
        check_positive(
            "powerups.emp_burst_radius",
            self.emp_burst_radius,
            1_000_000.0,
        )?;
        check_positive(
            "powerups.emp_gun_cooldown_mult",
            self.emp_gun_cooldown_mult,
            1_000.0,
        )?;
        check_count(
            "powerups.decoy_flare_duration_ticks",
            self.decoy_flare_duration_ticks,
            1,
            1_000_000,
        )?;
        check_positive(
            "powerups.decoy_flare_distance",
            self.decoy_flare_distance,
            1_000_000.0,
        )?;
        Ok(())
    }
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            max_forward_speed: constants::MAX_FORWARD_SPEED,
            max_reverse_speed: constants::MAX_REVERSE_SPEED,
            acceleration: constants::ACCELERATION,
            turn_rate_deg_per_s: constants::TURN_RATE_DEG_PER_S,
            hull_hp: constants::HULL_HP,
            max_ammo: constants::MAX_AMMO,
            gun_cooldown_ticks: constants::GUN_COOLDOWN_TICKS,
            hit_radius: constants::HIT_RADIUS,
            shell_speed: constants::SHELL_SPEED,
            max_shell_range: constants::MAX_SHELL_RANGE,
            splash_radius: constants::SPLASH_RADIUS,
            max_splash_damage: constants::MAX_SPLASH_DAMAGE,
            active_radar_range: constants::ACTIVE_RADAR_RANGE,
            active_radar_noise: constants::ACTIVE_RADAR_NOISE,
            passive_hear_active_range: constants::PASSIVE_HEAR_ACTIVE_RANGE,
            passive_hear_nearby_range: constants::PASSIVE_HEAR_NEARBY_RANGE,
            passive_bearing_noise_deg: constants::PASSIVE_BEARING_NOISE_DEG,
            wall_bump_damage: constants::WALL_BUMP_DAMAGE,
            powerups: PowerupConfig::default(),
        }
    }
}

impl SimConfig {
    /// Validate operator-supplied parameters. Returns a human-readable reason on the first
    /// field that fails. Bounds are deliberately generous — the goal is to reject values
    /// that would break the simulation (non-finite, zero, absurdly large), not to enforce
    /// balance.
    pub fn validate(&self) -> Result<(), String> {
        check_positive("max_forward_speed", self.max_forward_speed, 1_000.0)?;
        check_positive("max_reverse_speed", self.max_reverse_speed, 1_000.0)?;
        check_positive("acceleration", self.acceleration, 10_000.0)?;
        check_positive("turn_rate_deg_per_s", self.turn_rate_deg_per_s, 36_000.0)?;
        check_positive("hit_radius", self.hit_radius, 10_000.0)?;
        check_positive("shell_speed", self.shell_speed, 100_000.0)?;
        check_positive("max_shell_range", self.max_shell_range, 1_000_000.0)?;
        check_positive("splash_radius", self.splash_radius, 1_000_000.0)?;
        check_positive("active_radar_range", self.active_radar_range, 1_000_000.0)?;
        check_positive(
            "passive_hear_active_range",
            self.passive_hear_active_range,
            1_000_000.0,
        )?;
        check_positive(
            "passive_hear_nearby_range",
            self.passive_hear_nearby_range,
            1_000_000.0,
        )?;
        check_non_negative("active_radar_noise", self.active_radar_noise, 1_000_000.0)?;
        check_non_negative(
            "passive_bearing_noise_deg",
            self.passive_bearing_noise_deg,
            180.0,
        )?;
        check_count("hull_hp", self.hull_hp, 1, 10_000_000)?;
        check_count("max_ammo", self.max_ammo, 1, 10_000_000)?;
        check_count("gun_cooldown_ticks", self.gun_cooldown_ticks, 1, 1_000_000)?;
        check_count("max_splash_damage", self.max_splash_damage, 1, 10_000_000)?;
        check_count("wall_bump_damage", self.wall_bump_damage, 0, 10_000_000)?;
        self.powerups.validate()?;
        Ok(())
    }
}

fn check_positive(name: &str, value: f32, max: f32) -> Result<(), String> {
    if !value.is_finite() {
        return Err(format!("{name} must be a finite number"));
    }
    if value <= 0.0 {
        return Err(format!("{name} must be greater than zero"));
    }
    if value > max {
        return Err(format!("{name} must not exceed {max}"));
    }
    Ok(())
}

fn check_non_negative(name: &str, value: f32, max: f32) -> Result<(), String> {
    if !value.is_finite() {
        return Err(format!("{name} must be a finite number"));
    }
    if value < 0.0 {
        return Err(format!("{name} must not be negative"));
    }
    if value > max {
        return Err(format!("{name} must not exceed {max}"));
    }
    Ok(())
}

fn check_count(name: &str, value: u32, min: u32, max: u32) -> Result<(), String> {
    if value < min {
        return Err(format!("{name} must be at least {min}"));
    }
    if value > max {
        return Err(format!("{name} must not exceed {max}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_constants() {
        let cfg = SimConfig::default();
        assert_eq!(cfg.hull_hp, constants::HULL_HP);
        assert_eq!(cfg.shell_speed, constants::SHELL_SPEED);
        assert_eq!(cfg.max_ammo, constants::MAX_AMMO);
    }

    #[test]
    fn default_is_valid() {
        assert!(SimConfig::default().validate().is_ok());
    }

    #[test]
    fn rejects_zero_and_non_finite() {
        assert!(SimConfig {
            hull_hp: 0,
            ..SimConfig::default()
        }
        .validate()
        .is_err());
        assert!(SimConfig {
            shell_speed: f32::NAN,
            ..SimConfig::default()
        }
        .validate()
        .is_err());
        assert!(SimConfig {
            max_forward_speed: -1.0,
            ..SimConfig::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn roundtrips_through_json() {
        let cfg = SimConfig::default();
        let json = serde_json::to_string(&cfg).expect("serialize");
        let parsed: SimConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cfg, parsed);
    }
}
