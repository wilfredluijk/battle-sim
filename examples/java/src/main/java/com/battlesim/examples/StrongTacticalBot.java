package com.battlesim.examples;

import com.battlesim.naval.Bot;
import com.battlesim.naval.BotRunner;
import com.battlesim.naval.Command;
import com.battlesim.naval.Geometry;
import com.battlesim.naval.protocol.Contact;
import com.battlesim.naval.protocol.ContactKind;
import com.battlesim.naval.protocol.MapInfo;
import com.battlesim.naval.protocol.SensorMode;
import com.battlesim.naval.protocol.TickEvent;
import com.battlesim.naval.protocol.Vec2;
import com.battlesim.naval.protocol.Welcome;
import com.battlesim.naval.protocol.WorldView;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.Iterator;
import java.util.List;
import java.util.Optional;

/**
 * A stronger example bot. It still sails circular courses, but layers several
 * tactical behaviors on top: multi-target tracking, active/passive radar
 * scheduling, wall avoidance, evasive breaks, target scoring, and led fire.
 */
public final class StrongTacticalBot extends Bot {
    private final List<Track> tracks = new ArrayList<>();
    private int nextTrackId = 1;
    private int tickHz = 10;
    private int cooldownTicks = 15;
    private float shellSpeed = 50.0f;
    private float maxRange = 300.0f;
    private MapInfo map = new MapInfo(1000, 1000);
    private long nextFireTick = 0;
    private long evasionUntilTick = -1;
    private int evasionDirection = 1;
    private long forcePingUntilTick = 0;

    @Override
    public void onWelcome(Welcome welcome) {
        tickHz = welcome.tickHz();
        cooldownTicks = welcome.shipSpecs().gunCooldownTicks();
        shellSpeed = welcome.shipSpecs().shellSpeed();
        maxRange = welcome.shipSpecs().maxShellRange();
        map = welcome.map();
    }

    @Override
    public Command onTick(WorldView view) {
        reactToEvents(view);
        updateTracks(view);

        Optional<Track> target = bestTarget(view);
        SensorMode sensorMode = chooseSensorMode(view, target);
        Helm helm = steer(view, target);

        Command command = new Command()
                .throttle(helm.throttle())
                .rudder(helm.rudder())
                .sensorMode(sensorMode);

        target.ifPresent(t -> fireIfReady(view, command, t));
        ageTracks(view.tick());

        return command;
    }

    @Override
    public void onError(String code, String message) {
        if ("cooldown_active".equals(code)) {
            nextFireTick = lastTick() + Math.max(3, cooldownTicks / 2);
        } else if ("no_ammo".equals(code)) {
            nextFireTick = Long.MAX_VALUE / 2;
        } else {
            super.onError(code, message);
        }
    }

    private void reactToEvents(WorldView view) {
        for (TickEvent event : view.events()) {
            if (event instanceof TickEvent.Hit) {
                evasionDirection *= -1;
                evasionUntilTick = view.tick() + 35;
                forcePingUntilTick = Math.max(forcePingUntilTick, view.tick() + 4);
            } else if (event instanceof TickEvent.ShellSplash splash) {
                float distance = Geometry.distance(view.self().pos(), splash.pos());
                if (distance < 90.0f) {
                    evasionDirection = angleDelta(view.self().headingDeg(),
                            Geometry.bearingTo(splash.pos(), view.self().pos())) >= 0.0f ? 1 : -1;
                    evasionUntilTick = Math.max(evasionUntilTick, view.tick() + 25);
                }
            }
        }
    }

    private void updateTracks(WorldView view) {
        List<Contact> usable = view.contacts().stream()
                .filter(c -> c.kind() == ContactKind.SHIP || c.kind() == ContactKind.UNKNOWN)
                .toList();

        List<Contact> active = usable.stream().filter(c -> c.range().isPresent()).toList();
        for (Contact contact : active) {
            int index = nearestTrackIndex(view, contact);
            if (index >= 0 && activeScore(view, tracks.get(index), contact) < 75.0f) {
                tracks.get(index).updateActive(view, contact, tickHz);
            } else {
                Track created = Track.fromActive(nextTrackId++, view.tick(), contact);
                tracks.add(created);
            }
        }

        List<Contact> passive = usable.stream().filter(c -> c.range().isEmpty()).toList();
        for (Contact contact : passive) {
            tracks.stream()
                    .filter(t -> !t.isStale(view.tick()))
                    .min(Comparator.comparingDouble(t -> passiveScore(view, t, contact)))
                    .filter(t -> passiveScore(view, t, contact) < 28.0f)
                    .ifPresent(t -> t.updatePassive(view, contact, tickHz));
        }

        for (Track track : tracks) {
            if (track.lastSeenTick != view.tick()) track.quality *= 0.985f;
        }
    }

