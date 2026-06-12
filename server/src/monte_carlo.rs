//! Monte Carlo batch mode: run many back-to-back matches with the same connected bots,
//! varying the starting positions per match, and report which bot wins most often.
//!
//! The controller does not own a separate task — it lives entirely inside [`Room`] and
//! piggybacks on the existing tick loop. While a run is active the room:
//!
//! 1. Drives ticks in **lockstep** rather than wall-clock pacing (see
//!    [`run_room`](crate::room::run_room) for the gate). The match goes as fast as the
//!    slowest bot can respond, with a per-tick timeout backstop.
//! 2. **Skips the post-game lobby** between matches. When [`Room::step_tick`] detects an
//!    end condition, it records the winner, then immediately resets the world and
//!    re-positions ships using a freshly seeded layout.
//! 3. **Throttles** spectator broadcasts to `spectator_throttle` ticks. At full speed
//!    the JSON serialization for `/spectate` would otherwise dominate runtime.
//!
//! Determinism: the same `(mc_seed, n_matches, variance_mode, sim_config)` plus the same
//! starting roster always produces bit-identical outcomes (asserted by
//! `tests/monte_carlo_determinism.rs`). The per-match seed is
//! `mix(mc_seed, match_index)`, the position layout is a pure function of that seed.

use std::collections::BTreeMap;
use std::time::Duration;

use glam::Vec2;
use rand::{Rng, SeedableRng};
use rand_pcg::Pcg64;
use serde::{Deserialize, Serialize};

use crate::sim::{BotId, SimConfig};

/// Default per-tick timeout for the lockstep loop, in milliseconds. If a registered bot
/// hasn't sent its command for the current tick after this many ms, the room steps anyway
/// — better than wedging the whole batch on a stalled client.
pub const DEFAULT_PER_TICK_TIMEOUT_MS: u64 = 1000;

/// Default spectator broadcast cadence in MC mode: emit every Nth tick. Tuned to keep the
/// renderer responsive without serializing JSON on every step.
pub const DEFAULT_SPECTATOR_THROTTLE: u32 = 5;

/// Hard cap on `n_matches`. Way above any realistic UI input, but stops a malformed REST
/// payload from queueing millions of matches.
pub const MAX_MATCHES: u32 = 10_000;

/// How many most-recent results to keep in [`McStatus::results`] so the UI can render a
/// "last N matches" tail without unbounded memory growth.
pub const RESULT_TAIL_LIMIT: usize = 20;

/// Minimum separation (in world units) between any two ships in `Random` variance mode.
/// Picked larger than the default hit radius so two ships never spawn touching.
pub const MIN_SPAWN_SEPARATION: f32 = 80.0;

/// Hard cap on rejection-sampling attempts in `Random` mode. If we can't fit ships under
/// the separation constraint inside this many tries we fall back to `Rotated`. Bounded so
/// a pathological config can never spin forever.
const RANDOM_PLACEMENT_MAX_ATTEMPTS: u32 = 256;

/// Starting circle radius used by the non-random variance modes. Mirrors the constant
/// the room already uses for its default ring layout.
pub const STARTING_RING_RADIUS: f32 = 400.0;

/// How starting positions vary between matches in a Monte Carlo batch.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VarianceMode {
    /// Identical to the room's default layout: every bot on the radius-400 ring at
    /// evenly spaced angles, ordered by `BotId`, all facing centre. Control case for
    /// before/after comparisons.
    Fixed,
    /// Same ring, same per-bot order, but the whole ring is rotated by a per-match
    /// random angle. Relative geometry is preserved; the absolute world rotates.
    Rotated,
    /// Rotate plus permute which bot lands on which slot. Changes who faces whom.
    #[default]
    Shuffled,
    /// Sample each ship's spawn position uniformly inside a centered disk, rejection-
    /// sampled for `MIN_SPAWN_SEPARATION`. Initial heading is also randomized.
    Random,
}

