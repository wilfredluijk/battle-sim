package com.battlesim.naval;

import com.battlesim.naval.protocol.Vec2;
import java.util.Optional;

/**
 * Math helpers for bot authors.
 *
 * <p>Bearing convention matches the server: 0° = north (-y), 90° = east (+x),
 * increasing clockwise. Angles are in degrees.
 */
public final class Geometry {
    private Geometry() {}

    public static float distance(Vec2 a, Vec2 b) {
        return a.distanceTo(b);
    }

    /** Compass bearing in degrees from {@code from} to {@code to}, in {@code [0, 360)}. */
    public static float bearingTo(Vec2 from, Vec2 to) {
        float dx = to.x() - from.x();
        float dy = to.y() - from.y();
        double rad = Math.atan2(dx, -dy);
        double deg = Math.toDegrees(rad);
        if (deg < 0.0) deg += 360.0;
        return (float) deg;
    }

    /**
     * Predict the intercept point for a shell fired now at a moving target.
     *
     * @return predicted target position at impact, or empty if no real
     *     intercept solution exists (e.g. target outruns the shell).
     */
    public static Optional<Vec2> leadTarget(
            Vec2 shooterPos, Vec2 targetPos, Vec2 targetVel, float shellSpeed) {
        if (shellSpeed <= 0.0f) return Optional.empty();

        double rx = targetPos.x() - shooterPos.x();
        double ry = targetPos.y() - shooterPos.y();
        double vx = targetVel.x();
        double vy = targetVel.y();

        double a = vx * vx + vy * vy - (double) shellSpeed * shellSpeed;
        double b = 2.0 * (rx * vx + ry * vy);
        double c = rx * rx + ry * ry;

        Double t = null;
        if (Math.abs(a) < 1e-9) {
            if (Math.abs(b) < 1e-9) {
                t = c < 1e-9 ? 0.0 : null;
            } else {
                double cand = -c / b;
                t = cand >= 0.0 ? cand : null;
            }
        } else {
            double disc = b * b - 4.0 * a * c;
            if (disc < 0.0) return Optional.empty();
            double sqrtDisc = Math.sqrt(disc);
            double t1 = (-b - sqrtDisc) / (2.0 * a);
            double t2 = (-b + sqrtDisc) / (2.0 * a);
            double best = Double.POSITIVE_INFINITY;
            if (t1 >= 0.0 && t1 < best) best = t1;
            if (t2 >= 0.0 && t2 < best) best = t2;
            if (Double.isInfinite(best)) return Optional.empty();
            t = best;
        }

        if (t == null) return Optional.empty();
        return Optional.of(new Vec2(
                (float) (targetPos.x() + vx * t),
                (float) (targetPos.y() + vy * t)));
    }
}
