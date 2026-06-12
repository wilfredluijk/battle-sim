"""Powerful bot: a competitive all-rounder.

Behaviour layers, from outer to inner:

  * **Wall avoidance.** A constant pressure away from the arena edges so
    we never beach the ship (running into a wall stops you dead and
    costs 2 HP).
  * **Threat reaction.** A nearby `ShellSplash` or a fresh `Hit` event
    triggers an evasive jink — flip rudder, kick throttle — for a few
    ticks.
  * **Sensor management.** Default to passive listening. Burst active
    when we hear something on passive or have nothing on track, so we
    don't paint ourselves on the radar all match.
  * **Target tracking.** Maintains a multi-tick track per contact with
    velocity smoothing, the same pattern as `tracking_bot.py` but
    multi-track and prioritised by predicted hit value.
  * **Engagement.** Lead the target with `lead_target`, prefer shots
    within ~70% of `max_shell_range` so splash actually catches the
    enemy, and respect cooldown / ammo / friendly-fire (don't fire if
    we're inside our own splash radius).
  * **Endgame.** When ammo is low, become miserly: fire only on
    high-confidence, in-range, well-led solutions.

The bot is deliberately *not* random — its behaviour is a pure function
of the WorldView history, so two runs against an identical opponent
produce the same outcome.

Run against a local server:

    python examples/powerful_bot.py --host localhost --port 7878 --name warlord
"""

from __future__ import annotations

import argparse
import logging
import math
from dataclasses import dataclass, field
from typing import Dict, List, Optional, Tuple

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

log = logging.getLogger("powerful_bot")

Vec2 = Tuple[float, float]


@dataclass
class Track:
    pos: Vec2
    vel: Vec2
    last_seen_tick: int
    last_ranged_tick: int
    hits: int = 0  # approximate; updated whenever we splash close to it

    def age(self, tick: int) -> int:
        return tick - self.last_seen_tick


@dataclass
class TacticalState:
    evade_until_tick: int = 0
    evade_rudder_sign: float = 1.0
    active_until_tick: int = 0
    last_active_tick: int = -1000
    tracks: Dict[int, Track] = field(default_factory=dict)
    next_track_id: int = 0


