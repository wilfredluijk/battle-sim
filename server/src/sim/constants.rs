//! Simulation constants — the single source of truth for ship and weapon balance.
//!
//! `protocol::ShipSpecs::DEFAULT` is derived directly from these constants, so the
//! `welcome` payload always reflects what the simulator actually does. The
//! `system-design.md` ship/weapon tables track these values, but the runtime authority
//! lives here.

/// Fixed simulation timestep. The wall clock is only used to *pace* the tick loop, never to
/// drive physics — see `CLAUDE.md` "Determinism in the simulation".
pub const DT: f32 = 0.1;

// --- Ship (§5.2) -----------------------------------------------------------

pub const MAX_FORWARD_SPEED: f32 = 9.0;
pub const MAX_REVERSE_SPEED: f32 = 2.0;
pub const ACCELERATION: f32 = 3.5;
/// Yaw rate at full rudder and full forward speed. Scales linearly with `|speed| / max_forward`.
pub const TURN_RATE_DEG_PER_S: f32 = 20.0;
pub const HULL_HP: u32 = 100;
pub const MAX_AMMO: u32 = 250;
pub const GUN_COOLDOWN_TICKS: u32 = 15;
pub const HIT_RADIUS: f32 = 8.0;

// --- Weapons (§5.4) --------------------------------------------------------

pub const SHELL_SPEED: f32 = 70.0;
pub const MAX_SHELL_RANGE: f32 = 300.0;
pub const SPLASH_RADIUS: f32 = 15.0;
pub const MAX_SPLASH_DAMAGE: u32 = 25;

// --- Sensors (§5.3) --------------------------------------------------------

/// Radius within which an active radar pings register a contact.
pub const ACTIVE_RADAR_RANGE: f32 = 350.0;
/// Half-width of the uniform position noise applied to active-radar contacts (units).
pub const ACTIVE_RADAR_NOISE: f32 = 2.0;

/// Range at which a passive listener can hear a ship that is currently pinging.
pub const PASSIVE_HEAR_ACTIVE_RANGE: f32 = 500.0;
/// Range at which a passive listener can hear *any* ship (engine noise).
pub const PASSIVE_HEAR_NEARBY_RANGE: f32 = 150.0;
/// Half-width of the uniform bearing noise applied to passive contacts (degrees).
pub const PASSIVE_BEARING_NOISE_DEG: f32 = 5.0;
/// Placeholder distance used to project a bearing-only contact onto a `pos` so the wire
/// frame keeps a consistent shape. Bots get `range = None` so this isn't a real estimate.
pub const PASSIVE_CONTACT_PLACEHOLDER_DISTANCE: f32 = 100.0;

// --- World ----------------------------------------------------------------

/// HP cost when a ship slams into a wall.
pub const WALL_BUMP_DAMAGE: u32 = 2;

// --- Powerups --------------------------------------------------------------
// Defaults for [`super::config::PowerupConfig`]. See `docs/POWERUPS.md` for what each
// powerup does. Durations are in *ticks* — at the default `tick_hz = 10`, 50 ticks = 5 s.

// Overdrive: speed/accel/turn boost.
pub const OVERDRIVE_DURATION_TICKS: u32 = 50;
pub const OVERDRIVE_SPEED_MULT: f32 = 1.6;
pub const OVERDRIVE_ACCEL_MULT: f32 = 1.6;
pub const OVERDRIVE_TURN_MULT: f32 = 1.5;

// Reinforced hull: incoming splash damage scaled down.
pub const REINFORCED_HULL_DURATION_TICKS: u32 = 70;
pub const REINFORCED_HULL_DAMAGE_MULT: f32 = 0.45;

// Repair drones: instant burst on activation, then per-tick regen for a window.
pub const REPAIR_DRONES_DURATION_TICKS: u32 = 50;
pub const REPAIR_DRONES_HP_PER_TICK: u32 = 1;
/// HP healed immediately on activation, before per-tick regen begins.
pub const REPAIR_DRONES_INSTANT_HP: u32 = 20;

// Smoke screen: static AoE cloud that blocks active radar lines of sight from outside.
pub const SMOKE_SCREEN_DURATION_TICKS: u32 = 80;
pub const SMOKE_SCREEN_RADIUS: f32 = 70.0;

// Rapid fire: gun cooldown multiplier. Cooldown ticks are rounded (ties round up), so the
// default 15-tick cooldown becomes round(15 * 0.5) = round(7.5) = 8 ticks.
pub const RAPID_FIRE_DURATION_TICKS: u32 = 50;
pub const RAPID_FIRE_COOLDOWN_MULT: f32 = 0.5;

// Heavy shell: buff applied to shells fired during the window. Shell carries the buff.
pub const HEAVY_SHELL_DURATION_TICKS: u32 = 30;
pub const HEAVY_SHELL_SPLASH_MULT: f32 = 1.5;
pub const HEAVY_SHELL_DAMAGE_MULT: f32 = 1.3;

// Long-range salvo: buff applied to shells fired during the window.
pub const LONG_RANGE_DURATION_TICKS: u32 = 40;
pub const LONG_RANGE_RANGE_MULT: f32 = 1.5;
pub const LONG_RANGE_SPEED_MULT: f32 = 1.6;

// AWACS scan: double active radar range + zero noise on normal contacts. Silent-running
// targets are *not* fully pierced — they show only within base radar range as jittered,
// low-confidence contacts (soft counter to silent_running).
pub const AWACS_DURATION_TICKS: u32 = 60;
pub const AWACS_RANGE_MULT: f32 = 2.0;
/// Half-width of the position jitter (units) applied to silent-running contacts seen by AWACS.
pub const AWACS_SILENT_JITTER: f32 = 15.0;
/// Confidence reported for silent-running contacts surfaced by AWACS.
pub const AWACS_SILENT_CONFIDENCE: f32 = 0.6;

// Silent running: hidden from passive, halved active range against you. Firing breaks it.
pub const SILENT_RUNNING_DURATION_TICKS: u32 = 80;
pub const SILENT_RUNNING_ACTIVE_RANGE_MULT: f32 = 0.5;

// Counter-battery trace: arm window + reveal-track duration. Non-consuming: every hit during
// the armed window (re)starts a `REVEAL_TICKS`-long full-confidence track on the attacker.
pub const COUNTER_BATTERY_ARM_TICKS: u32 = 60;
pub const COUNTER_BATTERY_REVEAL_TICKS: u32 = 15;

// EMP burst: instantaneous AoE that slows guns and forces passive sensors.
pub const EMP_BURST_DURATION_TICKS: u32 = 40;
pub const EMP_BURST_RADIUS: f32 = 130.0;
pub const EMP_GUN_COOLDOWN_MULT: f32 = 2.0;

// Decoy flare: phantom contact that inherits the activator's heading/speed and cruises.
// Spawn distance ahead is jittered (seeded) in [MIN, MAX].
pub const DECOY_FLARE_DURATION_TICKS: u32 = 60;
pub const DECOY_FLARE_DISTANCE_MIN: f32 = 80.0;
pub const DECOY_FLARE_DISTANCE_MAX: f32 = 140.0;
