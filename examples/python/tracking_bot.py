"""Tracking bot: maintains a target track across ticks while orbiting.

Contact `id`s are per-tick — the SDK docs are clear about that. This bot
demonstrates how to build a *stable* track of an enemy ship by:

  1. Switching between active (ranged) and passive (bearing-only) sweeps
     so we keep eyes on the target without lighting up every tick.
  2. Associating each tick's contacts with the running track using a
     gating distance in active mode and a bearing tolerance in passive.
  3. Estimating target velocity from the position delta between active
     pings, then dead-reckoning the position during passive ticks.
  4. Using the velocity estimate to lead the shot via the SDK's
     `fire_at(..., target_vel=...)` helper.

While doing all that the ship keeps a steady turn on — it's an orbit, not
a chase.

Run against a local server:

    python examples/python/tracking_bot.py --host localhost --port 7878 --name tracker
"""

from __future__ import annotations

import argparse
import logging
import math
from dataclasses import dataclass
from typing import Optional, Tuple

from naval_sdk import Bot, Command, Contact, WorldView, bearing_to, run
from naval_sdk.protocol import Welcome

log = logging.getLogger("tracking_bot")


@dataclass
class Track:
    """A persistent estimate of one enemy ship across ticks."""

    pos: Tuple[float, float]
    vel: Tuple[float, float]
    last_seen_tick: int
    last_ranged_tick: int  # last tick we had a real range (active hit)


