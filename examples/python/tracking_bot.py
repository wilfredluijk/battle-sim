"""Tracking bot: orbits while keeping a stable track on the nearest enemy.

Demonstrates the *Layer 2* tactical toolkit: a hand-written ``on_tick`` that
plugs in :class:`Tracker` and :class:`Gunner` instead of re-implementing
contact association, velocity smoothing, cooldown tracking, and lead
computation. The hull does a steady orbit (constant rudder + throttle); all
the cleverness is in stitching noisy contacts into a usable target estimate
and converting that estimate into a vetted shot.

Run against a local server:

    python examples/python/tracking_bot.py --host localhost --port 7878 --name tracker
"""

from __future__ import annotations

import argparse
import logging

from naval_sdk import Bot, Command, WorldView, run
from naval_sdk.protocol import Welcome
from naval_sdk.tactical import DutyCycle, Gunner, Tracker

log = logging.getLogger("tracking_bot")


class TrackingBot(Bot):
    def __init__(self) -> None:
        super().__init__()
        self.tracker: Tracker | None = None
        self.gunner: Gunner | None = None
        self.sensor_policy = DutyCycle(active_ticks=4, passive_ticks=8)

    def on_welcome(self, welcome: Welcome) -> None:
        self.tracker = Tracker(welcome.ship_specs, tick_hz=welcome.tick_hz)
        self.gunner = Gunner(welcome.ship_specs)

    def on_tick(self, view: WorldView) -> Command:
        assert self.tracker is not None and self.gunner is not None

        tracks = self.tracker.update(view)
        ships = [t for t in tracks if t.kind == "ship"]

        cmd = Command(
            throttle=0.7,
            rudder=0.5,
            sensor_mode=self.sensor_policy.choose(view, self.tracker),
        )

        if ships:
            target = min(
                ships,
                key=lambda t: (t.pos[0] - view.me.pos[0]) ** 2 + (t.pos[1] - view.me.pos[1]) ** 2,
            )
            self.gunner.attempt(cmd, view.me, target, view)

        return cmd


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