class PowerfulBot(Bot):
    # Tuning -- intentionally exposed as class constants so they are easy
    # to tweak from a subclass or in a debugger.
    GATE_DIST = 80.0
    PASSIVE_BEARING_TOL = 25.0
    TRACK_STALE_TICKS = 60
    EVADE_TICKS = 8
    ACTIVE_BURST_TICKS = 5
    ACTIVE_COOLDOWN_TICKS = 25
    SPLASH_SAFETY_MARGIN = 1.5  # multiplier on splash radius for self-safety
    WALL_MARGIN = 80.0
    PREFERRED_RANGE_FRAC = 0.7  # try to engage at 70% of max shell range
    LOW_AMMO_THRESHOLD = 5

    def __init__(self) -> None:
        super().__init__()
        self.state = TacticalState()
        # Fallback defaults matching the current server (`sim::constants`); these are
        # overwritten in `on_welcome` from the authoritative `ship_specs` payload.
        self._shell_speed = 70.0
        self._max_shell_range = 300.0
        self._splash_radius = 15.0
        self._gun_cooldown = 15
        self._max_ammo = 250
        self._map_w = 700
        self._map_h = 700
        self._next_fire_tick = 0

    def on_welcome(self, welcome: Welcome) -> None:
        specs = welcome.ship_specs
        self._shell_speed = specs.shell_speed
        self._max_shell_range = specs.max_shell_range
        self._splash_radius = specs.splash_radius
        self._gun_cooldown = specs.gun_cooldown_ticks
        self._max_ammo = specs.max_ammo
        self._map_w = welcome.map.width
        self._map_h = welcome.map.height
        log.info(
            "specs: shell_speed=%.1f max_range=%.1f splash=%.1f cooldown=%d ammo=%d",
            self._shell_speed,
            self._max_shell_range,
            self._splash_radius,
            self._gun_cooldown,
            self._max_ammo,
        )

    def on_game_start(self, tick, starting_position, starting_heading_deg) -> None:
        # Monte-Carlo mode reuses this bot across many matches; the server resets
        # ``world.tick`` to 0 and sends only ``game_start`` (no fresh ``welcome``).
        # All match-scoped runtime state lives in ``self.state`` (tracks, the
        # track-id counter, evade/active-burst counters) plus ``self._next_fire_tick``.
        # Rebuild it so stale tracks from the previous match don't survive as
        # immortal "ghosts" with ``tick - last_seen_tick < 0`` that never prune.
        # Welcome-derived config (specs, map dims) is intentionally preserved.
        self.state = TacticalState()
        self._next_fire_tick = 0

    # ---- main loop --------------------------------------------------------

    def on_tick(self, view: WorldView) -> Command:
        me = view.me
        self._react_to_events(view)
        self._update_tracks(view)

        sensor_mode = self._pick_sensor_mode(view)
        rudder, throttle = self._navigate(view, sensor_mode)

        cmd = Command(throttle=throttle, rudder=rudder, sensor_mode=sensor_mode)

        target = self._pick_target(view)
        if target is not None and me.ammo > 0 and view.tick >= self._next_fire_tick:
            if self._try_shoot(cmd, view, target):
                self._next_fire_tick = view.tick + self._gun_cooldown

        return cmd

    # ---- events -----------------------------------------------------------

    def _react_to_events(self, view: WorldView) -> None:
        tick = view.tick
        for ev in view.events:
            if isinstance(ev, HitEvent):
                log.debug("hit for %d at tick %d", ev.amount, tick)
                self._start_evade(tick, flip=True)
            elif isinstance(ev, ShellSplashEvent):
                if distance(ev.pos, view.me.pos) < self._splash_radius * 4.0:
                    self._start_evade(tick, flip=False)

    def _start_evade(self, tick: int, flip: bool) -> None:
        if flip:
            self.state.evade_rudder_sign = -self.state.evade_rudder_sign
        if self.state.evade_until_tick < tick + self.EVADE_TICKS:
            self.state.evade_until_tick = tick + self.EVADE_TICKS

    # ---- sensor scheduling ------------------------------------------------

    def _pick_sensor_mode(self, view: WorldView) -> str:
        tick = view.tick

        # Stay active if we asked for a burst earlier and it hasn't expired.
        if tick < self.state.active_until_tick:
            return "active"

        passive_contacts = [c for c in view.contacts if c.kind in ("ship", "unknown")]
        heard_something = bool(passive_contacts)
        have_fresh_track = any(
            tick - t.last_ranged_tick <= 6 for t in self.state.tracks.values()
        )
        active_cooled = tick - self.state.last_active_tick >= self.ACTIVE_COOLDOWN_TICKS

        if active_cooled and (heard_something or not have_fresh_track):
            self.state.active_until_tick = tick + self.ACTIVE_BURST_TICKS
            self.state.last_active_tick = tick
            return "active"
        return "passive"

    # ---- navigation -------------------------------------------------------

    def _navigate(self, view: WorldView, sensor_mode: str) -> Tuple[float, float]:
        me = view.me
        tick = view.tick

        if tick < self.state.evade_until_tick:
            # Evasive: hard rudder one way, throttle hot.
            return self.state.evade_rudder_sign * 1.0, 1.0

        target = self._best_track(tick)
        desired_bearing = self._heading_for_engagement(view, target)

        # Steer away from walls if we're close to them.
        wall_push = self._wall_avoidance_bearing(me.pos, me.heading_deg)
        if wall_push is not None:
            desired_bearing = wall_push

        delta = _signed_bearing_delta(desired_bearing, me.heading_deg)
        rudder = max(-1.0, min(1.0, delta / 25.0))

        # Slow down when we need to turn hard so the rudder bites harder
        # (yaw rate scales with speed, but turning circle scales with speed^2).
        turn_severity = min(1.0, abs(delta) / 90.0)
        throttle = 1.0 - 0.4 * turn_severity
        return rudder, throttle

    def _heading_for_engagement(self, view: WorldView, target: Optional[Track]) -> float:
        me = view.me
        if target is None:
            # Search pattern: gentle starboard arc through the map centre.
            centre = (self._map_w * 0.5, self._map_h * 0.5)
            return bearing_to(me.pos, centre)

        rng = distance(me.pos, target.pos)
        preferred = self._max_shell_range * self.PREFERRED_RANGE_FRAC
        bearing = bearing_to(me.pos, target.pos)
        if rng > preferred * 1.2:
            return bearing  # close the gap
        if rng < preferred * 0.6:
            return (bearing + 180.0) % 360.0  # back off, we're inside our splash risk
        # Loiter perpendicular so we keep guns on target without closing.
        return (bearing + 90.0) % 360.0

    def _wall_avoidance_bearing(self, pos: Vec2, heading: float) -> Optional[float]:
        x, y = pos
        push_x = 0.0
        push_y = 0.0
        if x < self.WALL_MARGIN:
            push_x += (self.WALL_MARGIN - x)
        elif x > self._map_w - self.WALL_MARGIN:
            push_x -= (x - (self._map_w - self.WALL_MARGIN))
        if y < self.WALL_MARGIN:
            push_y += (self.WALL_MARGIN - y)
        elif y > self._map_h - self.WALL_MARGIN:
            push_y -= (y - (self._map_h - self.WALL_MARGIN))
        if push_x == 0.0 and push_y == 0.0:
            return None
        target = (pos[0] + push_x, pos[1] + push_y)
        return bearing_to(pos, target)

    # ---- tracking ---------------------------------------------------------

    def _update_tracks(self, view: WorldView) -> None:
        tick = view.tick
        dt = 0.1

        # 1. Dead-reckon every existing track.
        for tid, tr in list(self.state.tracks.items()):
            self.state.tracks[tid] = Track(
                pos=(tr.pos[0] + tr.vel[0] * dt, tr.pos[1] + tr.vel[1] * dt),
                vel=tr.vel,
                last_seen_tick=tr.last_seen_tick,
                last_ranged_tick=tr.last_ranged_tick,
                hits=tr.hits,
            )

        # 2. Sort contacts deterministically; ranged first so we attach the
        # high-quality fix before considering bearings.
        ship_contacts = sorted(
            (c for c in view.contacts if c.kind in ("ship", "unknown")),
            key=lambda c: (c.range is None, c.id),
        )

        for contact in ship_contacts:
            if contact.range is not None:
                self._fold_ranged_contact(contact, view.me.pos, tick)
            else:
                self._fold_passive_contact(contact, view.me.pos, tick)

        # 3. Drop stale tracks.
        for tid in [tid for tid, tr in self.state.tracks.items() if tick - tr.last_seen_tick > self.TRACK_STALE_TICKS]:
            log.debug("dropping stale track %d", tid)
            del self.state.tracks[tid]

    def _fold_ranged_contact(self, contact: Contact, my_pos: Vec2, tick: int) -> None:
        tid = self._associate_ranged(contact.pos)
        if tid is None:
            tid = self.state.next_track_id
            self.state.next_track_id += 1
            self.state.tracks[tid] = Track(
                pos=contact.pos,
                vel=(0.0, 0.0),
                last_seen_tick=tick,
                last_ranged_tick=tick,
            )
            return

        prev = self.state.tracks[tid]
        dt_ticks = max(1, tick - prev.last_ranged_tick)
        dt = dt_ticks * 0.1
        vx = (contact.pos[0] - prev.pos[0]) / dt
        vy = (contact.pos[1] - prev.pos[1]) / dt
        # Filter velocity to dampen noise (~2-unit position jitter at 10Hz
        # would otherwise inject 20 units/s of velocity noise per axis).
        smoothed = (0.6 * vx + 0.4 * prev.vel[0], 0.6 * vy + 0.4 * prev.vel[1])
        self.state.tracks[tid] = Track(
            pos=contact.pos,
            vel=smoothed,
            last_seen_tick=tick,
            last_ranged_tick=tick,
            hits=prev.hits,
        )

    def _fold_passive_contact(self, contact: Contact, my_pos: Vec2, tick: int) -> None:
        if not self.state.tracks:
            return
        # Match against the track with the closest predicted bearing.
        best_tid: Optional[int] = None
        best_delta = self.PASSIVE_BEARING_TOL
        for tid, tr in self.state.tracks.items():
            expected = bearing_to(my_pos, tr.pos)
            delta = _abs_bearing_delta(contact.bearing_deg, expected)
            if delta < best_delta:
                best_delta = delta
                best_tid = tid
        if best_tid is None:
            return
        tr = self.state.tracks[best_tid]
        self.state.tracks[best_tid] = Track(
            pos=tr.pos,
            vel=tr.vel,
            last_seen_tick=tick,
            last_ranged_tick=tr.last_ranged_tick,
            hits=tr.hits,
        )

    def _associate_ranged(self, pos: Vec2) -> Optional[int]:
        best_tid: Optional[int] = None
        best_dist = self.GATE_DIST
        for tid, tr in self.state.tracks.items():
            d = distance(pos, tr.pos)
            if d < best_dist:
                best_dist = d
                best_tid = tid
        return best_tid

    # ---- targeting --------------------------------------------------------

    def _best_track(self, tick: int) -> Optional[Track]:
        if not self.state.tracks:
            return None
        # Prefer recent, ranged tracks; tie-break by closeness to preferred range.
        def score(tr: Track) -> float:
            age = max(0, tick - tr.last_ranged_tick)
            return age  # lower is better
        return min(self.state.tracks.values(), key=score)

    def _pick_target(self, view: WorldView) -> Optional[Track]:
        tick = view.tick
        candidates = [
            tr for tr in self.state.tracks.values()
            if tick - tr.last_ranged_tick <= 15  # don't shoot purely-passive ghosts
        ]
        if not candidates:
            return None
        # Score by range to preferred and recency.
        preferred = self._max_shell_range * self.PREFERRED_RANGE_FRAC
        def score(tr: Track) -> float:
            rng = distance(view.me.pos, tr.pos)
            return abs(rng - preferred) + (tick - tr.last_ranged_tick) * 2.0
        return min(candidates, key=score)

    def _try_shoot(self, cmd: Command, view: WorldView, target: Track) -> bool:
        my_pos = view.me.pos
        rng = distance(my_pos, target.pos)
        if rng > self._max_shell_range:
            return False  # would just clamp and miss

        # Compute lead point so we can check self-splash.
        aim: Vec2 = target.pos
        if target.vel != (0.0, 0.0):
            predicted = lead_target(my_pos, target.pos, target.vel, self._shell_speed)
            if predicted is not None:
                aim = predicted

        if distance(my_pos, aim) < self._splash_radius * self.SPLASH_SAFETY_MARGIN:
            log.debug("skipping shot: aim too close, would splash myself")
            return False

        # Low-ammo discipline: only shoot when we're confident.
        if view.me.ammo <= self.LOW_AMMO_THRESHOLD:
            tick = view.last_tick
            if tick - target.last_ranged_tick > 4:
                return False  # need a very fresh fix
            if rng > self._max_shell_range * 0.85:
                return False  # need to be well inside range

        cmd.fire_at(
            target.pos,
            shooter_pos=my_pos,
            target_vel=target.vel,
            shell_speed=self._shell_speed,
            lead=True,
        )
        return True


# ---- small helpers ----------------------------------------------------------


def _signed_bearing_delta(target: float, current: float) -> float:
    return ((target - current + 540.0) % 360.0) - 180.0


def _abs_bearing_delta(a: float, b: float) -> float:
    return abs(_signed_bearing_delta(a, b))


def main() -> None:
    logging.basicConfig(level=logging.INFO)
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--host", default="localhost")
    p.add_argument("--port", type=int, default=7878)
    p.add_argument("--name", default="warlord")
    args = p.parse_args()
    run(PowerfulBot(), host=args.host, port=args.port, name=args.name)


if __name__ == "__main__":
    main()
