package com.battlesim.naval.tactical;

import static com.battlesim.naval.tactical.TacticalTestSupport.*;
import static org.junit.jupiter.api.Assertions.*;

import com.battlesim.naval.Command;
import com.battlesim.naval.protocol.TickEvent;
import com.battlesim.naval.protocol.WorldView;

import java.util.List;
import java.util.Optional;
import org.junit.jupiter.api.Test;

class EvaderTest {

    private static WorldView hitView(long tick) {
        return viewWithEvents(tick, me(), List.of(new TickEvent.Hit(10)));
    }

    @Test
    void idleReturnsEmpty() {
        Evader e = new Evader();
        assertTrue(e.update(view(0, me())).isEmpty());
        assertEquals(Evader.State.IDLE, e.state());
    }

    @Test
    void triggersOnHitThenCoolsDown() {
        Evader e = new Evader(5, 3);
        assertTrue(e.update(hitView(0)).isPresent());
        assertEquals(Evader.State.EVADING, e.state());

        for (int t = 1; t < 5; t++) {
            assertTrue(e.update(view(t, me())).isPresent(), "tick=" + t);
        }

        assertTrue(e.update(view(5, me())).isEmpty());
        assertEquals(Evader.State.COOLDOWN, e.state());

        assertTrue(e.update(view(8, me())).isEmpty());
        assertEquals(Evader.State.IDLE, e.state());
    }

    @Test
    void flipsRudderWhenHitInCooldown() {
        Evader e = new Evader(3, 5);
        e.update(hitView(0));
        Optional<Command> first = e.update(view(1, me()));
        float initial = first.orElseThrow().rudderValue();

        e.update(view(3, me()));
        assertEquals(Evader.State.COOLDOWN, e.state());

        Optional<Command> reHit = e.update(hitView(4));
        assertTrue(reHit.isPresent());
        assertEquals(-initial, reHit.get().rudderValue(), 1e-6f);
    }

    @Test
    void overrideHasFullThrottle() {
        Evader e = new Evader();
        Optional<Command> cmd = e.update(hitView(0));
        assertTrue(cmd.isPresent());
        assertEquals(1.0f, cmd.get().throttleValue(), 1e-6f);
        assertEquals(1.0f, Math.abs(cmd.get().rudderValue()), 1e-6f);
    }
}
