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
