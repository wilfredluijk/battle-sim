"""Loadout bot: picks two powerups and activates them tactically.

Demonstrates the full powerup workflow:

  1. `choose_powerups` returns ``["rapid_fire", "heavy_shell"]`` — a burst-damage
     synergy. The SDK sends `select_powerups` before `ready`.
  2. During the match the bot holds both picks until it has a ranged sensor contact
     close enough to abuse the rapid-fire window. When that happens it activates
     `rapid_fire` and then `heavy_shell` on the next tick, chains a salvo, and finishes
     with normal commands once the buffs expire.

See ``docs/POWERUPS.md`` for the full powerup catalog and other synergy recipes.

Run against a local server (start one with ``cargo run -- --port 7878`` first)::

    python examples/loadout_bot.py --host localhost --port 7878 --name burst
"""

from __future__ import annotations

import argparse
import logging
from typing import List

from naval_sdk import Bot, Command, FireCommand, WorldView, run
from naval_sdk.protocol import Welcome

log = logging.getLogger("loadout_bot")


class LoadoutBot(Bot):
    # Tunables — keep loadout choice and trigger range visible at the top so other
    # authors can riff on the strategy without trawling through `on_tick`.
    LOADOUT = ["rapid_fire", "heavy_shell"]
    BURST_TRIGGER_RANGE = 220.0
    LOW_HP_FALLBACK_TICK = 100  # fire the loadout no later than this tick

    def __init__(self) -> None:
        super().__init__()
        self._shell_speed = 70.0
        self._max_range = 300.0

    def on_welcome(self, welcome: Welcome) -> None:
        # Track useful gameplay constants so the aiming math reflects any operator
        # rebalance done via /api/room/config.
        self._shell_speed = welcome.ship_specs.shell_speed
        self._max_range = welcome.ship_specs.max_shell_range

    def choose_powerups(self, welcome: Welcome) -> List[str]:
        # Server validates the picks. If `LOADOUT` ever drifts from
        # `welcome.available_powerups`, the server will reply with a typed `error`
        # frame that `on_error` will log.
        return list(self.LOADOUT)

    def on_tick(self, view: WorldView) -> Command:
        # Default behaviour: half-throttle, mild turn, active radar so we can spot
        # someone to dump damage onto.
        cmd = Command(throttle=0.7, rudder=0.2, sensor_mode="active")

        # Pick the nearest ranged contact (active sensors give us range).
        target = view.nearest_contact()

        # Fire when ammo + cooldown allow it; the SDK leaves cooldown enforcement to
        # the server but reading `me.ammo` keeps the buffer warm.
        if target is not None and target.range is not None and view.me.ammo > 0:
            cmd.fire_at(
                target.pos,
                shooter_pos=view.me.pos,
                shell_speed=self._shell_speed,
                range=min(self._max_range, target.range + 10.0),
                lead=False,
            )

        # Trigger the burst combo when:
        # - a target is in range, OR
        # - we're past the fallback tick (don't sit on a loadout the whole match).
        target_in_range = (
            target is not None
            and target.range is not None
            and target.range <= self.BURST_TRIGGER_RANGE
        )
        force_pop = view.tick >= self.LOW_HP_FALLBACK_TICK or view.me.hp < 60

        if (target_in_range or force_pop) and view.me.powerup_ready("rapid_fire"):
            cmd.activate_powerup = "rapid_fire"
        elif view.me.powerup_active("rapid_fire") and view.me.powerup_ready("heavy_shell"):
            # Heavy shell buff is applied to *outgoing* shells, so activate it the
            # tick after rapid_fire so the second salvo carries the buff.
            cmd.activate_powerup = "heavy_shell"

        return cmd


def main() -> None:
    logging.basicConfig(level=logging.INFO)
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--host", default="localhost")
    p.add_argument("--port", type=int, default=7878)
    p.add_argument("--name", default="burst")
    args = p.parse_args()
    run(LoadoutBot(), host=args.host, port=args.port, name=args.name)


if __name__ == "__main__":
    main()
