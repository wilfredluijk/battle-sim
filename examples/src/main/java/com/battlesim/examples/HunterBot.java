package com.battlesim.examples;

import com.battlesim.naval.Bot;
import com.battlesim.naval.BotRunner;
import com.battlesim.naval.Command;
import com.battlesim.naval.Geometry;
import com.battlesim.naval.protocol.Contact;
import com.battlesim.naval.protocol.GameStart;
import com.battlesim.naval.protocol.SensorMode;
import com.battlesim.naval.protocol.TickEvent;
import com.battlesim.naval.protocol.Vec2;
import com.battlesim.naval.protocol.Welcome;
import com.battlesim.naval.protocol.WorldView;
import java.util.ArrayList;
import java.util.List;

/**
 * A fully featured combatant. Tracks contacts across ticks, leads moving
 * targets, manages a preferred stand-off range, conserves ammo around the
 * gun cooldown, switches between passive listening and active pinging based
 * on tactical need, evades when taking hits, and steers away from the map
 * edge.
 *
 * <p>The strategy in one paragraph: listen passively to keep a low profile;
 * if something is heard but range is unknown, ping briefly for a fix; once a
 * track is confirmed, close to optimal engagement range, lead the shot, and
 * pop active radar only long enough to fire a clean shell. If hit recently,
 * jink — alternate hard rudder to spoil the enemy's lead solution.
 */
public final class HunterBot extends Bot {

    private static final float ASSOCIATION_GATE = 50f;
    private static final int STALE_AFTER_TICKS = 40;
    private static final float VELOCITY_ALPHA = 0.5f;
    private static final float MAP_MARGIN = 60f;
    /** How long to keep pinging after losing range info on a target. */
    private static final int PING_BURST_TICKS = 4;
    /** Ticks of evasive jinking after taking damage. */
    private static final int EVADE_TICKS = 20;

    private static final class Track {
        Vec2 pos;
        Vec2 vel = new Vec2(0f, 0f);
        long lastSeenTick;
        long lastRangeTick = -1;
        int hits;

        Track(Vec2 pos, long tick) {
            this.pos = pos;
            this.lastSeenTick = tick;
            this.hits = 1;
        }
    }

    private final List<Track> tracks = new ArrayList<>();
    private long lastTickSeen = -1L;
    private long nextFireTick = 0L;
    private int pingFor = 0;
    private int evadeFor = 0;
    private int evadeFlip = 1;
    private float mapW = 1000f, mapH = 1000f;
    private float preferredRangeMin = 180f;
    private float preferredRangeMax = 260f;
    private float maxRange = 300f;
    private float shellSpeed = 50f;
    private int gunCooldown = 15;

    @Override
    public void onWelcome(Welcome w) {
        mapW = w.map().width();
        mapH = w.map().height();
        maxRange = w.shipSpecs().maxShellRange();
        shellSpeed = w.shipSpecs().shellSpeed();
        gunCooldown = w.shipSpecs().gunCooldownTicks();
        // Engagement envelope: comfortably inside max range but far enough to
        // give the lead solver a fighting chance.
        preferredRangeMax = maxRange * 0.85f;
        preferredRangeMin = maxRange * 0.55f;
    }

    @Override
    public void onGameStart(GameStart gs) {
        nextFireTick = 0L;
        tracks.clear();
        lastTickSeen = -1L;
        evadeFor = 0;
        pingFor = 0;
    }

    @Override
    public Command onTick(WorldView view) {
        // React to hits — start evasive maneuvering.
        for (TickEvent ev : view.events()) {
            if (ev instanceof TickEvent.Hit) {
                evadeFor = EVADE_TICKS;
            }
        }

        updateTracks(view);

        Track target = pickTarget(view.self().pos());
        SensorMode mode = chooseSensorMode(view, target);

        Command cmd = new Command().sensorMode(mode);
        steer(cmd, view, target);
        maybeFire(cmd, view, target);
        return cmd;
    }

    private SensorMode chooseSensorMode(WorldView view, Track target) {
        if (pingFor > 0) {
            pingFor--;
            return SensorMode.ACTIVE;
        }
        // No target yet: listen passively, only ping if we are completely blind.
        if (target == null) {
            return view.contacts().isEmpty() ? SensorMode.PASSIVE : SensorMode.PASSIVE;
        }
        // Have a track but no recent range — burst-ping to fix range.
        if (target.lastRangeTick < 0 || view.tick() - target.lastRangeTick > 6) {
            pingFor = PING_BURST_TICKS;
            return SensorMode.ACTIVE;
        }
        return SensorMode.PASSIVE;
    }

