package com.battlesim.naval.tactical;

import com.battlesim.naval.Bot;
import com.battlesim.naval.Command;
import com.battlesim.naval.Geometry;
import com.battlesim.naval.protocol.ContactKind;
import com.battlesim.naval.protocol.SensorMode;
import com.battlesim.naval.protocol.Vec2;
import com.battlesim.naval.protocol.Welcome;
import com.battlesim.naval.protocol.WorldView;

import java.util.ArrayList;
import java.util.List;
import java.util.Optional;

/**
 * Higher-level {@link Bot}. Subclass and override {@link #decide}.
 *
 * <p>The framework wires {@link Tracker}, {@link Gunner}, {@link Helm},
 * {@link SensorPolicy}, and {@link Evader} together. Bot authors return an
 * {@link Intent}; the framework translates it into a wire command in this
 * preemption order: {@code Evader > Custom > Intent > SensorPolicy > Gunner}.
 */
public abstract class TacticalBot extends Bot {

    protected Tracker tracker;
    protected Gunner gunner;
    protected Helm helm;
    protected Evader evader = new Evader();
    protected SensorPolicy sensorPolicy = new SensorPolicy.AlwaysActive();

    /**
     * Cached welcome. The runtime also sets {@link Bot#welcome()} before
     * firing {@code onWelcome}, but tests construct a TacticalBot directly
     * and never go through the runtime — so we keep our own copy.
     */
    private Welcome tacticalWelcome;

    private int patrolCorner = 0;

    /** Override me. Return an {@link Intent} describing what to do this tick. */
    public abstract Intent decide(TacticalContext ctx);

    /**
     * Override to customize subsystems (e.g. swap in a custom {@link Tracker}).
     * Default subsystems have already been constructed by the time this fires.
     */
    public void onTacticalWelcome(Welcome welcome) {}

    @Override
    public final void onWelcome(Welcome welcome) {
        this.tacticalWelcome = welcome;
        this.tracker = new Tracker(welcome.shipSpecs(), welcome.tickHz());
        this.gunner = new Gunner(welcome.shipSpecs());
        Helm.Config helmCfg = new Helm.Config();
        helmCfg.mapWidth = welcome.map().width();
        helmCfg.mapHeight = welcome.map().height();
        this.helm = new Helm(welcome.shipSpecs(), helmCfg);
        if (this.evader == null) this.evader = new Evader();
        onTacticalWelcome(welcome);
    }

    @Override
    public final Command onTick(WorldView view) {
        if (tracker == null || gunner == null || helm == null || tacticalWelcome == null) {
            return new Command();
        }

        List<Track> tracks = tracker.update(view);
        List<Track> ships = new ArrayList<>();
        for (Track t : tracks) {
            if (t.kind() == ContactKind.SHIP) ships.add(t);
        }
        ThreatList threats = new ThreatList(ships, view.self().pos());
        TacticalContext ctx = new TacticalContext(
                view,
                view.self(),
                tacticalWelcome.shipSpecs(),
                tracker,
                threats,
                tacticalWelcome.map().width(),
                tacticalWelcome.map().height());

        // 1. Evader preempts everything.
        Optional<Command> evade = evader.update(view);
        if (evade.isPresent()) {
            Command e = evade.get();
            e.sensorMode(sensorPolicy.choose(view, tracker));
            return e;
        }

        // 2. Player intent.
        Intent intent = decide(ctx);

        if (intent instanceof Intent.Custom c) {
            return c.command() != null ? c.command() : new Command();
        }

        Command cmd = intentToCommand(intent, ctx);

        // 3. Sensor overlay.
        cmd.sensorMode(sensorPolicy.choose(view, tracker));

        // 4. Gunner overlay.
        Track target = selectFireTarget(intent, threats);
        if (target != null) {
            gunner.attempt(cmd, view.self(), target, view);
        }
        return cmd;
    }

    private Command intentToCommand(Intent intent, TacticalContext ctx) {
        if (intent instanceof Intent.Hold) {
            return new Command().throttle(0.0f).rudder(0.0f);
        }
        if (intent instanceof Intent.Engage e) {
            Helm.Steering s = helm.steerToPoint(ctx.me(), e.target().pos());
            return new Command().throttle(s.throttle()).rudder(s.rudder());
        }
        if (intent instanceof Intent.RetreatTo r) {
            Helm.Steering s = helm.steerToPoint(ctx.me(), r.point());
            return new Command().throttle(s.throttle()).rudder(s.rudder());
        }
        if (intent instanceof Intent.Patrol p) {
            Vec2 waypoint = patrolWaypoint(p, ctx);
            Helm.Steering s = helm.steerToPoint(ctx.me(), waypoint);
            return new Command().throttle(s.throttle()).rudder(s.rudder());
        }
        return new Command();
    }

    private Track selectFireTarget(Intent intent, ThreatList threats) {
        if (intent instanceof Intent.Engage e) return e.target();
        if (intent instanceof Intent.Hold) return null;
        return threats.nearest().orElse(null);
    }

    private Vec2 patrolWaypoint(Intent.Patrol p, TacticalContext ctx) {
        Vec2[] corners = new Vec2[] {
                new Vec2(p.x1(), p.y1()),
                new Vec2(p.x2(), p.y1()),
                new Vec2(p.x2(), p.y2()),
                new Vec2(p.x1(), p.y2()),
        };
        Vec2 target = corners[patrolCorner];
        if (Geometry.distance(ctx.me().pos(), target) < 25.0f) {
            patrolCorner = (patrolCorner + 1) % 4;
            target = corners[patrolCorner];
        }
        return target;
    }
}