    private int nearestTrackIndex(WorldView view, Contact contact) {
        int best = -1;
        float bestScore = Float.POSITIVE_INFINITY;
        for (int i = 0; i < tracks.size(); i++) {
            Track track = tracks.get(i);
            if (track.lastSeenTick == view.tick()) continue;
            float score = activeScore(view, track, contact);
            if (score < bestScore) {
                best = i;
                bestScore = score;
            }
        }
        return best;
    }

    private float activeScore(WorldView view, Track track, Contact contact) {
        return Geometry.distance(track.predict(view.tick(), tickHz), contact.pos());
    }

    private float passiveScore(WorldView view, Track track, Contact contact) {
        Vec2 predicted = track.predict(view.tick(), tickHz);
        float bearing = Geometry.bearingTo(view.self().pos(), predicted);
        return Math.abs(angleDelta(bearing, contact.bearingDeg()));
    }

    private Optional<Track> bestTarget(WorldView view) {
        return tracks.stream()
                .filter(t -> !t.isStale(view.tick()))
                .max(Comparator.comparingDouble(t -> targetScore(view, t)));
    }

    private double targetScore(WorldView view, Track track) {
        Vec2 predicted = track.predict(view.tick(), tickHz);
        float range = Geometry.distance(view.self().pos(), predicted);
        double rangeScore = Math.max(0.0, 1.0 - Math.abs(range - 210.0) / 300.0);
        double freshness = Math.max(0.0, 1.0 - (view.tick() - track.lastSeenTick) / 60.0);
        double activeBonus = view.tick() - track.lastActiveTick <= 18 ? 0.25 : 0.0;
        return track.quality * 2.0 + rangeScore + freshness + activeBonus;
    }

    private SensorMode chooseSensorMode(WorldView view, Optional<Track> target) {
        if (view.tick() <= forcePingUntilTick) return SensorMode.ACTIVE;
        if (target.isEmpty()) return view.tick() % 12 < 3 ? SensorMode.ACTIVE : SensorMode.PASSIVE;

        Track t = target.get();
        long activeAge = view.tick() - t.lastActiveTick;
        if (activeAge > 22 || t.quality < 0.42f) return SensorMode.ACTIVE;
        if (view.tick() >= nextFireTick && view.self().ammo() > 0 && activeAge > 8) return SensorMode.ACTIVE;
        return view.tick() % 48 == 0 ? SensorMode.ACTIVE : SensorMode.PASSIVE;
    }

    private Helm steer(WorldView view, Optional<Track> target) {
        Vec2 self = view.self().pos();
        float desired = circularBearingAround(new Vec2(map.width() / 2.0f, map.height() / 2.0f), self, 1);
        float throttle = 0.88f;

        if (target.isPresent()) {
            Vec2 targetPos = target.get().predict(view.tick(), tickHz);
            float targetRange = Geometry.distance(self, targetPos);
            desired = circularBearingAround(targetPos, self, targetRange < 170.0f ? -1 : 1);
            if (targetRange > 260.0f) {
                desired = Geometry.bearingTo(self, targetPos);
            } else if (targetRange < 90.0f) {
                desired = Geometry.bearingTo(targetPos, self);
                throttle = 0.65f;
            }
        }

        desired = avoidWalls(self, desired);
        float rudder = clamp(angleDelta(view.self().headingDeg(), desired) / 45.0f, -1.0f, 1.0f);

        if (view.tick() < evasionUntilTick) {
            rudder = clamp(rudder + 0.55f * evasionDirection, -1.0f, 1.0f);
            throttle = view.tick() % 18 < 6 ? 0.35f : 1.0f;
        }

        return new Helm(throttle, rudder);
    }

    private float circularBearingAround(Vec2 center, Vec2 self, int direction) {
        float away = Geometry.bearingTo(center, self);
        return normalizeBearing(away + 90.0f * direction);
    }

    private float avoidWalls(Vec2 self, float desired) {
        float margin = 95.0f;
        if (self.x() < margin || self.y() < margin
                || self.x() > map.width() - margin || self.y() > map.height() - margin) {
            Vec2 center = new Vec2(map.width() / 2.0f, map.height() / 2.0f);
            return blendBearing(desired, Geometry.bearingTo(self, center), 0.70f);
        }
        return desired;
    }

