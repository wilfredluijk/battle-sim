"""Math helpers for bot authors.

Bearing convention matches the server (`server/src/sim/combat.rs::bearing_to_unit_vec`):
0° = north (-y), 90° = east (+x), increasing clockwise. All angles are in degrees.
"""

from __future__ import annotations

import math
from typing import Optional, Sequence, Tuple

Point = Sequence[float]  # [x, y]
Vec = Sequence[float]    # [vx, vy]


def distance(a: Point, b: Point) -> float:
    """Euclidean distance between two points."""
    dx = b[0] - a[0]
    dy = b[1] - a[1]
    return math.hypot(dx, dy)


def bearing_to(from_pos: Point, to_pos: Point) -> float:
    """Compass bearing in degrees from `from_pos` to `to_pos`.

    0° points along -y (up on the canvas), 90° along +x (right). Result is in [0, 360).
    """
    dx = to_pos[0] - from_pos[0]
    dy = to_pos[1] - from_pos[1]
    # bearing = atan2(dx, -dy) so that (dx=0, dy=-1) -> 0°, (dx=1, dy=0) -> 90°.
    rad = math.atan2(dx, -dy)
    deg = math.degrees(rad)
    if deg < 0.0:
        deg += 360.0
    return deg


def lead_target(
    shooter_pos: Point,
    target_pos: Point,
    target_vel: Vec,
    shell_speed: float,
) -> Optional[Tuple[float, float]]:
    """Predict the intercept point for a shell fired *now* at a moving target.

    Returns the predicted target position at impact, or `None` if no real
    intercept solution exists (target faster than the shell and running away).

    Solves `|target_pos + target_vel * t - shooter_pos| = shell_speed * t` for `t >= 0`.
    """
    if shell_speed <= 0.0:
        return None

    rx = target_pos[0] - shooter_pos[0]
    ry = target_pos[1] - shooter_pos[1]
    vx = target_vel[0]
    vy = target_vel[1]

    # Quadratic in t:  (v.v - s^2) t^2 + 2 (r.v) t + r.r = 0
    a = vx * vx + vy * vy - shell_speed * shell_speed
    b = 2.0 * (rx * vx + ry * vy)
    c = rx * rx + ry * ry

    t: Optional[float]
    if abs(a) < 1e-9:
        # Linear case: target speed equals shell speed.
        if abs(b) < 1e-9:
            t = 0.0 if c < 1e-9 else None
        else:
            candidate = -c / b
            t = candidate if candidate >= 0.0 else None
    else:
        disc = b * b - 4.0 * a * c
        if disc < 0.0:
            return None
        sqrt_disc = math.sqrt(disc)
        t1 = (-b - sqrt_disc) / (2.0 * a)
        t2 = (-b + sqrt_disc) / (2.0 * a)
        candidates = [x for x in (t1, t2) if x >= 0.0]
        t = min(candidates) if candidates else None

    if t is None:
        return None

    return (target_pos[0] + vx * t, target_pos[1] + vy * t)
