import math

import pytest

from naval_sdk.helpers import bearing_to, distance, lead_target


def test_distance_basic():
    assert distance((0.0, 0.0), (3.0, 4.0)) == pytest.approx(5.0)


def test_bearing_north():
    # Target is straight up (-y) -> compass bearing 0°.
    assert bearing_to((100.0, 100.0), (100.0, 50.0)) == pytest.approx(0.0)


def test_bearing_east():
    # Target straight right (+x) -> 90°.
    assert bearing_to((100.0, 100.0), (200.0, 100.0)) == pytest.approx(90.0)


def test_bearing_south():
    assert bearing_to((100.0, 100.0), (100.0, 200.0)) == pytest.approx(180.0)


def test_bearing_west():
    assert bearing_to((100.0, 100.0), (50.0, 100.0)) == pytest.approx(270.0)


def test_bearing_returns_in_range():
    for x, y in [(150, 80), (50, 200), (40, 40), (200, 200)]:
        b = bearing_to((100.0, 100.0), (float(x), float(y)))
        assert 0.0 <= b < 360.0


def test_lead_target_stationary():
    # Stationary target -> intercept point equals the target.
    pred = lead_target((0.0, 0.0), (100.0, 0.0), (0.0, 0.0), shell_speed=50.0)
    assert pred is not None
    assert pred[0] == pytest.approx(100.0)
    assert pred[1] == pytest.approx(0.0)


def test_lead_target_crossing():
    # Shooter at origin, target at (100,0) moving in +y at 10. Shell speed 50.
    # Solve: |(100, 10t) - (0,0)| = 50t  ->  100^2 + 100t^2 = 2500t^2  ->  t^2 = 100^2/2400
    pred = lead_target((0.0, 0.0), (100.0, 0.0), (0.0, 10.0), shell_speed=50.0)
    assert pred is not None
    expected_t = math.sqrt(10000.0 / 2400.0)
    assert pred[0] == pytest.approx(100.0)
    assert pred[1] == pytest.approx(10.0 * expected_t)


def test_lead_target_unreachable():
    # Target faster than shell, running directly away -> no solution.
    pred = lead_target((0.0, 0.0), (10.0, 0.0), (1000.0, 0.0), shell_speed=50.0)
    assert pred is None