    private void steer(Command cmd, WorldView view, Track target) {
        Vec2 self = view.self().pos();
        float heading = view.self().headingDeg();

        // Default: cruise.
        float throttle = 0.7f;
        float rudder = 0.0f;

        if (target != null) {
            float dist = Geometry.distance(self, target.pos);
            float bearing = Geometry.bearingTo(self, target.pos);
            float delta = signedDelta(bearing - heading);

            if (dist > preferredRangeMax) {
                // Close the gap, point at the target.
                throttle = 1.0f;
                rudder = clamp(delta / 30f, -1f, 1f);
            } else if (dist < preferredRangeMin) {
                // Too close: turn broadside and back off slightly.
                float broadside = signedDelta(bearing - heading + 90f);
                throttle = 0.4f;
                rudder = clamp(broadside / 30f, -1f, 1f);
            } else {
                // In the sweet spot: keep the gun bearing on the target.
                throttle = 0.6f;
                rudder = clamp(delta / 30f, -1f, 1f);
            }
        }

        // Evasion: jink hard, alternating direction, while keeping forward speed.
        if (evadeFor > 0) {
            evadeFor--;
            if (evadeFor % 6 == 0) evadeFlip = -evadeFlip;
            rudder = evadeFlip * 1.0f;
            throttle = 1.0f;
        }

        // Edge avoidance: if we're nosing into the wall, override the rudder.
        Vec2 nose = new Vec2(
                self.x() + (float) Math.sin(Math.toRadians(heading)) * 30f,
                self.y() - (float) Math.cos(Math.toRadians(heading)) * 30f);
        if (nose.x() < MAP_MARGIN || nose.x() > mapW - MAP_MARGIN
                || nose.y() < MAP_MARGIN || nose.y() > mapH - MAP_MARGIN) {
            Vec2 center = new Vec2(mapW * 0.5f, mapH * 0.5f);
            float toCenter = Geometry.bearingTo(self, center);
            rudder = clamp(signedDelta(toCenter - heading) / 30f, -1f, 1f);
            throttle = Math.max(throttle, 0.5f);
        }

        cmd.throttle(throttle).rudder(rudder);
    }

    private void maybeFire(Command cmd, WorldView view, Track target) {
        if (target == null || target.hits < 2) return;
        if (view.self().ammo() <= 0) return;
        if (view.tick() < nextFireTick) return;

        Vec2 self = view.self().pos();
        float dist = Geometry.distance(self, target.pos);
        if (dist > maxRange) return;

        // Don't waste shells when the firing arc isn't lined up.
        float bearing = Geometry.bearingTo(self, target.pos);
        float delta = Math.abs(signedDelta(bearing - view.self().headingDeg()));
        if (delta > 60f && view.self().ammo() < 5) return;

        cmd.fireAt(self, target.pos, target.vel, shellSpeed);
        nextFireTick = view.tick() + gunCooldown;
    }

    private void updateTracks(WorldView view) {
        long tick = view.tick();
        float dt = (lastTickSeen < 0) ? 0f : (tick - lastTickSeen) / (float) welcome().tickHz();
        lastTickSeen = tick;

        for (Track t : tracks) {
            t.pos = new Vec2(t.pos.x() + t.vel.x() * dt, t.pos.y() + t.vel.y() * dt);
        }

        boolean[] usedTracks = new boolean[tracks.size()];
        for (Contact c : view.contacts()) {
            int bestIdx = -1;
            float bestDist = ASSOCIATION_GATE;
            for (int i = 0; i < tracks.size(); i++) {
                if (usedTracks[i]) continue;
                float d = Geometry.distance(tracks.get(i).pos, c.pos());
                if (d < bestDist) {
                    bestDist = d;
                    bestIdx = i;
                }
            }
            if (bestIdx >= 0) {
                Track t = tracks.get(bestIdx);
                if (dt > 0f) {
                    float vx = (c.pos().x() - t.pos.x()) / dt;
                    float vy = (c.pos().y() - t.pos.y()) / dt;
                    t.vel = new Vec2(
                            t.vel.x() * (1f - VELOCITY_ALPHA) + vx * VELOCITY_ALPHA,
                            t.vel.y() * (1f - VELOCITY_ALPHA) + vy * VELOCITY_ALPHA);
                }
                t.pos = c.pos();
                t.lastSeenTick = tick;
                if (c.range().isPresent()) t.lastRangeTick = tick;
                t.hits++;
                usedTracks[bestIdx] = true;
            } else {
                Track fresh = new Track(c.pos(), tick);
                if (c.range().isPresent()) fresh.lastRangeTick = tick;
                tracks.add(fresh);
            }
        }

        tracks.removeIf(t -> tick - t.lastSeenTick > STALE_AFTER_TICKS);
    }

    private Track pickTarget(Vec2 self) {
        Track best = null;
        float bestScore = Float.POSITIVE_INFINITY;
        for (Track t : tracks) {
            float dist = Geometry.distance(self, t.pos);
            // Reward confirmed tracks and proximity, penalize tracks we have
            // no range fix on (they're shakier).
            float score = dist
                    - 8f * Math.min(t.hits, 10)
                    + (t.lastRangeTick < 0 ? 60f : 0f);
            if (score < bestScore) {
                bestScore = score;
                best = t;
            }
        }
        return best;
    }

    private static float signedDelta(float deg) {
        float d = ((deg % 360f) + 540f) % 360f - 180f;
        return d;
    }

    private static float clamp(float v, float lo, float hi) {
        return Math.max(lo, Math.min(hi, v));
    }

    public static void main(String[] args) {
        String host = args.length > 0 ? args[0] : "localhost";
        int port    = args.length > 1 ? Integer.parseInt(args[1]) : 7878;
        String name = args.length > 2 ? args[2] : "hunter";
        BotRunner.run(new HunterBot(), host, port, name);
    }
}
