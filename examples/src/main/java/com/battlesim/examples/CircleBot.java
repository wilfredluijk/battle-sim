package com.battlesim.examples;

import com.battlesim.naval.Bot;
import com.battlesim.naval.BotRunner;
import com.battlesim.naval.Command;
import com.battlesim.naval.protocol.FireCommand;
import com.battlesim.naval.protocol.SensorMode;
import com.battlesim.naval.protocol.WorldView;
import java.util.Random;

/**
 * Simplest possible bot: throttle on, rudder over, fire at a random bearing
 * whenever the gun is cool. No sensing, no aiming, no learning.
 *
 * <p>Useful as a sparring partner and as the smallest readable example of
 * the SDK shape. Compare against {@link TrackingBot} and {@link HunterBot}
 * for progressively more sophisticated strategies.
 */
public final class CircleBot extends Bot {
    private final Random rng;
    private long nextFireTick = 0L;

    public CircleBot(long seed) {
        this.rng = new Random(seed);
    }

    public CircleBot() {
        this(System.nanoTime());
    }

    @Override
    public Command onTick(WorldView view) {
        Command cmd = new Command()
                .throttle(0.6f)
                .rudder(0.4f)
                .sensorMode(SensorMode.ACTIVE);

        if (view.self().ammo() > 0 && view.tick() >= nextFireTick) {
            float bearing = rng.nextFloat() * 360f;
            float range = 150f + rng.nextFloat() * 150f;
            cmd.fire(new FireCommand(bearing, range));
            int cooldown = welcome() == null ? 15 : welcome().shipSpecs().gunCooldownTicks();
            nextFireTick = view.tick() + cooldown;
        }
        return cmd;
    }

    public static void main(String[] args) {
        String host = args.length > 0 ? args[0] : "localhost";
        int port    = args.length > 1 ? Integer.parseInt(args[1]) : 7878;
        String name = args.length > 2 ? args[2] : "circle";
        BotRunner.run(new CircleBot(42L), host, port, name);
    }
}
