package com.battlesim.examples;

import com.battlesim.naval.Bot;
import com.battlesim.naval.BotRunner;
import com.battlesim.naval.Command;
import com.battlesim.naval.Geometry;
import com.battlesim.naval.protocol.ContactKind;
import com.battlesim.naval.protocol.SensorMode;
import com.battlesim.naval.protocol.Vec2;
import com.battlesim.naval.protocol.Welcome;
import com.battlesim.naval.protocol.WorldView;
import com.battlesim.naval.tactical.Evader;
import com.battlesim.naval.tactical.Gunner;
import com.battlesim.naval.tactical.Helm;
import com.battlesim.naval.tactical.SensorPolicy;
import com.battlesim.naval.tactical.Track;
import com.battlesim.naval.tactical.Tracker;

import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.Optional;

/**
 * Stealth-first tactician using the Layer-2 toolkit for mechanics and
 * keeping the *tactics* (range-band orbit, HP-aware break-off, sensor
 * scheduling) as bespoke code. Mirrors the Python {@code tactician_bot.py}.
 */
public final class StrongTacticalBot extends Bot {
    private static final float PREFERRED_RANGE = 180.0f;
    private static final float RANGE_BAND_HALF_WIDTH = 40.0f;
    private static final int LOW_HP_THRESHOLD = 30;

    private Tracker tracker;
    private Gunner gunner;
    private Helm helm;
    private final Evader evader = new Evader(14, 8);
    private final SensorPolicy sensorPolicy = new SensorPolicy.PingWhenStale(15);

    @Override
    public void onWelcome(Welcome welcome) {
        this.tracker = new Tracker(welcome.shipSpecs(), welcome.tickHz());
        Gunner.Config gunCfg = new Gunner.Config();
        gunCfg.selfSplashMargin = 2.0f;
        this.gunner = new Gunner(welcome.shipSpecs(), gunCfg);
        Helm.Config helmCfg = new Helm.Config();
        helmCfg.mapWidth = welcome.map().width();
        helmCfg.mapHeight = welcome.map().height();
        helmCfg.wallMargin = 90.0f;
        this.helm = new Helm(welcome.shipSpecs(), helmCfg);
    }

    @Override
    public Command onTick(WorldView view) {
        List<Track> tracks = tracker.update(view);
        List<Track> ships = new ArrayList<>();
        for (Track t : tracks) {
            if (t.kind() == ContactKind.SHIP) ships.add(t);
        }
        SensorMode sensor = sensorPolicy.choose(view, tracker);

        // 1. Evasion preempts everything.
        Optional<Command> evade = evader.update(view);
        if (evade.isPresent()) {
            return evade.get().sensorMode(sensor);
        }

        // 2. No threats: drift toward map centre while sweeping.
        if (ships.isEmpty()) {
            Vec2 centre = new Vec2(
                    welcome().map().width() / 2.0f,
                    welcome().map().height() / 2.0f);
            Helm.Steering s = helm.steerToPoint(view.self(), centre);
            return new Command().throttle(s.throttle()).rudder(s.rudder()).sensorMode(sensor);
        }

        // 3. Pick the highest-priority threat.
        Track target = bestTarget(view, ships);

        // 4. Plan heading: range-band orbit, or break off if hurt.
        float bearing = engagementBearing(view, target);
        Helm.Steering s = helm.steerToBearing(view.self(), bearing);

        Command cmd = new Command().throttle(s.throttle()).rudder(s.rudder()).sensorMode(sensor);

        // 5. Take the shot if Gunner approves.
        gunner.attempt(cmd, view.self(), target, view);
        return cmd;
    }

    private Track bestTarget(WorldView view, List<Track> ships) {
        return ships.stream().min(Comparator.comparingDouble(t -> {
            float rng = Geometry.distance(view.self().pos(), t.pos());
            double rangePen = Math.abs(rng - PREFERRED_RANGE);
            double stalePen = Math.max(0, view.tick() - t.lastSeenTick()) * 1.5;
            double drPen = "active".equals(t.source()) ? 0.0 : 35.0;
            return rangePen + stalePen + drPen;
        })).orElseThrow();
    }

    private float engagementBearing(WorldView view, Track target) {
        Vec2 me = view.self().pos();
        float rng = Geometry.distance(me, target.pos());
        float toTarget = Geometry.bearingTo(me, target.pos());

        if (view.self().hp() <= LOW_HP_THRESHOLD) {
            return Geometry.wrapBearing(toTarget + 150.0f);
        }
        if (rng > PREFERRED_RANGE + RANGE_BAND_HALF_WIDTH) return toTarget;
        if (rng < PREFERRED_RANGE - RANGE_BAND_HALF_WIDTH) {
            return Geometry.wrapBearing(toTarget + 180.0f);
        }
        return Geometry.wrapBearing(toTarget + 90.0f);
    }

    public static void main(String[] args) {
        BotArgs parsed = BotArgs.parse(args, "strong-tactical");
        BotRunner.run(new StrongTacticalBot(), parsed.host(), parsed.port(), parsed.name());
    }
}
