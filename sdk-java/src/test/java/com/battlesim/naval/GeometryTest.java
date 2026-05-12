package com.battlesim.naval;

import static org.junit.jupiter.api.Assertions.*;

import com.battlesim.naval.protocol.Vec2;
import java.util.Optional;
import org.junit.jupiter.api.Test;

class GeometryTest {

    private static final float EPS = 1e-3f;

    @Test
    void distanceBasic() {
        assertEquals(5.0f, Geometry.distance(new Vec2(0, 0), new Vec2(3, 4)), EPS);
    }

    @Test
    void bearingNorth() {
        // Target straight up (-y) -> 0°.
        assertEquals(0.0f, Geometry.bearingTo(new Vec2(100, 100), new Vec2(100, 50)), EPS);
    }

    @Test
    void bearingEast() {
        assertEquals(90.0f, Geometry.bearingTo(new Vec2(100, 100), new Vec2(200, 100)), EPS);
    }

    @Test
    void bearingSouth() {
        assertEquals(180.0f, Geometry.bearingTo(new Vec2(100, 100), new Vec2(100, 200)), EPS);
    }

    @Test
    void bearingWest() {
        assertEquals(270.0f, Geometry.bearingTo(new Vec2(100, 100), new Vec2(50, 100)), EPS);
    }

    @Test
    void bearingInRange() {
        for (float[] xy : new float[][] {{150, 80}, {50, 200}, {40, 40}, {200, 200}}) {
            float b = Geometry.bearingTo(new Vec2(100, 100), new Vec2(xy[0], xy[1]));
            assertTrue(b >= 0.0f && b < 360.0f, "bearing out of range: " + b);
        }
    }

    @Test
    void leadStationary() {
        Optional<Vec2> p =
                Geometry.leadTarget(new Vec2(0, 0), new Vec2(100, 0), new Vec2(0, 0), 50.0f);
        assertTrue(p.isPresent());
        assertEquals(100.0f, p.get().x(), EPS);
        assertEquals(0.0f, p.get().y(), EPS);
    }

    @Test
    void leadCrossing() {
        Optional<Vec2> p =
                Geometry.leadTarget(new Vec2(0, 0), new Vec2(100, 0), new Vec2(0, 10), 50.0f);
        assertTrue(p.isPresent());
        // Closed form: t = sqrt(10000 / 2400)
        double expectedT = Math.sqrt(10000.0 / 2400.0);
        assertEquals(100.0f, p.get().x(), 1e-2);
        assertEquals((float) (10.0 * expectedT), p.get().y(), 1e-2);
    }

    @Test
    void leadUnreachable() {
        Optional<Vec2> p =
                Geometry.leadTarget(new Vec2(0, 0), new Vec2(10, 0), new Vec2(1000, 0), 50.0f);
        assertTrue(p.isEmpty());
    }
}
