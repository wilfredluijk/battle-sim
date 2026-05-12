"""Tactician bot: stealth-first, evidence-based naval combat.

Doctrine
--------

The tactician treats every ping as a broadcast and every shell as
non-renewable capital. It loses fights of attrition and wins fights of
information. The strategy, in five tenets:

1. **Passive by default.** Active radar reaches 350 units but paints us
   to every listener within 500. We only ping when we are about to fire
   or when we have lost track of every known enemy. Default cadence is
   a 2-tick burst no more than once per 30 ticks.

2. **Track from any signal.** Active fixes give a noisy position;
   passive sweeps give a bearing only. Both fold into per-enemy
   `EnemyTrack`s. When we have enough self-motion between two passive
   bearings on the same target we triangulate a position estimate from
   the rays — submarine-style TMA, cheap two-ray intersection. The
   tracker carries an uncertainty bound that grows with age and shrinks
   on observation; the gunnery layer reads it to decide whether a shot
   is worth a shell.

3. **Score every shot.** Shells are scarce (20 total, no reload). Before
   firing we estimate expected splash damage given the lead solution,
   the flight time, and the track's current uncertainty. Below a
   threshold (which climbs as ammo runs out and as our own HP drops)
   we hold. We also never fire if the aim point sits inside our own
   splash radius, and never if we'd waste a shell on a track that's
   gone purely-passive for too long.

4. **Stay unpredictable.** Even when the navigator wants a straight
   course, we overlay a slow yaw oscillation. A constant bearing/range
   solution is a dead solution: a competent opponent leads us and
   walks splashes onto us in three shots.

5. **React to evidence.** A `Hit` means our position is known to
   someone — we evade hard, flip rudder, run at full throttle for a
   dozen ticks. A nearby `ShellSplash` means we were almost known — we
   break the course we were on so any extrapolation misses. Low HP
   re-weights the whole policy toward survival: longer-range only,
   no shots that might splash us, more evasion.

Override priorities (top wins each tick):

  1. Wall avoidance — running aground costs 2 HP and a dead stop.
  2. Active evasion (Hit / splash response).
  3. Engagement positioning toward the best target.
  4. Search sweeps when we've lost contact with everyone.

The bot is RNG-free: behaviour is a pure function of the WorldView
history, which keeps replays and unit tests reproducible.
"""

from __future__ import annotations

import argparse
import logging
import math
from collections import deque
from dataclasses import dataclass, field
from typing import Deque, Dict, List, Optional, Tuple

from naval_sdk import (
    Bot,
    Command,
    Contact,
    WorldView,
    bearing_to,
    distance,
    lead_target,
    run,
)
from naval_sdk.protocol import HitEvent, ShellSplashEvent, Welcome

log = logging.getLogger("tactician_bot")

Vec2 = Tuple[float, float]

# ---------------------------------------------------------------------------
# Tunables. Centralised so a subclass or a debugger can prod them without
# spelunking through the control flow.
# ---------------------------------------------------------------------------

# Range bands (units).
PREFERRED_RANGE = 180.0
RANGE_BAND_HALF_WIDTH = 40.0
MIN_ENGAGEMENT_RANGE = 60.0
MAX_ENGAGEMENT_RANGE = 280.0

# Sensor scheduling (ticks).
ACTIVE_BURST_TICKS = 2
ACTIVE_COOLDOWN_TICKS = 30
PRE_FIRE_ACTIVE_LEAD_TICKS = 3        # ping this many ticks before a planned shot
NO_CONTACT_PING_INTERVAL = 18         # if blind, ping every N ticks

# Track ageing.
TRACK_STALE_TICKS = 90
RANGED_STALE_TICKS = 25
GATE_DIST = 75.0
PASSIVE_BEARING_TOL_DEG = 18.0
PASSIVE_OBS_HISTORY = 8

# Velocity estimation.
VEL_LP_NEW_WEIGHT = 0.55              # low-pass weight for new velocity sample
MAX_PLAUSIBLE_SPEED = 8.0

# Triangulation.
MIN_TRIANGULATION_PARALLAX_DEG = 8.0  # below this two bearings are too colinear
MIN_TRIANGULATION_BASELINE = 15.0     # below this our own motion is too small