/// Operator-supplied configuration for a single MC run. Sent in the body of
/// `POST /api/montecarlo/start` and frozen for the duration of the run.
#[derive(Debug, Clone, Deserialize)]
pub struct McConfig {
    pub n_matches: u32,
    /// Root seed for the run. Per-match seeds are derived deterministically from this.
    pub mc_seed: u64,
    pub variance_mode: VarianceMode,
    /// Lockstep step deadline in milliseconds. Defaults to [`DEFAULT_PER_TICK_TIMEOUT_MS`].
    #[serde(default)]
    pub per_tick_timeout_ms: Option<u64>,
    /// Spectator broadcast cadence (every Nth tick). `0` disables spectator updates
    /// entirely. Defaults to [`DEFAULT_SPECTATOR_THROTTLE`].
    #[serde(default)]
    pub spectator_throttle: Option<u32>,
    /// Optional balance-parameter override applied once at the start of the run.
    /// `None` keeps whatever `SimConfig` the room already has.
    #[serde(default)]
    pub sim_config: Option<SimConfig>,
}

impl McConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.n_matches == 0 {
            return Err("n_matches must be at least 1".into());
        }
        if self.n_matches > MAX_MATCHES {
            return Err(format!("n_matches must not exceed {MAX_MATCHES}"));
        }
        if let Some(ms) = self.per_tick_timeout_ms {
            if ms == 0 {
                return Err("per_tick_timeout_ms must be greater than zero".into());
            }
            if ms > 300_000 {
                return Err("per_tick_timeout_ms must not exceed 300000 (5 minutes)".into());
            }
        }
        if let Some(throttle) = self.spectator_throttle {
            if throttle > 1000 {
                return Err("spectator_throttle must not exceed 1000".into());
            }
        }
        if let Some(cfg) = self.sim_config.as_ref() {
            cfg.validate()?;
        }
        Ok(())
    }

    pub fn per_tick_timeout(&self) -> Duration {
        Duration::from_millis(
            self.per_tick_timeout_ms
                .unwrap_or(DEFAULT_PER_TICK_TIMEOUT_MS),
        )
    }

    pub fn effective_spectator_throttle(&self) -> u32 {
        self.spectator_throttle
            .unwrap_or(DEFAULT_SPECTATOR_THROTTLE)
    }
}

/// One match's outcome, recorded as the controller chains from one match to the next.
#[derive(Debug, Clone, Serialize)]
pub struct MatchResult {
    /// 1-based index into the run. The first match is `match_index = 1`.
    pub match_index: u32,
    pub seed: u64,
    /// Winning bot's id, or `None` for a draw.
    pub winner: Option<BotId>,
    pub winner_name: Option<String>,
    pub duration_ticks: u64,
    pub replay_id: Option<String>,
}

/// Snapshot of the run's progress, surfaced via `GET /api/montecarlo/status`.
#[derive(Debug, Clone, Serialize)]
pub struct McStatus {
    /// `true` while a run is in flight. After the run finishes (or stops) this stays
    /// `false` and the remaining fields describe the last run, until a new one starts.
    pub running: bool,
    pub run_id: String,
    pub completed: u32,
    pub total: u32,
    pub variance_mode: VarianceMode,
    pub mc_seed: u64,
    pub started_at_unix: u64,
    /// Set once the run finishes naturally, is stopped, or aborts. `None` while running.
    pub finished_at_unix: Option<u64>,
    /// Tick counter of the in-progress match (0 between matches). Useful for the UI's
    /// "match #N, tick T" badge.
    pub current_match_tick: u64,
    /// Win count per bot id, plus per-bot display name for the UI. Cleared at run start.
    pub wins: BTreeMap<BotId, u32>,
    pub bot_names: BTreeMap<BotId, String>,
    pub draws: u32,
    /// Tail of recent match results. Bounded to [`RESULT_TAIL_LIMIT`] entries.
    pub results: Vec<MatchResult>,
    /// Human-readable reason the run ended (`"completed"`, `"stopped"`, `"bot_disconnected"`,
    /// `"insufficient_bots"`, etc.). `None` while running.
    pub ended_reason: Option<String>,
}

