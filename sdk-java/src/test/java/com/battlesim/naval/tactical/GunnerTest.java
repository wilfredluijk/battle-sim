package com.battlesim.naval.tactical;

import static com.battlesim.naval.tactical.TacticalTestSupport.*;
import static org.junit.jupiter.api.Assertions.*;

import com.battlesim.naval.Command;
import com.battlesim.naval.protocol.ContactKind;
import com.battlesim.naval.protocol.SelfState;
import com.battlesim.naval.protocol.Vec2;
import com.battlesim.naval.protocol.WorldView;

import java.util.Optional;
import org.junit.jupiter.api.Test;

class GunnerTest {
    private static final float EPS = 1e-2f;

    private Track track(int id, Vec2 pos, Vec2 vel, long lastActive) {
        return new Track(id, ContactKind.SHIP, pos, pos, vel, lastActive, lastActive,
                lastActive, 1.0f, "active");
    }

    @Test
    void solvesAtRest() {
        Gunner g = new Gunner(SPECS);
        SelfState me = me();
        WorldView v = view(0, me);
        Optional<FireSolution> sol = g.solve(me, track(1, new Vec2(100, 0), new Vec2(0, 0), 0), v);
        assertTrue(sol.isPresent());
        assertEquals(90.0f, sol.get().bearingDeg(), EPS);
        assertEquals(100.0f, sol.get().range(), EPS);
    }

    @Test
    void respectsCooldown() {
        Gunner g = new Gunner(SPECS);
        SelfState me = me();
        assertTrue(g.solve(me, track(1, new Vec2(100, 0), new Vec2(0, 0), 0), view(0, me)).isPresent());
        g.noteFired(0);
        for (int tick = 1; tick < SPECS.gunCooldownTicks(); tick++) {
            assertTrue(g.solve(me, track(1, new Vec2(100, 0), new Vec2(0, 0), tick),
                    view(tick, me)).isEmpty(), "tick=" + tick);
        }
        assertTrue(g.solve(me, track(1, new Vec2(100, 0), new Vec2(0, 0), SPECS.gunCooldownTicks()),
                view(SPECS.gunCooldownTicks(), me)).isPresent());
    }

    @Test
    void refusesWhenOutOfRange() {
        Gunner g = new Gunner(SPECS);
        SelfState me = me();
        Track t = track(1, new Vec2(SPECS.maxShellRange() + 50, 0), new Vec2(0, 0), 0);
        assertTrue(g.solve(me, t, view(0, me)).isEmpty());
    }

    @Test
    void refusesSelfSplash() {
        Gunner g = new Gunner(SPECS);
        SelfState me = me();
        Track t = track(1, new Vec2(10, 0), new Vec2(0, 0), 0);
        assertTrue(g.solve(me, t, view(0, me)).isEmpty());
    }

    @Test
    void refusesStaleActiveTrack() {
        Gunner.Config cfg = new Gunner.Config();
        cfg.maxActiveAgeTicks = 5;
        Gunner g = new Gunner(SPECS, cfg);
        SelfState me = me();
        Track t = track(1, new Vec2(100, 0), new Vec2(0, 0), 0);
        assertTrue(g.solve(me, t, view(10, me)).isEmpty());
    }

    @Test
    void refusesWhenOutOfAmmo() {
        Gunner g = new Gunner(SPECS);
        SelfState me = me(new Vec2(0, 0), 0.0f, 100, 0);
        Track t = track(1, new Vec2(100, 0), new Vec2(0, 0), 0);
        assertTrue(g.solve(me, t, view(0, me)).isEmpty());
    }

    @Test
    void attemptAttachesFireAndStartsCooldown() {
        Gunner g = new Gunner(SPECS);
        SelfState me = me();
        Command cmd = new Command();
        boolean fired = g.attempt(cmd, me, track(1, new Vec2(100, 0), new Vec2(0, 0), 0), view(0, me));
        assertTrue(fired);
        assertTrue(cmd.fireValue().isPresent());
        assertEquals(SPECS.gunCooldownTicks(), g.nextFireTick());

        Command cmd2 = new Command();
        boolean firedAgain = g.attempt(cmd2, me, track(1, new Vec2(100, 0), new Vec2(0, 0), 1), view(1, me));
        assertFalse(firedAgain);
        assertTrue(cmd2.fireValue().isEmpty());
    }

    @Test
    void leadsMovingTarget() {
        Gunner g = new Gunner(SPECS);
        SelfState me = me();
        Track t = track(1, new Vec2(100, 0), new Vec2(0, 10), 0);
        Optional<FireSolution> sol = g.solve(me, t, view(0, me));
        assertTrue(sol.isPresent());
        // Aim y > 0 (lead in +y) and bearing somewhere between east and south.
        assertTrue(sol.get().aimPos().y() > 0.0f);
        assertTrue(sol.get().bearingDeg() > 90.0f && sol.get().bearingDeg() < 180.0f);
    }
}
