package com.battlesim.naval.tactical;

import com.battlesim.naval.Geometry;
import com.battlesim.naval.protocol.SelfState;
import com.battlesim.naval.protocol.ShipSpecs;
import com.battlesim.naval.protocol.Vec2;

/**
 * Steering helper.
 *
 * <p>Translates a desired bearing or waypoint into a {@code (throttle, rudder)}
 * pair that respects the speed-coupled turn rate of the ship. Sharp turns get
 * reduced throttle so the rudder bites rather than the hull plows straight.
 */
public class Helm {

    public static final class Config {
        public float mapWidth = 800.0f;
        public float mapHeight = 800.0f;
        public float wallMargin = 30.0f;
        public float turnAggressionDeg = 30.0f;
        public float alignThresholdDeg = 10.0f;
        public float minTurnThrottle = 0.55f;
    }

    /** Result of a steering decision. */
    public record Steering(float throttle, float rudder) {}

    private final ShipSpecs specs;
    private final Config cfg;

    public Helm(ShipSpecs specs) {
        this(specs, new Config());
    }

    public Helm(ShipSpecs specs, Config config) {
        this.specs = specs;
        this.cfg = config;
    }

    public Steering steerToBearing(SelfState me, float targetBearingDeg) {
        return steerToBearing(me, targetBearingDeg, true, 1.0f);
    }

    public Steering steerToBearing(
            SelfState me, float targetBearingDeg, boolean respectWalls, float desiredThrottle) {
        float bearing = respectWalls ? wallOverride(me, targetBearingDeg) : targetBearingDeg;
        float delta = Geometry.signedBearingDelta(bearing, me.headingDeg());
        float rudder = Geometry.clamp(delta / cfg.turnAggressionDeg, -1.0f, 1.0f);

        float absDelta = Math.abs(delta);
        float throttle;
        if (absDelta <= cfg.alignThresholdDeg) {
            throttle = desiredThrottle;
        } else {
            float denom = Math.max(180.0f - cfg.alignThresholdDeg, 1e-6f);
            float scale = Geometry.clamp((180.0f - absDelta) / denom, 0.0f, 1.0f);
            throttle = cfg.minTurnThrottle + (desiredThrottle - cfg.minTurnThrottle) * scale;
        }
        return new Steering(throttle, rudder);
    }

    public Steering steerToPoint(SelfState me, Vec2 target) {
        return steerToPoint(me, target, true, 1.0f);
    }

    public Steering steerToPoint(SelfState me, Vec2 target, boolean respectWalls, float desiredThrottle) {
        return steerToBearing(me, Geometry.bearingTo(me.pos(), target), respectWalls, desiredThrottle);
    }

    /**
     * If we're inside the wall margin and the target points further into the
     * wall, redirect toward an inward bearing instead.
     */
    float wallOverride(SelfState me, float targetBearing) {
        float x = me.pos().x();
        float y = me.pos().y();
        float pushX = 0.0f;
        float pushY = 0.0f;
        if (x < cfg.wallMargin) pushX = 1.0f;
        else if (x > cfg.mapWidth - cfg.wallMargin) pushX = -1.0f;
        if (y < cfg.wallMargin) pushY = 1.0f;
        else if (y > cfg.mapHeight - cfg.wallMargin) pushY = -1.0f;
        if (pushX == 0.0f && pushY == 0.0f) return targetBearing;

        float pushBearing = Geometry.bearingTo(new Vec2(0.0f, 0.0f), new Vec2(pushX, pushY));
        float delta = Math.abs(Geometry.signedBearingDelta(targetBearing, pushBearing));
        if (delta <= 90.0f) return targetBearing;
        return pushBearing;
    }
}