impl McStatus {
    pub fn empty() -> Self {
        Self {
            running: false,
            run_id: String::new(),
            completed: 0,
            total: 0,
            variance_mode: VarianceMode::Fixed,
            mc_seed: 0,
            started_at_unix: 0,
            finished_at_unix: None,
            current_match_tick: 0,
            wins: BTreeMap::new(),
            bot_names: BTreeMap::new(),
            draws: 0,
            results: Vec::new(),
            ended_reason: None,
        }
    }
}

/// In-flight state for an active Monte Carlo run, owned by the room.
#[derive(Debug, Clone)]
pub struct McState {
    pub run_id: String,
    pub config: McConfig,
    /// 0-based index of the match that is *about to* start (or is currently running).
    /// After the last match finishes this becomes `n_matches`, signalling "done".
    pub current_index: u32,
    pub wins: BTreeMap<BotId, u32>,
    pub draws: u32,
    pub results: Vec<MatchResult>,
    pub started_at_unix: u64,
}

impl McState {
    pub fn new(config: McConfig, run_id: String, started_at_unix: u64) -> Self {
        Self {
            run_id,
            config,
            current_index: 0,
            wins: BTreeMap::new(),
            draws: 0,
            results: Vec::new(),
            started_at_unix,
        }
    }

    /// `true` if there is still at least one match left to start.
    pub fn has_more_matches(&self) -> bool {
        self.current_index < self.config.n_matches
    }

    /// Seed for the next match (the one indexed by `current_index`). Computed via the
    /// `splitmix64` finalizer applied to `(mc_seed XOR match_index)` so adjacent indices
    /// don't produce correlated RNG streams.
    pub fn seed_for_next_match(&self) -> u64 {
        mix_match_seed(self.config.mc_seed, self.current_index)
    }

    /// Record the outcome of the match that just ended and advance the counter.
    pub fn record_result(
        &mut self,
        winner: Option<BotId>,
        winner_name: Option<String>,
        duration_ticks: u64,
        replay_id: Option<String>,
        seed: u64,
    ) {
        let match_index = self.current_index + 1;
        match winner.as_ref() {
            Some(id) => *self.wins.entry(id.clone()).or_insert(0) += 1,
            None => self.draws += 1,
        }
        let result = MatchResult {
            match_index,
            seed,
            winner,
            winner_name,
            duration_ticks,
            replay_id,
        };
        self.results.push(result);
        if self.results.len() > RESULT_TAIL_LIMIT {
            let overflow = self.results.len() - RESULT_TAIL_LIMIT;
            self.results.drain(0..overflow);
        }
        self.current_index += 1;
    }
}

