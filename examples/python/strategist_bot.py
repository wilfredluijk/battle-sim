"""Strategist bot: pure Layer-3 tactical decision-making.

This bot is the showcase for the toolkit's high-level layer. It subclasses
``TacticalBot`` and overrides ``decide()`` — that's all. The framework wires
up tracking, fire control, steering, sensor scheduling, and evasion. The
bot author writes *intent*, not plumbing.

Run against a local server:

    python examples/python/strategist_bot.py --host localhost --port 7878 --name strategist
"""

from __future__ import annotations

import argparse
import logging

from naval_sdk import run
from naval_sdk.tactical import Intent, PingWhenStale, TacticalBot, TacticalContext

log = logging.getLogger("strategist_bot")

LOW_HP = 30


class StrategistBot(TacticalBot):
    sensor_policy = PingWhenStale(stale_threshold_ticks=12)

    def decide(self, ctx: TacticalContext) -> Intent:
        # Hurt? Run for the corner furthest from the nearest threat.
        if ctx.me.hp < LOW_HP and ctx.threats:
            return Intent.retreat_to(self._safest_corner(ctx))

        # See a threat? Engage the closest.
        if ctx.threats:
            return Intent.engage(ctx.threats.nearest())

        # No contacts: patrol the central box to find someone.
        return Intent.patrol(
            rect=(
                ctx.map_width * 0.25,
                ctx.map_height * 0.25,
                ctx.map_width * 0.75,
                ctx.map_height * 0.75,
            )
        )

    @staticmethod
    def _safest_corner(ctx: TacticalContext):
        threat = ctx.threats.nearest()
        assert threat is not None
        margin = 60.0
        corners = [
            (margin, margin),
            (ctx.map_width - margin, margin),
            (margin, ctx.map_height - margin),
            (ctx.map_width - margin, ctx.map_height - margin),
        ]
        return max(
            corners,
            key=lambda c: (c[0] - threat.pos[0]) ** 2 + (c[1] - threat.pos[1]) ** 2,
        )


def main() -> None:
    logging.basicConfig(level=logging.INFO)
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--host", default="localhost")
    p.add_argument("--port", type=int, default=7878)
    p.add_argument("--name", default="strategist")
    args = p.parse_args()
    run(StrategistBot(), host=args.host, port=args.port, name=args.name)


if __name__ == "__main__":
    main()
