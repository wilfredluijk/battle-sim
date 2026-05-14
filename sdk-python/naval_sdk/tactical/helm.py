"""Steering helper.

Translates a desired bearing or waypoint into a ``(throttle, rudder)`` pair
that respects the speed-coupled turn rate of the ship (sharp turns need
less throttle, otherwise the rudder is wasted on a fast-but-straight hull).

See ``docs/design-decisions/sdk-tactical-toolkit.md`` §4.4.
"""

from __future__ import annotations

from typing import Tuple

from ..helpers import bearing_to, clamp, signed_bearing_delta
from ..protocol import SelfState, ShipSpecs


class Helm:
    def __init__(
        self,
        specs: ShipSpecs,
        *,
        map_width: float = 800.0,
        map_height: float = 800.0,
        wall_margin: float = 30.0,
        turn_aggression_deg: float = 30.0,
        align_threshold_deg: float = 10.0,
        min_turn_throttle: float = 0.55,
    ) -> None:
        self._specs = specs
        self._w = float(map_width)
        self._h = float(map_height)
        self._margin = float(wall_margin)
        self._turn_agg = float(turn_aggression_deg)
        self._align = float(align_threshold_deg)
        self._min_throttle = float(min_turn_throttle)

    def steer_to_bearing(
        self,
        me: SelfState,
        target_bearing_deg: float,
        *,
        respect_walls: bool = True,
        desired_throttle: float = 1.0,
    ) -> Tuple[float, float]:
        """Return ``(throttle, rudder)`` to align with ``target_bearing_deg``.

        Rudder is proportional to the signed bearing delta, scaled by
        ``turn_aggression_deg``. Throttle tapers toward ``min_turn_throttle``
        as the required turn grows, so the ship doesn't try to plough straight
        through a heading change.
        """
        bearing = (
            self._wall_override(me, target_bearing_deg) if respect_walls else target_bearing_deg
        )
        delta = signed_bearing_delta(bearing, me.heading_deg)
        rudder = clamp(delta / self._turn_agg, -1.0, 1.0)

        abs_delta = abs(delta)
        if abs_delta <= self._align:
            throttle = desired_throttle
        else:
            scale = (180.0 - abs_delta) / max(180.0 - self._align, 1e-6)
            scale = clamp(scale, 0.0, 1.0)
            throttle = self._min_throttle + (desired_throttle - self._min_throttle) * scale
        return throttle, rudder

    def steer_to_point(
        self,
        me: SelfState,
        target: Tuple[float, float],
        *,
        respect_walls: bool = True,
        desired_throttle: float = 1.0,
    ) -> Tuple[float, float]:
        """Convenience: steer toward a world-space point."""
        return self.steer_to_bearing(
            me,
            bearing_to(me.pos, target),
            respect_walls=respect_walls,
            desired_throttle=desired_throttle,
        )

    # -- Internals ---------------------------------------------------------

    def _wall_override(self, me: SelfState, target_bearing: float) -> float:
        """If we're inside the wall margin and the target points further into
        the wall, redirect toward an inward bearing instead.
        """
        x, y = me.pos
        push_x = 0.0
        push_y = 0.0
        if x < self._margin:
            push_x = 1.0
        elif x > self._w - self._margin:
            push_x = -1.0
        if y < self._margin:
            push_y = 1.0
        elif y > self._h - self._margin:
            push_y = -1.0
        if push_x == 0.0 and push_y == 0.0:
            return target_bearing

        # Compass bearing of the push vector.
        push_bearing = bearing_to((0.0, 0.0), (push_x, push_y))
        # If the target bearing already agrees with the push direction (within
        # 90°), leave it alone — we're heading away from the wall on our own.
        delta = abs(signed_bearing_delta(target_bearing, push_bearing))
        if delta <= 90.0:
            return target_bearing
        return push_bearing