class TrackingBot(Bot):
    # Tunables. These are intentionally loose so the bot stays robust to
    # sensor noise and to brief sight losses.
    GATE_DIST = 60.0           # active: max jump between consecutive ticks
    PASSIVE_BEARING_TOL = 20.0  # passive: max bearing delta to count as same target
    TRACK_STALE_TICKS = 40     # drop the track after this long without a hit
    ACTIVE_BURST_TICKS = 4     # ping for this many ticks at a time
    PASSIVE_LISTEN_TICKS = 8   # then go silent this long

    def __init__(self) -> None:
        super().__init__()
        self._track: Optional[Track] = None
        self._sensor_phase = 0  # tick counter inside the active/passive cycle
        self._shell_speed = 50.0
        self._max_shell_range = 300.0
        self._gun_cooldown = 15
        self._next_fire_tick = 0

    def on_welcome(self, welcome: Welcome) -> None:
        specs = welcome.ship_specs
        self._shell_speed = specs.shell_speed
        self._max_shell_range = specs.max_shell_range
        self._gun_cooldown = specs.gun_cooldown_ticks

    # ---- sensor scheduling ------------------------------------------------

    def _sensor_mode_for_tick(self) -> str:
        cycle = self.ACTIVE_BURST_TICKS + self.PASSIVE_LISTEN_TICKS
        phase = self._sensor_phase % cycle
        self._sensor_phase += 1
        return "active" if phase < self.ACTIVE_BURST_TICKS else "passive"

    # ---- contact association ---------------------------------------------

    def _update_track(self, view: WorldView) -> None:
        """Fold this tick's contacts into the running track estimate."""

        # Dead-reckon first so association uses the predicted position.
        if self._track is not None:
            dt = 0.1  # tick rate is 10 Hz; matches the server's fixed step
            predicted = (
                self._track.pos[0] + self._track.vel[0] * dt,
                self._track.pos[1] + self._track.vel[1] * dt,
            )
            self._track = Track(
                pos=predicted,
                vel=self._track.vel,
                last_seen_tick=self._track.last_seen_tick,
                last_ranged_tick=self._track.last_ranged_tick,
            )

        ship_contacts = [c for c in view.contacts if c.kind in ("ship", "unknown")]
        if not ship_contacts:
            self._maybe_drop_stale(view.tick)
            return

        ranged = [c for c in ship_contacts if c.range is not None]
        bearings = [c for c in ship_contacts if c.range is None]

        if ranged:
            # Active mode: full position. Either match against the existing
            # track by distance gating, or seed a new track on the nearest one.
            match = self._match_ranged(ranged)
            self._fold_ranged(match, view.tick)
        elif bearings and self._track is not None:
            # Passive mode: refine the track's heading from a bearing-only hit
            # if it falls inside our gate.
            self._fold_passive(bearings, view.tick)
        else:
            self._maybe_drop_stale(view.tick)

    def _match_ranged(self, ranged: list[Contact]) -> Contact:
        if self._track is None:
            return min(ranged, key=lambda c: _hypot(c.pos))
        track_pos = self._track.pos
        within_gate = [
            c for c in ranged if _dist(c.pos, track_pos) <= self.GATE_DIST
        ]
        if within_gate:
            return min(within_gate, key=lambda c: _dist(c.pos, track_pos))
        # Lost the previous target — re-acquire the closest fresh one.
        return min(ranged, key=lambda c: _dist(c.pos, track_pos))

    def _fold_ranged(self, contact: Contact, tick: int) -> None:
        new_pos = contact.pos
        if self._track is None or tick - self._track.last_ranged_tick > self.TRACK_STALE_TICKS:
            # Bootstrap: no usable velocity yet.
            self._track = Track(pos=new_pos, vel=(0.0, 0.0), last_seen_tick=tick, last_ranged_tick=tick)
            return

        dt_ticks = max(1, tick - self._track.last_ranged_tick)
        dt = dt_ticks * 0.1
        vx = (new_pos[0] - self._track.pos[0]) / dt
        vy = (new_pos[1] - self._track.pos[1]) / dt
        # Light low-pass filter so a single noisy fix doesn't wreck the lead.
        old_vx, old_vy = self._track.vel
        smoothed = (0.5 * vx + 0.5 * old_vx, 0.5 * vy + 0.5 * old_vy)
        self._track = Track(pos=new_pos, vel=smoothed, last_seen_tick=tick, last_ranged_tick=tick)

    def _fold_passive(self, bearings: list[Contact], tick: int) -> None:
        assert self._track is not None
        expected_brg = bearing_to(self.me_pos, self._track.pos)
        # Pick the contact whose bearing best matches what we expect.
        best = min(bearings, key=lambda c: _bearing_delta(c.bearing_deg, expected_brg))
        if _bearing_delta(best.bearing_deg, expected_brg) > self.PASSIVE_BEARING_TOL:
            self._maybe_drop_stale(tick)
            return
        # No range, but we have a fresh bearing — mark the track as still seen.
        self._track = Track(
            pos=self._track.pos,
            vel=self._track.vel,
            last_seen_tick=tick,
            last_ranged_tick=self._track.last_ranged_tick,
        )

    def _maybe_drop_stale(self, tick: int) -> None:
        if self._track is None:
            return
        if tick - self._track.last_seen_tick > self.TRACK_STALE_TICKS:
            log.debug("dropping stale track at tick %d", tick)
            self._track = None

    # ---- main control loop ------------------------------------------------

    def on_tick(self, view: WorldView) -> Command:
        self.me_pos = view.me.pos  # for the helpers above
        self._update_track(view)

        # The mandate is: travel in a circle. Throttle and rudder are constant.
        cmd = Command(throttle=0.7, rudder=0.5, sensor_mode=self._sensor_mode_for_tick())

        track = self._track
        if (
            track is not None
            and view.me.ammo > 0
            and view.tick >= self._next_fire_tick
            and track.last_ranged_tick != 0  # need at least one real fix
        ):
            rng = _dist(view.me.pos, track.pos)
            if rng <= self._max_shell_range:
                cmd.fire_at(
                    track.pos,
                    shooter_pos=view.me.pos,
                    target_vel=track.vel,
                    shell_speed=self._shell_speed,
                    lead=True,
                )
                self._next_fire_tick = view.tick + self._gun_cooldown

        return cmd


# ---- small helpers (kept local — the SDK exposes the ones we actually need) --


def _dist(a: Tuple[float, float], b: Tuple[float, float]) -> float:
    return math.hypot(b[0] - a[0], b[1] - a[1])


def _hypot(p: Tuple[float, float]) -> float:
    return math.hypot(p[0], p[1])


def _bearing_delta(a: float, b: float) -> float:
    d = (a - b + 540.0) % 360.0 - 180.0
    return abs(d)


def main() -> None:
    logging.basicConfig(level=logging.INFO)
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--host", default="localhost")
    p.add_argument("--port", type=int, default=7878)
    p.add_argument("--name", default="tracker")
    args = p.parse_args()
    run(TrackingBot(), host=args.host, port=args.port, name=args.name)


if __name__ == "__main__":
    main()
