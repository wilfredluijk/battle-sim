//! Simulation constants. Source of truth: `system-design.md` §5.2 (ship) and §5.4 (weapons).
//!
//! Values must stay in sync with `protocol::ShipSpecs::DEFAULT`. A future test should
//! cross-check the two; for now they are duplicated by inspection.

/// Fixed simulation timestep. The wall clock is only used to *pace* the tick loop, never to
/// drive physics — see `CLAUDE.md` "Determinism in the simulation".
pub const DT: f32 = 0.1;

// --- Ship (§5.2) -----------------------------------------------------------

pub const MAX_FORWARD_SPEED: f32 = 6.0;
pub const MAX_REVERSE_SPEED: f32 = 2.0;
pub const ACCELERATION: f32 = 1.5;
/// Yaw rate at full rudder and full forward speed. Scales linearly with `|speed| / max_forward`.
pub const TURN_RATE_DEG_PER_S: f32 = 15.0;
pub const HULL_HP: u32 = 100;
pub const MAX_AMMO: u32 = 20;
pub const GUN_COOLDOWN_TICKS: u32 = 15;
pub const HIT_RADIUS: f32 = 8.0;

// --- Weapons (§5.4) --------------------------------------------------------

pub const SHELL_SPEED: f32 = 50.0;
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
