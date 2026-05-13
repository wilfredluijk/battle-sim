package com.battlesim.naval.tactical;

import static com.battlesim.naval.tactical.TacticalTestSupport.*;
import static org.junit.jupiter.api.Assertions.*;

import com.battlesim.naval.Command;
import com.battlesim.naval.protocol.SelfState;
import com.battlesim.naval.protocol.SensorMode;
import com.battlesim.naval.protocol.TickEvent;
import com.battlesim.naval.protocol.Vec2;
import com.battlesim.naval.protocol.WorldView;

import java.util.List;
import org.junit.jupiter.api.Test;

/**
 * End-to-end-style: drive {@link TacticalBot} through a synthetic match.
 * No server, no network — just construct frames and assert reasonable output.
 */
class TacticalBotIntegrationTest {

    /** Engages the nearest threat or holds station. */
    static final class EngageNearest extends TacticalBot {
        @Override
        public Intent decide(TacticalContext ctx) {
            return ctx.threats().nearest()
                    .<Intent>map(Intent::engage)
                    .orElseGet(Intent::hold);
        }
    }

    @Test
    void holdsWithNoContacts() {
        EngageNearest bot = new EngageNearest();
        bot.onWelcome(WELCOME);
        Command cmd = bot.onTick(view(0, me(new Vec2(400, 400), 0.0f, 100, 20)));
        assertEquals(0.0f, cmd.throttleValue(), 1e-6f);
        assertEquals(0.0f, cmd.rudderValue(), 1e-6f);
        assertTrue(cmd.fireValue().isEmpty());
    }

    @Test
    void steersTowardEngagedTarget() {
        EngageNearest bot = new EngageNearest();
        bot.onWelcome(WELCOME);
        Vec2 mePos = new Vec2(400, 400);
        Vec2 enemy = new Vec2(600, 400); // east
        SelfState me = me(mePos, 0.0f, 100, 20); // heading north
        Command cmd = bot.onTick(view(0, me, List.of(activeContact(enemy, mePos))));
        // Heading 0 (north), target east -> right turn.
        assertTrue(cmd.rudderValue() > 0.0f);
        assertTrue(cmd.throttleValue() > 0.0f);
    }

    @Test
    void firesWhenTargetInRangeAndLinedUp() {
        EngageNearest bot = new EngageNearest();
        bot.onWelcome(WELCOME);
        Vec2 mePos = new Vec2(400, 400);
        Vec2 enemy = new Vec2(500, 400);
        SelfState me = me(mePos, 90.0f, 100, 20);
        Command cmd = bot.onTick(view(0, me, List.of(activeContact(enemy, mePos))));
        assertTrue(cmd.fireValue().isPresent());

        Command cmd2 = bot.onTick(view(1, me, List.of(activeContact(enemy, mePos))));
        assertTrue(cmd2.fireValue().isEmpty(), "cooldown should block");
    }

    @Test
    void evasionPreemptsIntent() {
        EngageNearest bot = new EngageNearest();
        bot.onWelcome(WELCOME);
        Vec2 mePos = new Vec2(400, 400);
        Vec2 enemy = new Vec2(500, 400);
        SelfState me = me(mePos, 90.0f, 100, 20);
        WorldView v = new WorldView(0L, 80L, me, List.of(activeContact(enemy, mePos)),
                List.of(new TickEvent.Hit(10)));
        Command cmd = bot.onTick(v);
        assertEquals(1.0f, Math.abs(cmd.rudderValue()), 1e-6f);
        assertEquals(1.0f, cmd.throttleValue(), 1e-6f);
        assertTrue(cmd.fireValue().isEmpty());
    }

    @Test
    void customIntentPassesThrough() {
        class Custom extends TacticalBot {
            @Override
            public Intent decide(TacticalContext ctx) {
                return Intent.custom(new Command().throttle(-0.5f).rudder(0.25f).sensorMode(SensorMode.PASSIVE));
            }
        }
        Custom bot = new Custom();
        bot.onWelcome(WELCOME);
        Command cmd = bot.onTick(view(0, me(new Vec2(400, 400), 0.0f, 100, 20)));
        assertEquals(-0.5f, cmd.throttleValue(), 1e-6f);
        assertEquals(0.25f, cmd.rudderValue(), 1e-6f);
        assertEquals(SensorMode.PASSIVE, cmd.sensorModeValue());
    }

    @Test
    void fullMatchLoopDoesNotExplode() {
        EngageNearest bot = new EngageNearest();
        bot.onWelcome(WELCOME);

        Vec2 mePos = new Vec2(400, 400);
        float enemyX = 600.0f;
        for (int tick = 0; tick < 50; tick++) {
            enemyX -= 1.0f;
            Vec2 enemy = new Vec2(enemyX, 400);
            SelfState me = me(mePos, 90.0f, 100, Math.max(0, 20 - tick / 15));
            Command cmd = bot.onTick(view(tick, me, List.of(activeContact(enemy, mePos))));
            assertTrue(cmd.throttleValue() >= -1.0f && cmd.throttleValue() <= 1.0f);
            assertTrue(cmd.rudderValue() >= -1.0f && cmd.rudderValue() <= 1.0f);
            assertNotNull(cmd.sensorModeValue());
        }
    }
}
