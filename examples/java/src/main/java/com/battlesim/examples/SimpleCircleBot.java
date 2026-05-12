package com.battlesim.examples;

import com.battlesim.naval.Bot;
import com.battlesim.naval.BotRunner;
import com.battlesim.naval.Command;
import com.battlesim.naval.protocol.FireCommand;
import com.battlesim.naval.protocol.SensorMode;
import com.battlesim.naval.protocol.Welcome;
import com.battlesim.naval.protocol.WorldView;
import java.util.Random;

/**
 * A deliberately small bot: make a lazy circle, keep radar on, and throw shells
 * in random directions every few seconds.
 */
public final class SimpleCircleBot extends Bot {
    private final Random random = new Random(42);
    private long nextFireTick = 0;
    private float maxRange = 300.0f;

    @Override
    public void onWelcome(Welcome welcome) {
        maxRange = welcome.shipSpecs().maxShellRange();
    }

    @Override
    public Command onTick(WorldView view) {
        Command command = new Command()
                .throttle(0.65f)
                .rudder(0.42f)
                .sensorMode(SensorMode.ACTIVE);

        if (view.self().ammo() > 0 && view.tick() >= nextFireTick) {
            float bearing = random.nextFloat() * 360.0f;
            float range = 90.0f + random.nextFloat() * Math.max(1.0f, maxRange - 90.0f);
            command.fire(new FireCommand(bearing, range));
            nextFireTick = view.tick() + 25 + random.nextInt(25);
        }

        return command;
    }

    @Override
    public void onError(String code, String message) {
        if ("cooldown_active".equals(code)) {
            nextFireTick = lastTick() + 10;
        } else {
            super.onError(code, message);
        }
    }

    public static void main(String[] args) {
        BotArgs parsed = BotArgs.parse(args, "simple-circle");
        BotRunner.run(new SimpleCircleBot(), parsed.host(), parsed.port(), parsed.name());
    }
}