# Navigation.
WALL_MARGIN = 90.0
COURSE_OSC_AMPLITUDE_DEG = 14.0
COURSE_OSC_PERIOD_TICKS = 25
EVADE_TICKS_HIT = 14
EVADE_TICKS_SPLASH = 7
EVADE_HARD_RUDDER = 1.0

# Engagement quality gates.
MIN_EXPECTED_DAMAGE = 5.0
MIN_EXPECTED_DAMAGE_LOW_AMMO = 11.0
LOW_AMMO_THRESHOLD = 6
LOW_HP_THRESHOLD = 30

# Self-splash safety: never let the aim point land within this radius of us.
# Splash radius is 15; we add a generous margin because the lead solution
# can land us closer than intended if we close the range hard.
SELF_SPLASH_RADIUS_MULTIPLIER = 2.0


# ---------------------------------------------------------------------------
# State models.
# ---------------------------------------------------------------------------


@dataclass
class PassiveObservation:
    my_pos: Vec2
    bearing_deg: float
    tick: int


@dataclass
class EnemyTrack:
    """Estimate of one enemy ship's position and velocity over time."""

    pos: Vec2
    vel: Vec2
    last_seen_tick: int
    last_ranged_tick: int  # -1 if never observed in active
    last_passive_tick: int  # -1 if never observed in passive
    passive_obs: Deque[PassiveObservation] = field(
        default_factory=lambda: deque(maxlen=PASSIVE_OBS_HISTORY)
    )
    confirmed_ranged: bool = False

    def uncertainty(self, tick: int) -> float:
        """Rough 1-sigma position uncertainty in units, growing with age."""
        speed = math.hypot(self.vel[0], self.vel[1])
        if self.confirmed_ranged:
            base = 2.0  # active position noise
            age = max(0, tick - self.last_ranged_tick)
        elif self.last_passive_tick >= 0:
            base = 20.0  # passive triangulation noise
            age = max(0, tick - self.last_passive_tick)
        else:
            return 9_999.0
        # Diverges with target manoeuvring; the constant 0.4 is an
        # acceleration-noise term that bites the longer we coast on memory.
        return base + speed * 0.1 * age + 0.4 * age


@dataclass
class TacState:
    tracks: Dict[int, EnemyTrack] = field(default_factory=dict)
    next_track_id: int = 0

    last_active_tick: int = -10_000
    active_until_tick: int = -1

    evade_until_tick: int = -1
    evade_rudder_sign: float = 1.0

    next_fire_tick: int = 0
    last_fired_at_tick: int = -10_000

    # When we plan a shot, we set this to schedule a pre-fire ping so the
    # gunnery layer has a fresh range fix on the firing tick.
    pre_fire_ping_target: Optional[int] = None


# ---------------------------------------------------------------------------
# Helper math.
# ---------------------------------------------------------------------------


def _signed_bearing_delta(target: float, current: float) -> float:
    return ((target - current + 540.0) % 360.0) - 180.0


def _abs_bearing_delta(a: float, b: float) -> float:
    return abs(_signed_bearing_delta(a, b))


def _bearing_to_unit(bearing_deg: float) -> Vec2:
    """Server convention: 0° = -y, 90° = +x, clockwise."""
    rad = math.radians(bearing_deg)
    return (math.sin(rad), -math.cos(rad))


def _ray_intersection(
    origin_a: Vec2, dir_a: Vec2, origin_b: Vec2, dir_b: Vec2
) -> Optional[Vec2]:
    """Intersect two parametric rays. Returns the point or None if (anti-)parallel
    or both intersection parameters aren't positive."""
    det = dir_b[0] * dir_a[1] - dir_a[0] * dir_b[1]
    if abs(det) < 1e-4:
        return None
    dx = origin_b[0] - origin_a[0]
    dy = origin_b[1] - origin_a[1]
    t_a = (dir_b[0] * dy - dir_b[1] * dx) / det
    t_b = (dir_a[0] * dy - dir_a[1] * dx) / det
    if t_a < 0 or t_b < 0:
        return None
    return (origin_a[0] + t_a * dir_a[0], origin_a[1] + t_a * dir_a[1])


