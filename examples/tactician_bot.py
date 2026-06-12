"""Tactician bot: stealth-first, evidence-based naval combat.

Uses the *Layer 2* tactical toolkit (``Tracker``, ``Gunner``, ``Helm``,
``Evader``, ``SensorPolicy``) for the mechanics — contact association,
velocity smoothing, cooldown, lead, evasion — and keeps the *tactics* as
custom code in ``on_tick``: range-band orbit, HP-aware course adjustments,
and a stealth-biased ``PingWhenStale`` sensor policy.

Doctrine in one sentence: passive by default, active only when the tracker
has lost the picture, fire only when a vetted shot is available.

Run against a local server:

    python examples/tactician_bot.py --host localhost --port 7878 --name tactician
"""

from __future__ import annotations

import argparse
import logging

from naval_sdk import Bot, Command, WorldView, bearing_to, distance, run
from naval_sdk.protocol import Welcome
from naval_sdk.tactical import (
    Evader,
    Gunner,
    Helm,
    PingWhenStale,
    Tracker,
)
from naval_sdk.tactical.tracker import Track

log = logging.getLogger("tactician_bot")

PREFERRED_RANGE = 180.0
RANGE_BAND_HALF_WIDTH = 40.0
LOW_HP_THRESHOLD = 30


class TacticianBot(Bot):
    def __init__(self) -> None:
        super().__init__()
        self.tracker: Tracker | None = None
        self.gunner: Gunner | None = None
        self.helm: Helm | None = None
        self.evader = Evader(evasion_ticks=14, cooldown_ticks=8)
        self.sensor_policy = PingWhenStale(stale_threshold_ticks=15)

    def on_welcome(self, welcome: Welcome) -> None:
        specs = welcome.ship_specs
        self.tracker = Tracker(specs, tick_hz=welcome.tick_hz)
        self.gunner = Gunner(specs, self_splash_margin=2.0)
        self.helm = Helm(
            specs,
            map_width=float(welcome.map.width),
            map_height=float(welcome.map.height),
            wall_margin=90.0,
        )

    def on_game_start(self, tick, starting_position, starting_heading_deg) -> None:
        # Monte-Carlo mode reuses this bot across matches; the server resets the
        # tick to 0 and sends only ``game_start``. Clear match-scoped state so a
        # carried-over track doesn't become an immortal "ghost" the bot chases
        # forever, and reset the evader's state machine. Welcome-derived config
        # (specs, map, helm) is preserved.
        if self.tracker is not None:
            self.tracker.reset()
        self.evader.reset()

    def on_tick(self, view: WorldView) -> Command:
        assert self.tracker and self.gunner and self.helm

        tracks = self.tracker.update(view)
        ships = [t for t in tracks if t.kind == "ship"]
        sensor = self.sensor_policy.choose(view, self.tracker)

        # 1. Evasion preempts everything.
        evade = self.evader.update(view)
        if evade is not None:
            evade.sensor_mode = sensor
            return evade

        # 2. No threats: drift toward map centre while sweeping.
        if not ships:
            cx = (self.welcome.map.width / 2.0, self.welcome.map.height / 2.0)
            throttle, rudder = self.helm.steer_to_point(view.me, cx)
            return Command(throttle=throttle, rudder=rudder, sensor_mode=sensor)

        # 3. Pick the highest-priority threat.
        target = self._best_target(view, ships)

        # 4. Plan heading: stay at the preferred range band, or break off if hurt.
        bearing = self._engagement_bearing(view, target)
        throttle, rudder = self.helm.steer_to_bearing(view.me, bearing)

        cmd = Command(throttle=throttle, rudder=rudder, sensor_mode=sensor)

        # 5. Take the shot if Gunner approves.
        self.gunner.attempt(cmd, view.me, target, view)
        return cmd

    # -- tactics ----------------------------------------------------------

    def _best_target(self, view: WorldView, ships: list) -> Track:
        """Prefer fresh tracks near the engagement sweet spot."""

        def score(t: Track) -> float:
            rng = distance(view.me.pos, t.pos)
            range_pen = abs(rng - PREFERRED_RANGE)
            stale_pen = max(0, view.tick - t.last_seen_tick) * 1.5
            dead_reckon_pen = 35.0 if t.source != "active" else 0.0
            return range_pen + stale_pen + dead_reckon_pen

        return min(ships, key=score)

    def _engagement_bearing(self, view: WorldView, target: Track) -> float:
        """Range-band orbit: close, back off, or orbit perpendicular."""
        me = view.me
        rng = distance(me.pos, target.pos)
        to_target = bearing_to(me.pos, target.pos)

        # Low HP: break away from the engagement.
        if me.hp <= LOW_HP_THRESHOLD:
            return (to_target + 150.0) % 360.0

        if rng > PREFERRED_RANGE + RANGE_BAND_HALF_WIDTH:
            return to_target  # close
        if rng < PREFERRED_RANGE - RANGE_BAND_HALF_WIDTH:
            return (to_target + 180.0) % 360.0  # back off
        return (to_target + 90.0) % 360.0  # orbit perpendicular


def main() -> None:
    logging.basicConfig(level=logging.INFO)
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--host", default="localhost")
    p.add_argument("--port", type=int, default=7878)
    p.add_argument("--name", default="tactician")
    p.add_argument("-v", "--verbose", action="store_true")
    args = p.parse_args()
    if args.verbose:
        logging.getLogger("tactician_bot").setLevel(logging.DEBUG)
    run(TacticianBot(), host=args.host, port=args.port, name=args.name)


if __name__ == "__main__":
    main()