/// `splitmix64`-style finalizer applied to `(seed XOR index)`. Avalanches every input bit
/// across the output so close `(seed, index)` pairs don't produce correlated RNG streams.
fn mix_match_seed(mc_seed: u64, match_index: u32) -> u64 {
    let mut z = mc_seed ^ (match_index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Build the per-bot `(position, heading_deg)` layout for one match.
///
/// `bot_count` must match the number of registered bots; the returned vector is in
/// registration / `BotId` order. The bots' assignment to slots is permuted in `Shuffled`
/// mode but the *returned vector position* is always BotId-aligned so the caller can do
/// `ordered_ids[i] -> layout[i]` without further bookkeeping.
///
/// The center / map size mirror the room's existing ring layout.
pub fn place_ships_for_variance(
    mode: VarianceMode,
    seed: u64,
    bot_count: usize,
    map_width: f32,
    map_height: f32,
) -> Vec<(Vec2, f32)> {
    if bot_count == 0 {
        return Vec::new();
    }
    let center = Vec2::new(map_width * 0.5, map_height * 0.5);
    // BALANCE/DETERMINISM: bound the spawn ring to the map so ships start strictly inside
    // the walls even on small maps. `STARTING_RING_RADIUS` is the upper bound (legacy
    // behaviour on 1000x1000 maps, where 0.4*1000 == 400). On smaller maps the radius
    // shrinks so physics never clamps a spawn onto a wall. Changing this changes spawn
    // geometry, and therefore match outcomes — old on-disk replays used the unbounded ring.
    let ring_radius = bounded_ring_radius(map_width, map_height);
    let mut rng = Pcg64::seed_from_u64(seed);
    match mode {
        VarianceMode::Fixed => ring_layout(
            center,
            bot_count,
            0.0,
            &(0..bot_count).collect::<Vec<_>>(),
            ring_radius,
        ),
        VarianceMode::Rotated => {
            let angle_offset = rng.gen_range(0.0_f32..std::f32::consts::TAU);
            ring_layout(
                center,
                bot_count,
                angle_offset,
                &(0..bot_count).collect::<Vec<_>>(),
                ring_radius,
            )
        }
        VarianceMode::Shuffled => {
            let angle_offset = rng.gen_range(0.0_f32..std::f32::consts::TAU);
            let permutation = deterministic_permutation(bot_count, &mut rng);
            ring_layout(center, bot_count, angle_offset, &permutation, ring_radius)
        }
        VarianceMode::Random => {
            random_disk_layout(center, bot_count, map_width, map_height, &mut rng).unwrap_or_else(
                || {
                    // Rejection sampling couldn't satisfy the separation constraint
                    // within RANDOM_PLACEMENT_MAX_ATTEMPTS. Fall back to a rotated ring
                    // so the match still runs deterministically.
                    let angle_offset = rng.gen_range(0.0_f32..std::f32::consts::TAU);
                    ring_layout(
                        center,
                        bot_count,
                        angle_offset,
                        &(0..bot_count).collect::<Vec<_>>(),
                        ring_radius,
                    )
                },
            )
        }
    }
}

/// Spawn-ring radius bounded to the map. Returns [`STARTING_RING_RADIUS`] on large maps and
/// shrinks on smaller ones so every spawn stays well inside the walls. `0.4 * min_dim`
/// leaves a comfortable margin (a 700x700 map yields 280, ~70 units of clearance from each
/// edge before any ship geometry). On 1000x1000 maps `0.4*1000 == 400 == STARTING_RING_RADIUS`,
/// so legacy behaviour is unchanged there.
fn bounded_ring_radius(map_width: f32, map_height: f32) -> f32 {
    STARTING_RING_RADIUS.min(0.4 * map_width.min(map_height))
}

fn ring_layout(
    center: Vec2,
    bot_count: usize,
    angle_offset: f32,
    slot_assignment: &[usize],
    ring_radius: f32,
) -> Vec<(Vec2, f32)> {
    let n = bot_count as f32;
    let mut out = vec![(Vec2::ZERO, 0.0_f32); bot_count];
    for (bot_index, &slot) in slot_assignment.iter().enumerate() {
        let angle = std::f32::consts::TAU * (slot as f32) / n + angle_offset;
        let offset = Vec2::new(angle.cos(), angle.sin()) * ring_radius;
        let pos = center + offset;
        let heading = compass_deg_facing(pos, center);
        out[bot_index] = (pos, heading);
    }
    out
}

fn random_disk_layout(
    center: Vec2,
    bot_count: usize,
    map_width: f32,
    map_height: f32,
    rng: &mut Pcg64,
) -> Option<Vec<(Vec2, f32)>> {
    // BALANCE/DETERMINISM: bound the sampling disk to the map (same rule as the ring) so
    // candidate positions stay inside the walls on small maps before the `margin` clamp.
    let radius = bounded_ring_radius(map_width, map_height) * 1.1;
    // Stay inside the map even if the disk would otherwise clip the edge.
    let margin = MIN_SPAWN_SEPARATION;
    let min_x = (center.x - radius).max(margin);
    let max_x = (center.x + radius).min(map_width - margin);
    let min_y = (center.y - radius).max(margin);
    let max_y = (center.y + radius).min(map_height - margin);
    if max_x <= min_x || max_y <= min_y {
        return None;
    }

    let mut placed: Vec<Vec2> = Vec::with_capacity(bot_count);
    for _ in 0..bot_count {
        let mut placed_one = false;
        for _ in 0..RANDOM_PLACEMENT_MAX_ATTEMPTS {
            let candidate = Vec2::new(rng.gen_range(min_x..max_x), rng.gen_range(min_y..max_y));
            // Rejection: must be inside the disk AND separated from every prior pick.
            if (candidate - center).length() > radius {
                continue;
            }
            if placed
                .iter()
                .all(|p| (candidate - *p).length() >= MIN_SPAWN_SEPARATION)
            {
                placed.push(candidate);
                placed_one = true;
                break;
            }
        }
        if !placed_one {
            return None;
        }
    }

    Some(
        placed
            .into_iter()
            .map(|pos| {
                let heading = rng.gen_range(0.0_f32..360.0);
                (pos, heading)
            })
            .collect(),
    )
}

/// Knuth shuffle of `0..n` using a seeded RNG. Returns a `Vec` so iteration order is
/// fully deterministic (unlike `rand::seq::SliceRandom::shuffle`, which would also be
/// fine but goes through traits the determinism contract doesn't audit).
fn deterministic_permutation(n: usize, rng: &mut Pcg64) -> Vec<usize> {
    let mut out: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        // gen_range is u64-domain underneath; the modulo bias on the small ranges we
        // use here is negligible (<= 8 slots in practice).
        let j = rng.gen_range(0..=i);
        out.swap(i, j);
    }
    out
}

/// Mirror of [`crate::room::compass_deg_facing`] — small helper so this module doesn't
/// reach into `room.rs`. Compass bearing of the vector `from -> to`, with 0° = north.
fn compass_deg_facing(from: Vec2, to: Vec2) -> f32 {
    let v = to - from;
    let deg = v.x.atan2(-v.y).to_degrees();
    if deg < 0.0 {
        deg + 360.0
    } else {
        deg
    }
}

/// Generate the replay id for a single match in an MC run. Embeds the run id, the
/// 1-based match index zero-padded to four digits (sorts naturally in `ls`), and the
/// per-match seed so a replay's filename alone identifies its position in the batch.
pub fn make_mc_replay_id(run_id: &str, match_index: u32, seed: u64) -> String {
    format!("mc_{run_id}_match_{match_index:04}_seed_{seed:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variance_mode_deserializes_snake_case() {
        let raw = r#""shuffled""#;
        let mode: VarianceMode = serde_json::from_str(raw).expect("parse");
        assert_eq!(mode, VarianceMode::Shuffled);
    }

    #[test]
    fn validate_rejects_zero_matches() {
        let cfg = McConfig {
            n_matches: 0,
            mc_seed: 1,
            variance_mode: VarianceMode::Fixed,
            per_tick_timeout_ms: None,
            spectator_throttle: None,
            sim_config: None,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_caps_n_matches() {
        let cfg = McConfig {
            n_matches: MAX_MATCHES + 1,
            mc_seed: 1,
            variance_mode: VarianceMode::Fixed,
            per_tick_timeout_ms: None,
            spectator_throttle: None,
            sim_config: None,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn match_seeds_are_deterministic_but_uncorrelated() {
        let a = mix_match_seed(42, 0);
        let b = mix_match_seed(42, 1);
        let c = mix_match_seed(42, 0);
        assert_eq!(a, c, "same input must produce same output");
        assert_ne!(a, b, "adjacent indices should not collide");
        // Hamming distance between adjacent mixes should be roughly half the bits — a very
        // loose sanity bound just to catch the "off by one" bug where indices barely move.
        let xor = a ^ b;
        assert!(
            xor.count_ones() >= 8,
            "weak mixer: a^b only differs in {} bit(s)",
            xor.count_ones()
        );
    }

    #[test]
    fn fixed_mode_returns_legacy_layout() {
        let layout = place_ships_for_variance(VarianceMode::Fixed, 42, 4, 1000.0, 1000.0);
        assert_eq!(layout.len(), 4);
        let center = Vec2::new(500.0, 500.0);
        for (pos, heading) in &layout {
            // Every ship on the ring.
            let r = (*pos - center).length();
            assert!(
                (r - STARTING_RING_RADIUS).abs() < 1e-3,
                "ship off ring: r = {r}",
            );
            // Heading points toward centre.
            let dir = Vec2::new(heading.to_radians().sin(), -heading.to_radians().cos());
            let towards = (center - *pos).normalize();
            assert!(dir.dot(towards) > 0.999, "heading not facing centre");
        }
    }

    #[test]
    fn rotated_mode_varies_with_seed_but_is_reproducible() {
        let a = place_ships_for_variance(VarianceMode::Rotated, 1, 4, 1000.0, 1000.0);
        let a2 = place_ships_for_variance(VarianceMode::Rotated, 1, 4, 1000.0, 1000.0);
        let b = place_ships_for_variance(VarianceMode::Rotated, 2, 4, 1000.0, 1000.0);
        assert_eq!(a, a2, "same seed → same layout");
        assert_ne!(a, b, "different seed → different rotation");
        // Distance between adjacent ships preserved (it's a rotation, not a scaling).
        let d0_legacy = (a[0].0 - a[1].0).length();
        let d0_fixed = {
            let l = place_ships_for_variance(VarianceMode::Fixed, 0, 4, 1000.0, 1000.0);
            (l[0].0 - l[1].0).length()
        };
        assert!((d0_legacy - d0_fixed).abs() < 1e-3);
    }

    #[test]
    fn shuffled_mode_permutes_bot_slots() {
        // With 6 bots, at least one shuffled seed should map BotId 0 to a slot other
        // than 0 — verifies the permutation actually fires.
        let fixed = place_ships_for_variance(VarianceMode::Fixed, 0, 6, 1000.0, 1000.0);
        let mut saw_permutation = false;
        for seed in 1u64..50 {
            let shuffled =
                place_ships_for_variance(VarianceMode::Shuffled, seed, 6, 1000.0, 1000.0);
            // For the shuffle to be detectable, account for the random rotation by
            // checking the *multiset* of (radius, …) matches but the per-index mapping
            // differs. Distances from centre must match (rotation preserves them).
            for (f, s) in fixed.iter().zip(shuffled.iter()) {
                let rf = (f.0 - Vec2::new(500.0, 500.0)).length();
                let rs = (s.0 - Vec2::new(500.0, 500.0)).length();
                assert!((rf - rs).abs() < 1e-3);
            }
            // Different positions vs. the rotated-only layout at the same seed indicate
            // the permutation is doing something. Compare against Rotated at this seed.
            let rotated = place_ships_for_variance(VarianceMode::Rotated, seed, 6, 1000.0, 1000.0);
            if shuffled != rotated {
                saw_permutation = true;
                break;
            }
        }
        assert!(
            saw_permutation,
            "Shuffled mode never produced a non-trivial permutation"
        );
    }

    #[test]
    fn random_mode_respects_separation() {
        for seed in 1u64..20 {
            let layout = place_ships_for_variance(VarianceMode::Random, seed, 4, 1000.0, 1000.0);
            assert_eq!(layout.len(), 4);
            for i in 0..layout.len() {
                for j in (i + 1)..layout.len() {
                    let d = (layout[i].0 - layout[j].0).length();
                    assert!(
                        d >= MIN_SPAWN_SEPARATION - 1e-3,
                        "ships {i}/{j} too close at seed {seed}: d = {d}",
                    );
                }
            }
        }
    }

    #[test]
    fn random_mode_is_reproducible() {
        let a = place_ships_for_variance(VarianceMode::Random, 7, 4, 1000.0, 1000.0);
        let b = place_ships_for_variance(VarianceMode::Random, 7, 4, 1000.0, 1000.0);
        assert_eq!(a, b);
    }

    #[test]
    fn spawns_stay_inside_small_maps() {
        // On small maps the spawn ring must shrink so no ship lands on or past a wall.
        // Check every VarianceMode across a few seeds for both a 700x700 and a 400x400 map.
        let maps = [(700.0_f32, 700.0_f32), (400.0_f32, 400.0_f32)];
        let modes = [
            VarianceMode::Fixed,
            VarianceMode::Rotated,
            VarianceMode::Shuffled,
            VarianceMode::Random,
        ];
        // Comfortable buffer from each edge (well over any ship hit radius ~ a few units).
        let buffer = 1.0_f32;
        for (w, h) in maps {
            let center = Vec2::new(w * 0.5, h * 0.5);
            let max_r = bounded_ring_radius(w, h);
            for mode in modes {
                for seed in 0u64..20 {
                    let layout = place_ships_for_variance(mode, seed, 4, w, h);
                    assert_eq!(layout.len(), 4);
                    for (pos, _heading) in &layout {
                        assert!(
                            pos.x > buffer && pos.x < w - buffer,
                            "x out of bounds: {} on {w}x{h} ({mode:?}, seed {seed})",
                            pos.x,
                        );
                        assert!(
                            pos.y > buffer && pos.y < h - buffer,
                            "y out of bounds: {} on {w}x{h} ({mode:?}, seed {seed})",
                            pos.y,
                        );
                        // Distance from centre never exceeds the bounded radius. Random
                        // mode samples a disk of radius `max_r * 1.1`, so allow that.
                        let r = (*pos - center).length();
                        let limit = if mode == VarianceMode::Random {
                            max_r * 1.1 + 1e-3
                        } else {
                            max_r + 1e-3
                        };
                        assert!(
                            r < limit,
                            "spawn too far from centre: r = {r} (limit {limit}) on {w}x{h} ({mode:?}, seed {seed})",
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn large_map_radius_matches_legacy() {
        // On 1000x1000, 0.4*1000 == 400 == STARTING_RING_RADIUS, so the ring is unchanged.
        assert!((bounded_ring_radius(1000.0, 1000.0) - STARTING_RING_RADIUS).abs() < 1e-3);
        // Anything >= 1000 in both dims also pins to the constant.
        assert!((bounded_ring_radius(2000.0, 1500.0) - STARTING_RING_RADIUS).abs() < 1e-3);
    }

    #[test]
    fn make_mc_replay_id_is_safe_for_filesystem() {
        let id = make_mc_replay_id("abc123", 7, 0xDEAD_BEEF);
        // The handler validates replay ids with /^[A-Za-z0-9_-]+$/; assert that.
        assert!(id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'));
        assert!(id.contains("mc_abc123"));
        assert!(id.contains("match_0007"));
    }

    #[test]
    fn state_records_results_and_caps_tail() {
        let cfg = McConfig {
            n_matches: 50,
            mc_seed: 1,
            variance_mode: VarianceMode::Fixed,
            per_tick_timeout_ms: None,
            spectator_throttle: None,
            sim_config: None,
        };
        let mut s = McState::new(cfg, "run".into(), 1000);
        for _ in 0..(RESULT_TAIL_LIMIT + 5) {
            let seed = s.seed_for_next_match();
            s.record_result(
                Some("b_1".into()),
                Some("alice".into()),
                123,
                Some("rid".into()),
                seed,
            );
        }
        assert_eq!(s.results.len(), RESULT_TAIL_LIMIT);
        assert_eq!(s.current_index, (RESULT_TAIL_LIMIT + 5) as u32);
        assert_eq!(
            s.wins.get("b_1").copied(),
            Some((RESULT_TAIL_LIMIT + 5) as u32)
        );
    }
}
