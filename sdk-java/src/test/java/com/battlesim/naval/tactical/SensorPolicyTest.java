package com.battlesim.naval.tactical;

import static com.battlesim.naval.tactical.TacticalTestSupport.*;
import static org.junit.jupiter.api.Assertions.*;

import com.battlesim.naval.protocol.SelfState;
import com.battlesim.naval.protocol.SensorMode;
import com.battlesim.naval.protocol.Vec2;

import java.util.List;
import org.junit.jupiter.api.Test;

class SensorPolicyTest {

    @Test
    void alwaysActive() {
        SensorPolicy p = new SensorPolicy.AlwaysActive();
        Tracker tr = new Tracker(SPECS, 10);
        assertEquals(SensorMode.ACTIVE, p.choose(view(0, me()), tr));
        assertEquals(SensorMode.ACTIVE, p.choose(view(99, me()), tr));
    }

    @Test
    void alwaysPassive() {
        SensorPolicy p = new SensorPolicy.AlwaysPassive();
        Tracker tr = new Tracker(SPECS, 10);
        assertEquals(SensorMode.PASSIVE, p.choose(view(0, me()), tr));
    }

    @Test
    void dutyCycleSequence() {
        SensorPolicy p = new SensorPolicy.DutyCycle(3, 2);
        Tracker tr = new Tracker(SPECS, 10);
        SensorMode[] expected = {
                SensorMode.ACTIVE, SensorMode.ACTIVE, SensorMode.ACTIVE,
                SensorMode.PASSIVE, SensorMode.PASSIVE,
                SensorMode.ACTIVE, SensorMode.ACTIVE, SensorMode.ACTIVE,
                SensorMode.PASSIVE, SensorMode.PASSIVE,
        };
        for (int t = 0; t < 10; t++) {
            assertEquals(expected[t], p.choose(view(t, me()), tr), "tick=" + t);
        }
    }

    @Test
    void pingWhenStaleActiveWithNoTracks() {
        SensorPolicy p = new SensorPolicy.PingWhenStale(10);
        Tracker tr = new Tracker(SPECS, 10);
        assertEquals(SensorMode.ACTIVE, p.choose(view(0, me()), tr));
    }

    @Test
    void pingWhenStalePassiveWhenFresh() {
        SensorPolicy p = new SensorPolicy.PingWhenStale(10);
        Tracker tr = new Tracker(SPECS, 10);
        SelfState me = me();
        tr.update(view(0, me, List.of(activeContact(new Vec2(100, 0), me.pos()))));
        assertEquals(SensorMode.PASSIVE, p.choose(view(5, me), tr));
    }

    @Test
    void pingWhenStaleActiveWhenOld() {
        SensorPolicy p = new SensorPolicy.PingWhenStale(10);
        Tracker.Config cfg = new Tracker.Config();
        cfg.stalenessTicks = 100;
        Tracker tr = new Tracker(SPECS, 10, cfg);
        SelfState me = me();
        tr.update(view(0, me, List.of(activeContact(new Vec2(100, 0), me.pos()))));
        for (int t = 1; t <= 15; t++) tr.update(view(t, me));
        assertEquals(SensorMode.ACTIVE, p.choose(view(15, me), tr));
    }
}
