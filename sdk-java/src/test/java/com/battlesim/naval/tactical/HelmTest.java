package com.battlesim.naval.tactical;

import static com.battlesim.naval.tactical.TacticalTestSupport.*;
import static org.junit.jupiter.api.Assertions.*;

import com.battlesim.naval.protocol.SelfState;
import com.battlesim.naval.protocol.Vec2;
import org.junit.jupiter.api.Test;

class HelmTest {
    private static final float EPS = 1e-3f;

    @Test
    void alignedRunsFullThrottleZeroRudder() {
        Helm helm = new Helm(SPECS);
        SelfState me = me(new Vec2(400, 400), 90.0f, 100, 20);
        Helm.Steering s = helm.steerToBearing(me, 90.0f);
        assertEquals(0.0f, s.rudder(), EPS);
        assertEquals(1.0f, s.throttle(), EPS);
    }

    @Test
    void rudderSignMatchesRequiredTurnDirection() {
        Helm helm = new Helm(SPECS);
        SelfState me = me(new Vec2(400, 400), 0.0f, 100, 20);
        assertTrue(helm.steerToBearing(me, 90.0f).rudder() > 0.0f);
        assertTrue(helm.steerToBearing(me, 270.0f).rudder() < 0.0f);
    }

    @Test
    void tapersThrottleForSharpTurns() {
        Helm helm = new Helm(SPECS);
        SelfState me = me(new Vec2(400, 400), 0.0f, 100, 20);
        float aligned = helm.steerToBearing(me, 0.0f).throttle();
        float sharp = helm.steerToBearing(me, 180.0f).throttle();
        assertTrue(sharp < aligned);
        assertTrue(sharp >= 0.5f);
    }

    @Test
    void wallOverridePushesInward() {
        Helm.Config cfg = new Helm.Config();
        cfg.mapWidth = 800.0f;
        cfg.mapHeight = 800.0f;
        cfg.wallMargin = 30.0f;
        Helm helm = new Helm(SPECS, cfg);
        SelfState me = me(new Vec2(400, 10), 0.0f, 100, 20);
        // Target north (0°) drives into the wall; helm should redirect south-ish.
        float redirected = helm.wallOverride(me, 0.0f);
        assertTrue(redirected > 90.0f && redirected < 270.0f, "got " + redirected);
    }

    @Test
    void wallOverrideLeavesSafeBearingsAlone() {
        Helm.Config cfg = new Helm.Config();
        cfg.mapWidth = 800.0f;
        cfg.mapHeight = 800.0f;
        cfg.wallMargin = 30.0f;
        Helm helm = new Helm(SPECS, cfg);
        SelfState me = me(new Vec2(400, 10), 0.0f, 100, 20);
        // Target south (180°) is already away from the north wall.
        assertEquals(180.0f, helm.wallOverride(me, 180.0f), EPS);
    }
}