    private void fireIfReady(WorldView view, Command command, Track target) {
        if (view.self().ammo() <= 0 || view.tick() < nextFireTick) return;
        if (view.tick() - target.lastActiveTick > 18 || target.quality < 0.50f) return;

        Vec2 targetPos = target.predict(view.tick(), tickHz);
        float range = Geometry.distance(view.self().pos(), targetPos);
        if (range < 45.0f || range > maxRange) return;

        command.fireAt(view.self().pos(), targetPos, target.velocity, shellSpeed);
        nextFireTick = view.tick() + cooldownTicks;
        target.quality = Math.max(0.20f, target.quality - 0.08f);
    }

    private void ageTracks(long tick) {
        Iterator<Track> it = tracks.iterator();
        while (it.hasNext()) {
            Track track = it.next();
            if (tick - track.lastSeenTick > 110) {
                it.remove();
            }
        }
    }

    private static float blendBearing(float a, float b, float bWeight) {
        return normalizeBearing(a + angleDelta(a, b) * bWeight);
    }

    private static float normalizeBearing(float bearing) {
        float value = bearing % 360.0f;
        return value < 0.0f ? value + 360.0f : value;
    }

    private static float angleDelta(float from, float to) {
        return ((to - from + 540.0f) % 360.0f) - 180.0f;
    }

    private static float clamp(float value, float min, float max) {
        return Math.max(min, Math.min(max, value));
    }

    private record Helm(float throttle, float rudder) {}

    private static final class Track {
        private final int id;
        private Vec2 position;
        private Vec2 velocity;
        private long lastSeenTick;
        private long lastActiveTick;
        private float quality;

        private Track(int id, Vec2 position, Vec2 velocity, long lastSeenTick, long lastActiveTick, float quality) {
            this.id = id;
            this.position = position;
            this.velocity = velocity;
            this.lastSeenTick = lastSeenTick;
            this.lastActiveTick = lastActiveTick;
            this.quality = quality;
        }

        private static Track fromActive(int id, long tick, Contact contact) {
            return new Track(id, contact.pos(), new Vec2(0.0f, 0.0f), tick, tick, 0.58f);
        }

        private void updateActive(WorldView view, Contact contact, int tickHz) {
            float dt = Math.max(1.0f, view.tick() - lastSeenTick) / tickHz;
            Vec2 observedVelocity = new Vec2(
                    (contact.pos().x() - position.x()) / dt,
                    (contact.pos().y() - position.y()) / dt);
            position = blend(predict(view.tick(), tickHz), contact.pos(), 0.68f);
            velocity = blend(velocity, observedVelocity, 0.38f);
            lastSeenTick = view.tick();
            lastActiveTick = view.tick();
            quality = Math.min(1.0f, quality + 0.20f);
        }

        private void updatePassive(WorldView view, Contact contact, int tickHz) {
            Vec2 predicted = predict(view.tick(), tickHz);
            float range = Math.max(70.0f, Geometry.distance(view.self().pos(), predicted));
            Vec2 bearingFix = pointOnBearing(view.self().pos(), contact.bearingDeg(), range);
            position = blend(predicted, bearingFix, 0.18f);
            lastSeenTick = view.tick();
            quality = Math.max(0.12f, quality - 0.015f);
        }

        private Vec2 predict(long tick, int tickHz) {
            float dt = (tick - lastSeenTick) / (float) tickHz;
            return new Vec2(position.x() + velocity.x() * dt, position.y() + velocity.y() * dt);
        }

        private boolean isStale(long tick) {
            return tick - lastSeenTick > 80 || quality < 0.08f;
        }

        private static Vec2 blend(Vec2 a, Vec2 b, float bWeight) {
            float aWeight = 1.0f - bWeight;
            return new Vec2(a.x() * aWeight + b.x() * bWeight, a.y() * aWeight + b.y() * bWeight);
        }

        private static Vec2 pointOnBearing(Vec2 origin, float bearingDeg, float range) {
            double radians = Math.toRadians(bearingDeg);
            return new Vec2(
                    (float) (origin.x() + Math.sin(radians) * range),
                    (float) (origin.y() - Math.cos(radians) * range));
        }
    }

    public static void main(String[] args) {
        BotArgs parsed = BotArgs.parse(args, "strong-tactical");
        BotRunner.run(new StrongTacticalBot(), parsed.host(), parsed.port(), parsed.name());
    }
}
