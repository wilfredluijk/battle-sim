"""Circle bot: drives in a circle and fires at random bearings.

The simplest possible "doing something" bot. It pins the rudder over to
trace a steady arc, paints with active radar, and once the gun cools
down lets a shell loose in a random direction. No tracking, no aiming —
purely a baseline for the other examples to outperform.

Run against the training server with your participant file:

    python examples/circle_bot.py --env-file player01.env --name circler
"""

from __future__ import annotations

import argparse
import logging
import random

from naval_sdk.cli import add_connection_arguments, connection_options
from naval_sdk import Bot, Command, FireCommand, WorldView, run
from naval_sdk.protocol import Welcome


class CircleBot(Bot):
    def __init__(self, seed: int = 0) -> None:
        super().__init__()
        # Seed RNG explicitly so two runs with the same seed behave identically
        # — useful when reproducing a match from a replay.
        self._rng = random.Random(seed)
        self._cooldown_ticks = 15  # overwritten from welcome
        self._next_fire_tick = 0

    def on_welcome(self, welcome: Welcome) -> None:
        self._cooldown_ticks = welcome.ship_specs.gun_cooldown_ticks

    def on_tick(self, view: WorldView) -> Command:
        # Half-ahead with a steady starboard rudder traces a wide circle.
        cmd = Command(throttle=0.6, rudder=0.4, sensor_mode="active")

        if view.me.ammo > 0 and view.tick >= self._next_fire_tick:
            bearing = self._rng.uniform(0.0, 360.0)
            # Fire near max range so the splash actually has a chance of
            # catching someone we didn't aim at.
            cmd.fire = FireCommand(bearing_deg=bearing, range=250.0)
            self._next_fire_tick = view.tick + self._cooldown_ticks

        return cmd


def main() -> None:
    logging.basicConfig(level=logging.INFO)
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    add_connection_arguments(p, default_url="wss://93.190.187.250/bot")
    p.add_argument("--name", default="circler")
    p.add_argument("--seed", type=int, default=0, help="RNG seed for the random-fire pattern")
    args = p.parse_args()
    connection = connection_options(p, args)
    run(CircleBot(seed=args.seed), name=args.name, **connection)


if __name__ == "__main__":
    main()
