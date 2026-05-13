package com.battlesim.examples;

import com.battlesim.naval.Bot;
import com.battlesim.naval.BotRunner;
import com.battlesim.naval.Command;
import com.battlesim.naval.protocol.ContactKind;
import com.battlesim.naval.protocol.Welcome;
import com.battlesim.naval.protocol.WorldView;
import com.battlesim.naval.tactical.Gunner;
import com.battlesim.naval.tactical.SensorPolicy;
import com.battlesim.naval.tactical.Track;
import com.battlesim.naval.tactical.Tracker;

import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;

/**
 * Tracking bot rewritten with the Layer-2 tactical toolkit.
 *
 * <p>Hull orbits at constant rudder/throttle; the SDK's {@link Tracker} stitches
 * contacts across ticks, {@link SensorPolicy.DutyCycle} schedules pings, and
 * {@link Gunner} handles cooldown, lead, range, and self-splash gating.
 */
public final class TrackingCircleBot extends Bot {
    private Tracker tracker;
    private Gunner gunner;
    private final SensorPolicy sensorPolicy = new SensorPolicy.DutyCycle(4, 8);

    @Override
    public void onWelcome(Welcome welcome) {
        this.tracker = new Tracker(welcome.shipSpecs(), welcome.tickHz());
        this.gunner = new Gunner(welcome.shipSpecs());
    }

    @Override
    public Command onTick(WorldView view) {
        List<Track> tracks = tracker.update(view);
        List<Track> ships = new ArrayList<>();
        for (Track t : tracks) {
            if (t.kind() == ContactKind.SHIP) ships.add(t);
        }

        Command cmd = new Command()
                .throttle(0.72f)
                .rudder(0.36f)
                .sensorMode(sensorPolicy.choose(view, tracker));

        if (!ships.isEmpty()) {
            Track target = ships.stream()
                    .min(Comparator.comparingDouble(
                            t -> {
                                float dx = t.pos().x() - view.self().pos().x();
                                float dy = t.pos().y() - view.self().pos().y();
                                return dx * dx + dy * dy;
                            }))
                    .orElseThrow();
            gunner.attempt(cmd, view.self(), target, view);
        }

        return cmd;
    }

    public static void main(String[] args) {
        BotArgs parsed = BotArgs.parse(args, "tracking-circle");
        BotRunner.run(new TrackingCircleBot(), parsed.host(), parsed.port(), parsed.name());
    }
}