# ---------------------------------------------------------------------------
# The bot.
# ---------------------------------------------------------------------------


class TacticianBot(Bot):
    def __init__(self) -> None:
        super().__init__()
        self.state = TacState()

        # Ship specs (overwritten by welcome).
        self._shell_speed = 50.0
        self._max_shell_range = 300.0
        self._splash_radius = 15.0
        self._max_splash_damage = 25
        self._gun_cooldown = 15
        self._max_ammo = 20
        self._hull_hp = 100
        self._max_forward_speed = 6.0
        self._map_w = 1000
        self._map_h = 1000

    # ---- lifecycle ----

    def on_welcome(self, welcome: Welcome) -> None:
        specs = welcome.ship_specs
        self._shell_speed = specs.shell_speed
        self._max_shell_range = specs.max_shell_range
        self._splash_radius = specs.splash_radius
        self._max_splash_damage = specs.max_splash_damage
        self._gun_cooldown = specs.gun_cooldown_ticks
        self._max_ammo = specs.max_ammo
        self._hull_hp = specs.hull_hp
        self._max_forward_speed = specs.max_forward_speed
        self._map_w = welcome.map.width
        self._map_h = welcome.map.height
        log.info(
            "tactician specs: shell %.0f units/s, max_range %.0f, splash r=%.0f damage=%d, ammo=%d",
            self._shell_speed,
            self._max_shell_range,
            self._splash_radius,
            self._max_splash_damage,
            self._max_ammo,
        )

    # ---- main loop ----

    def on_tick(self, view: WorldView) -> Command:
        self._ingest_events(view)
        self._ingest_contacts(view)

        target = self._select_target(view)

        sensor_mode = self._decide_sensor_mode(view, target)
        heading_deg = self._plan_heading(view, target)

        rudder, throttle = self._steer_toward(view, heading_deg, target)

        cmd = Command(throttle=throttle, rudder=rudder, sensor_mode=sensor_mode)

        if (
            target is not None
            and view.me.ammo > 0
            and view.tick >= self.state.next_fire_tick
        ):
            self._try_fire(cmd, view, target)

        return cmd

    # =====================================================================
    # Perception
    # =====================================================================

    def _ingest_events(self, view: WorldView) -> None:
        tick = view.tick
        for ev in view.events:
            if isinstance(ev, HitEvent):
                # Someone has us pinned. Evade hard and flip the rudder so
                # subsequent shooters can't reuse the same lead solution.
                self._start_evade(tick, EVADE_TICKS_HIT, flip=True)
            elif isinstance(ev, ShellSplashEvent):
                # Treat as a near-miss only if it landed within a couple of
                # splash radii of us. Distant splashes are someone else's
                # problem.
                if distance(ev.pos, view.me.pos) < self._splash_radius * 4.0:
                    self._start_evade(tick, EVADE_TICKS_SPLASH, flip=False)

    def _start_evade(self, tick: int, duration: int, *, flip: bool) -> None:
        if flip:
            self.state.evade_rudder_sign = -self.state.evade_rudder_sign
        new_end = tick + duration
        if new_end > self.state.evade_until_tick:
            self.state.evade_until_tick = new_end

    def _ingest_contacts(self, view: WorldView) -> None:
        tick = view.tick
        dt = 0.1

        # Dead-reckon every existing track first so that association uses the
        # predicted position rather than a stale one.
        for tid, tr in list(self.state.tracks.items()):
            self.state.tracks[tid] = EnemyTrack(
                pos=(tr.pos[0] + tr.vel[0] * dt, tr.pos[1] + tr.vel[1] * dt),
                vel=tr.vel,
                last_seen_tick=tr.last_seen_tick,
                last_ranged_tick=tr.last_ranged_tick,
                last_passive_tick=tr.last_passive_tick,
                passive_obs=tr.passive_obs,
                confirmed_ranged=tr.confirmed_ranged,
            )

        # Sort deterministically; ranged first so the high-quality fix wins
        # association before any bearing-only contacts are folded in.
        ship_contacts = sorted(
            (c for c in view.contacts if c.kind in ("ship", "unknown")),
            key=lambda c: (c.range is None, c.id),
        )

        for contact in ship_contacts:
            if contact.range is not None:
                self._fold_ranged(contact, view.me.pos, tick)
            else:
                self._fold_passive(contact, view.me.pos, tick)

        # Drop stale tracks last, so a tick where we see nothing still ages
        # the existing tracks predictably.
        for tid in list(self.state.tracks.keys()):
            if tick - self.state.tracks[tid].last_seen_tick > TRACK_STALE_TICKS:
                del self.state.tracks[tid]

    def _fold_ranged(self, contact: Contact, my_pos: Vec2, tick: int) -> None:
        tid = self._associate_ranged(contact.pos)
        if tid is None:
            self._spawn_track(contact.pos, tick, ranged=True)
            return

        prev = self.state.tracks[tid]
        if prev.last_ranged_tick < 0:
            # First ranged fix on a previously bearings-only track: trust the
            # new position entirely, keep any motion-analysis velocity estimate.
            self.state.tracks[tid] = EnemyTrack(
                pos=contact.pos,
                vel=prev.vel,
                last_seen_tick=tick,
                last_ranged_tick=tick,
                last_passive_tick=prev.last_passive_tick,
                passive_obs=prev.passive_obs,
                confirmed_ranged=True,
            )
            return

        dt_ticks = max(1, tick - prev.last_ranged_tick)
        dt = dt_ticks * 0.1
        vx_new = (contact.pos[0] - prev.pos[0]) / dt
        vy_new = (contact.pos[1] - prev.pos[1]) / dt
        # Clamp to plausible speed so a noisy fix doesn't inject a 60 u/s
        # velocity spike that wrecks the lead solution.
        speed = math.hypot(vx_new, vy_new)
        if speed > MAX_PLAUSIBLE_SPEED:
            scale = MAX_PLAUSIBLE_SPEED / speed
            vx_new *= scale
            vy_new *= scale

        w = VEL_LP_NEW_WEIGHT
        vel = (
            w * vx_new + (1.0 - w) * prev.vel[0],
            w * vy_new + (1.0 - w) * prev.vel[1],
        )
        self.state.tracks[tid] = EnemyTrack(
            pos=contact.pos,
            vel=vel,
            last_seen_tick=tick,
            last_ranged_tick=tick,
            last_passive_tick=prev.last_passive_tick,
            passive_obs=prev.passive_obs,
            confirmed_ranged=True,
        )

    def _fold_passive(self, contact: Contact, my_pos: Vec2, tick: int) -> None:
        # Find the track whose predicted bearing best matches this contact.
        best_tid: Optional[int] = None
        best_delta = PASSIVE_BEARING_TOL_DEG
        for tid, tr in self.state.tracks.items():
            expected = bearing_to(my_pos, tr.pos)
            delta = _abs_bearing_delta(contact.bearing_deg, expected)
            if delta < best_delta:
                best_delta = delta
                best_tid = tid

        if best_tid is None:
            # Could be a new contact. Without range we can't seed a useful
            # position; place it on the bearing ray at a guess radius and let
            # triangulation refine it on the next pass.
            guess = 250.0
            unit = _bearing_to_unit(contact.bearing_deg)
            seed_pos = (my_pos[0] + unit[0] * guess, my_pos[1] + unit[1] * guess)
            tid = self._spawn_track(seed_pos, tick, ranged=False)
            self.state.tracks[tid].passive_obs.append(
                PassiveObservation(my_pos=my_pos, bearing_deg=contact.bearing_deg, tick=tick)
            )
            return

        tr = self.state.tracks[best_tid]
        tr.passive_obs.append(
            PassiveObservation(my_pos=my_pos, bearing_deg=contact.bearing_deg, tick=tick)
        )

        # Try to triangulate a fresh position estimate from the observation
        # history. Only updates the position estimate if we get a clean fix
        # and the track has never been ranged (active is strictly better).
        if not tr.confirmed_ranged:
            estimate = self._triangulate(tr.passive_obs)
            if estimate is not None:
                new_pos = estimate
            else:
                new_pos = tr.pos
        else:
            new_pos = tr.pos

        self.state.tracks[best_tid] = EnemyTrack(
            pos=new_pos,
            vel=tr.vel,
            last_seen_tick=tick,
            last_ranged_tick=tr.last_ranged_tick,
            last_passive_tick=tick,
            passive_obs=tr.passive_obs,
            confirmed_ranged=tr.confirmed_ranged,
        )

    def _associate_ranged(self, pos: Vec2) -> Optional[int]:
        best_tid: Optional[int] = None
        best_dist = GATE_DIST
        for tid, tr in self.state.tracks.items():
            d = distance(pos, tr.pos)
            if d < best_dist:
                best_dist = d
                best_tid = tid
        return best_tid

    def _spawn_track(self, pos: Vec2, tick: int, *, ranged: bool) -> int:
        tid = self.state.next_track_id
        self.state.next_track_id += 1
        self.state.tracks[tid] = EnemyTrack(
            pos=pos,
            vel=(0.0, 0.0),
            last_seen_tick=tick,
            last_ranged_tick=tick if ranged else -1,
            last_passive_tick=-1 if ranged else tick,
            confirmed_ranged=ranged,
        )
        return tid

    def _triangulate(self, obs: Deque[PassiveObservation]) -> Optional[Vec2]:
        """Cheap two-ray intersection using the pair of observations with the
        most parallax. Returns None when the geometry is degenerate (rays
        nearly parallel, or our own baseline too short)."""
        if len(obs) < 2:
            return None
        obs_list = list(obs)
        best_point: Optional[Vec2] = None
        best_score = 0.0
        for i in range(len(obs_list)):
            for j in range(i + 1, len(obs_list)):
                a, b = obs_list[i], obs_list[j]
                baseline = distance(a.my_pos, b.my_pos)
                if baseline < MIN_TRIANGULATION_BASELINE:
                    continue
                bearing_gap = _abs_bearing_delta(a.bearing_deg, b.bearing_deg)
                if bearing_gap < MIN_TRIANGULATION_PARALLAX_DEG:
                    continue
                dir_a = _bearing_to_unit(a.bearing_deg)
                dir_b = _bearing_to_unit(b.bearing_deg)
                point = _ray_intersection(a.my_pos, dir_a, b.my_pos, dir_b)
                if point is None:
                    continue
                # Score: more parallax + longer baseline is more reliable.
                score = baseline * bearing_gap
                if score > best_score:
                    best_score = score
                    best_point = point
        return best_point

    # =====================================================================
    # Targeting
    # =====================================================================

    def _select_target(self, view: WorldView) -> Optional[EnemyTrack]:
        if not self.state.tracks:
            return None
        tick = view.tick

        def score(tr: EnemyTrack) -> float:
            rng = distance(view.me.pos, tr.pos)
            # Prefer tracks near the engagement sweet spot, with recent fixes
            # and low uncertainty. Lower score is better.
            range_pen = abs(rng - PREFERRED_RANGE)
            stale_pen = max(0, tick - tr.last_seen_tick) * 1.5
            unc_pen = tr.uncertainty(tick) * 0.8
            ranged_bonus = 0.0 if tr.confirmed_ranged else 35.0
            return range_pen + stale_pen + unc_pen + ranged_bonus

        return min(self.state.tracks.values(), key=score)

    # =====================================================================
    # Gunnery
    # =====================================================================

    def _try_fire(self, cmd: Command, view: WorldView, target: EnemyTrack) -> None:
        my_pos = view.me.pos
        tick = view.tick

        # Need an active fix to be confident enough to shoot. If we haven't
        # had one recently, schedule one and skip firing this tick — the
        # sensor planner will see `pre_fire_ping_target` and burst active.
        if tick - target.last_ranged_tick > RANGED_STALE_TICKS:
            self.state.pre_fire_ping_target = self._track_id(target)
            return

        rng = distance(my_pos, target.pos)
        if rng > MAX_ENGAGEMENT_RANGE or rng > self._max_shell_range:
            return  # would clamp and miss

        # Compute the lead solution explicitly so we can score it.
        aim: Vec2 = target.pos
        speed = math.hypot(target.vel[0], target.vel[1])
        if speed > 0.5:
            predicted = lead_target(my_pos, target.pos, target.vel, self._shell_speed)
            if predicted is not None:
                aim = predicted

        flight_time = distance(my_pos, aim) / self._shell_speed
        if flight_time <= 0:
            return

        # Self-splash safety. The aim point must be far enough from us that
        # our own splash doesn't catch the ship.
        if distance(my_pos, aim) < self._splash_radius * SELF_SPLASH_RADIUS_MULTIPLIER:
            return

        # Expected miss combines track-position uncertainty (grown to impact)
        # with a velocity-extrapolation error term. Both are 1-sigma; we
        # add them in quadrature since they're roughly independent.
        pos_sigma = target.uncertainty(tick)
        vel_sigma = 1.0 + speed * 0.2  # crude: faster targets manoeuvre more
        miss = math.hypot(pos_sigma, vel_sigma * flight_time)

        expected_damage = self._splash_damage_at(miss)

        threshold = MIN_EXPECTED_DAMAGE
        if view.me.ammo <= LOW_AMMO_THRESHOLD:
            threshold = MIN_EXPECTED_DAMAGE_LOW_AMMO
        if view.me.hp <= LOW_HP_THRESHOLD:
            threshold *= 1.3

        if expected_damage < threshold:
            return

        cmd.fire_at(
            target.pos,
            shooter_pos=my_pos,
            target_vel=target.vel,
            shell_speed=self._shell_speed,
            lead=True,
        )
        self.state.next_fire_tick = tick + self._gun_cooldown
        self.state.last_fired_at_tick = tick
        self.state.pre_fire_ping_target = None
        log.debug(
            "fire: tick=%d range=%.1f miss=%.1f expDmg=%.1f ammo=%d",
            tick,
            rng,
            miss,
            expected_damage,
            view.me.ammo,
        )

    def _splash_damage_at(self, miss_distance: float) -> float:
        if miss_distance >= self._splash_radius:
            return 0.0
        return self._max_splash_damage * (1.0 - miss_distance / self._splash_radius)

    def _track_id(self, track: EnemyTrack) -> Optional[int]:
        for tid, tr in self.state.tracks.items():
            if tr is track:
                return tid
        return None

    # =====================================================================
    # Sensor scheduling
    # =====================================================================

    def _decide_sensor_mode(self, view: WorldView, target: Optional[EnemyTrack]) -> str:
        tick = view.tick

        # Burst already in progress: honour it.
        if tick < self.state.active_until_tick:
            return "active"

        cooled_down = tick - self.state.last_active_tick >= ACTIVE_COOLDOWN_TICKS

        # Pre-fire ping: gunnery layer asked for a fresh range fix.
        if cooled_down and self.state.pre_fire_ping_target is not None:
            return self._begin_active_burst(tick)

        # Blind sweep: no track at all, ping periodically to find someone.
        if not self.state.tracks and cooled_down:
            if tick - self.state.last_active_tick >= NO_CONTACT_PING_INTERVAL:
                return self._begin_active_burst(tick)

        # Target gone fully passive (only bearings) and is plausibly in range:
        # one ping to confirm.
        if (
            target is not None
            and cooled_down
            and not target.confirmed_ranged
            and target.last_passive_tick == tick
        ):
            est_range = distance(view.me.pos, target.pos)
            if est_range < MAX_ENGAGEMENT_RANGE + 50.0:
                return self._begin_active_burst(tick)

        return "passive"

    def _begin_active_burst(self, tick: int) -> str:
        self.state.active_until_tick = tick + ACTIVE_BURST_TICKS
        self.state.last_active_tick = tick
        return "active"

    # =====================================================================
    # Navigation
    # =====================================================================

    def _plan_heading(self, view: WorldView, target: Optional[EnemyTrack]) -> float:
        me = view.me
        tick = view.tick

        # Override 1: wall avoidance.
        wall_bearing = self._wall_avoidance_bearing(me.pos)
        if wall_bearing is not None:
            return wall_bearing

        # Override 2: active evasion.
        if tick < self.state.evade_until_tick:
            # Pick a course roughly perpendicular to our current heading —
            # makes us a moving target on the opponent's lead solution.
            base = (me.heading_deg + 90.0 * self.state.evade_rudder_sign) % 360.0
            return base

        # Engagement positioning.
        if target is not None:
            rng = distance(me.pos, target.pos)
            bearing_to_target = bearing_to(me.pos, target.pos)
            if rng > PREFERRED_RANGE + RANGE_BAND_HALF_WIDTH:
                desired = bearing_to_target  # close
            elif rng < PREFERRED_RANGE - RANGE_BAND_HALF_WIDTH:
                desired = (bearing_to_target + 180.0) % 360.0  # back off
            else:
                # In the sweet spot: orbit so guns stay on the target and
                # we keep generating parallax for passive triangulation.
                desired = (bearing_to_target + 90.0) % 360.0

            # Low HP: bias away from the target to gain disengagement room.
            if me.hp <= LOW_HP_THRESHOLD:
                desired = (bearing_to_target + 150.0) % 360.0

            return self._with_oscillation(desired, tick)

        # No target: drift toward the centre of the map while sweeping.
        centre = (self._map_w * 0.5, self._map_h * 0.5)
        return self._with_oscillation(bearing_to(me.pos, centre), tick)

    def _with_oscillation(self, base_bearing: float, tick: int) -> float:
        phase = (tick % COURSE_OSC_PERIOD_TICKS) / COURSE_OSC_PERIOD_TICKS
        offset = COURSE_OSC_AMPLITUDE_DEG * math.sin(2.0 * math.pi * phase)
        return (base_bearing + offset) % 360.0

    def _wall_avoidance_bearing(self, pos: Vec2) -> Optional[float]:
        x, y = pos
        push_x = 0.0
        push_y = 0.0
        if x < WALL_MARGIN:
            push_x = WALL_MARGIN - x
        elif x > self._map_w - WALL_MARGIN:
            push_x = -(x - (self._map_w - WALL_MARGIN))
        if y < WALL_MARGIN:
            push_y = WALL_MARGIN - y
        elif y > self._map_h - WALL_MARGIN:
            push_y = -(y - (self._map_h - WALL_MARGIN))
        if push_x == 0.0 and push_y == 0.0:
            return None
        target = (pos[0] + push_x, pos[1] + push_y)
        return bearing_to(pos, target)

    def _steer_toward(
        self,
        view: WorldView,
        desired_heading_deg: float,
        target: Optional[EnemyTrack],
    ) -> Tuple[float, float]:
        me = view.me
        tick = view.tick

        # Evasion: hard rudder, max throttle. Yaw rate scales with speed so we
        # need speed to turn at all.
        if tick < self.state.evade_until_tick:
            return self.state.evade_rudder_sign * EVADE_HARD_RUDDER, 1.0

        delta = _signed_bearing_delta(desired_heading_deg, me.heading_deg)
        rudder = max(-1.0, min(1.0, delta / 22.0))

        # Throttle policy: keep speed up so the rudder bites, but ease off in
        # tight turns so we don't overshoot. Pull throttle harder when the
        # target is in the kill zone and we want a smaller orbit.
        turn_severity = min(1.0, abs(delta) / 90.0)
        throttle = 0.95 - 0.45 * turn_severity

        # Low HP: push speed regardless of turn so we can break contact.
        if me.hp <= LOW_HP_THRESHOLD:
            throttle = max(throttle, 0.85)

        # Inside engagement band: a steady moderate speed gives the gunnery
        # layer a consistent firing platform.
        if target is not None:
            rng = distance(me.pos, target.pos)
            if abs(rng - PREFERRED_RANGE) < RANGE_BAND_HALF_WIDTH:
                throttle = min(throttle, 0.7)

        return rudder, throttle


# ---------------------------------------------------------------------------
# Entry point.
# ---------------------------------------------------------------------------


def main() -> None:
    logging.basicConfig(level=logging.INFO)
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--host", default="localhost")
    p.add_argument("--port", type=int, default=7878)
    p.add_argument("--name", default="tactician")
    p.add_argument("-v", "--verbose", action="store_true", help="Enable debug logging")
    args = p.parse_args()
    if args.verbose:
        logging.getLogger("tactician_bot").setLevel(logging.DEBUG)
    run(TacticianBot(), host=args.host, port=args.port, name=args.name)


if __name__ == "__main__":
    main()
