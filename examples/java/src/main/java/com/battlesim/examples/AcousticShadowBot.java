package com.battlesim.examples;

import com.battlesim.naval.Bot;
import com.battlesim.naval.BotRunner;
import com.battlesim.naval.Command;
import com.battlesim.naval.Geometry;
import com.battlesim.naval.protocol.Contact;
import com.battlesim.naval.protocol.ContactKind;
import com.battlesim.naval.protocol.MapInfo;
import com.battlesim.naval.protocol.SensorMode;
import com.battlesim.naval.protocol.ShipSpecs;
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
 * A sound-first tactical bot.
 *
 * <p>The bot spends most of the match passive, using active-radar opponents as
 * unwilling beacons. Bearing-only contacts are triangulated across the bot's own
 * movement, then confirmed with short radar bursts immediately before firing.
 * Movement tries to stay circular and hard to predict: shadow at range, orbit
 * across the target's bearing, break away after splashes, and avoid giving away
 * active pings unless the information is worth the exposure.
 */
public final class AcousticShadowBot extends Bot {
    private static final float PASSIVE_NEAR_RANGE = 150.0f;
    private static final float PASSIVE_PINGER_RANGE = 500.0f;

    private final List<Track> tracks = new ArrayList<>();

    private int tickHz = 10;
    private int cooldownTicks = 15;
    private float shellSpeed = 50.0f;
    private float maxShellRange = 300.0f;
    private float splashRadius = 15.0f;
    private int hullHp = 100;
    private MapInfo map = new MapInfo(1000, 1000);

    private long nextFireTick = 0;
    private long activeBurstUntilTick = -1;
    private long evasionUntilTick = -1;
    private long reverseUntilTick = -1;
    private long lastContactTick = -9999;
    private long lastDamageTick = -9999;
    private int orbitDirection = 1;
    private int lastHp = -1;
    private Vec2 lastThreatPos;

    @Override
    public void onWelcome(Welcome welcome) {
        ShipSpecs specs = welcome.shipSpecs();
        tickHz = welcome.tickHz();
        cooldownTicks = specs.gunCooldownTicks();
        shellSpeed = specs.shellSpeed();
        maxShellRange = specs.maxShellRange();
        splashRadius = specs.splashRadius();
        hullHp = specs.hullHp();
        map = welcome.map();
    }

    @Override
    public Command onTick(WorldView view) {
        if (lastHp < 0) lastHp = view.self().hp();

        readBattleDamage(view);
        updateTracks(view);

        Optional<Track> target = chooseTarget(view);
        SensorMode sensorMode = chooseSensorMode(view, target);
        Helm helm = chooseHelm(view, target);

        Command command = new Command()
                .throttle(helm.throttle())
                .rudder(helm.rudder())
                .sensorMode(sensorMode);

        target.ifPresent(t -> fireIfProfitable(view, command, t));
        ageTracks(view.tick());
        lastHp = view.self().hp();

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

    private void readBattleDamage(WorldView view) {
        if (view.self().hp() < lastHp) {
            lastDamageTick = view.tick();
            evasionUntilTick = Math.max(evasionUntilTick, view.tick() + 42);
            reverseUntilTick = Math.max(reverseUntilTick, view.tick() + 8);
            orbitDirection *= -1;
            activeBurstUntilTick = Math.max(activeBurstUntilTick, view.tick() + 3);
        }

        for (TickEvent event : view.events()) {
            if (event instanceof TickEvent.Hit) {
                lastDamageTick = view.tick();
                evasionUntilTick = Math.max(evasionUntilTick, view.tick() + 46);
                reverseUntilTick = Math.max(reverseUntilTick, view.tick() + 7);
                orbitDirection *= -1;
                activeBurstUntilTick = Math.max(activeBurstUntilTick, view.tick() + 4);
            } else if (event instanceof TickEvent.ShellSplash splash) {
                float distance = Geometry.distance(view.self().pos(), splash.pos());
                if (distance < 100.0f) {
                    lastThreatPos = splash.pos();
                    long evasionTicks = distance < splashRadius * 4.0f ? 44 : 24;
                    evasionUntilTick = Math.max(evasionUntilTick, view.tick() + evasionTicks);
                    if (distance < splashRadius * 2.2f) {
                        reverseUntilTick = Math.max(reverseUntilTick, view.tick() + 5);
                    }
                    orbitDirection = angleDelta(
                            Geometry.bearingTo(splash.pos(), view.self().pos()),
                            view.self().headingDeg()) >= 0.0f ? 1 : -1;
                }
            }
        }
    }

    private void updateTracks(WorldView view) {
        List<Contact> shipContacts = view.contacts().stream()
                .filter(c -> c.kind() == ContactKind.SHIP || c.kind() == ContactKind.UNKNOWN)
                .toList();

        for (Contact contact : shipContacts.stream().filter(c -> c.range().isPresent()).toList()) {
            Track track = bestActiveMatch(view, contact)
                    .filter(t -> activeScore(view, t, contact) < activeGate(t))
                    .orElseGet(() -> createTrackFromActive(view, contact));
            track.updateActive(view, contact, tickHz);
            lastContactTick = view.tick();
        }

        for (Contact contact : shipContacts.stream().filter(c -> c.range().isEmpty()).toList()) {
            Track track = bestPassiveMatch(view, contact)
                    .filter(t -> passiveScore(view, t, contact) < passiveGate(t, contact))
                    .orElseGet(() -> createTrackFromPassive(view, contact));
            track.updatePassive(view, contact, tickHz, map);
            lastContactTick = view.tick();
        }

        for (Track track : tracks) {
            if (track.lastSeenTick != view.tick()) {
                track.quality *= 0.992f;
                track.uncertainty = Math.min(180.0f, track.uncertainty + 0.85f);
            }
            track.pingerHeat *= 0.985f;
        }
    }

    private Optional<Track> bestActiveMatch(WorldView view, Contact contact) {
        return tracks.stream()
                .filter(t -> t.lastSeenTick != view.tick())
                .min(Comparator.comparingDouble(t -> activeScore(view, t, contact)));
    }

    private double activeScore(WorldView view, Track track, Contact contact) {
        Vec2 predicted = track.predict(view.tick(), tickHz);
        float distance = Geometry.distance(predicted, contact.pos());
        long stale = view.tick() - track.lastSeenTick;
        return distance + Math.max(0, stale - 35) * 1.5;
    }

    private float activeGate(Track track) {
        return 48.0f + track.uncertainty * 0.45f + (1.0f - track.quality) * 28.0f;
    }

    private Optional<Track> bestPassiveMatch(WorldView view, Contact contact) {
        return tracks.stream()
                .filter(t -> t.lastSeenTick != view.tick())
                .min(Comparator.comparingDouble(t -> passiveScore(view, t, contact)));
    }

    private double passiveScore(WorldView view, Track track, Contact contact) {
        Vec2 predicted = track.predict(view.tick(), tickHz);
        float predictedBearing = Geometry.bearingTo(view.self().pos(), predicted);
        float bearingError = Math.abs(angleDelta(predictedBearing, contact.bearingDeg()));
        float predictedRange = Geometry.distance(view.self().pos(), predicted);
        float rangePenalty = 0.0f;
        if (contact.confidence() >= 0.75f && predictedRange > PASSIVE_PINGER_RANGE + 60.0f) {
            rangePenalty = (predictedRange - PASSIVE_PINGER_RANGE) / 55.0f;
        } else if (contact.confidence() < 0.75f && predictedRange > PASSIVE_NEAR_RANGE + 80.0f) {
            rangePenalty = (predictedRange - PASSIVE_NEAR_RANGE) / 40.0f;
        }
        return bearingError + rangePenalty + (1.0f - contact.confidence()) * 1.5f;
    }

    private float passiveGate(Track track, Contact contact) {
        float base = contact.confidence() >= 0.75f ? 22.0f : 16.0f;
        return base + track.uncertainty * 0.04f + (1.0f - track.quality) * 5.0f;
    }

    private Track createTrackFromActive(WorldView view, Contact contact) {
        Track track = new Track(contact.pos(), new Vec2(0.0f, 0.0f), view.tick());
        track.lastActiveTick = view.tick();
        track.rangeEstimate = (float) contact.range().orElse(Geometry.distance(view.self().pos(), contact.pos()));
        track.quality = 0.56f;
        track.uncertainty = 18.0f;
        tracks.add(track);
        return track;
    }

    private Track createTrackFromPassive(WorldView view, Contact contact) {
        float initialRange = contact.confidence() >= 0.75f ? 330.0f : 125.0f;
        Vec2 estimate = clampToMap(pointOnBearing(view.self().pos(), contact.bearingDeg(), initialRange), 5.0f);
        Track track = new Track(estimate, new Vec2(0.0f, 0.0f), view.tick());
        track.rangeEstimate = initialRange;
        track.quality = contact.confidence() >= 0.75f ? 0.32f : 0.24f;
        track.uncertainty = contact.confidence() >= 0.75f ? 120.0f : 70.0f;
        tracks.add(track);
        return track;
    }

    private Optional<Track> chooseTarget(WorldView view) {
        return tracks.stream()
                .filter(t -> !t.isStale(view.tick()))
                .max(Comparator.comparingDouble(t -> targetScore(view, t)));
    }

    private double targetScore(WorldView view, Track track) {
        Vec2 pos = track.predict(view.tick(), tickHz);
        float range = Geometry.distance(view.self().pos(), pos);
        double rangeScore = 1.0 - Math.min(1.0, Math.abs(range - desiredStandOff(view)) / 260.0);
        double freshness = 1.0 - Math.min(1.0, (view.tick() - track.lastSeenTick) / 90.0);
        double activeFresh = view.tick() - track.lastActiveTick <= 18 ? 0.35 : 0.0;
        double loudBonus = Math.min(0.55, track.pingerHeat * 0.5);
        double shootable = range > 55.0f && range < maxShellRange + 20.0f ? 0.25 : -0.35;
        return track.quality * 2.8 + freshness + rangeScore + activeFresh + loudBonus + shootable;
    }

    private SensorMode chooseSensorMode(WorldView view, Optional<Track> target) {
        if (view.tick() <= activeBurstUntilTick) return SensorMode.ACTIVE;
        if (view.self().ammo() <= 0) return SensorMode.PASSIVE;

        if (target.isEmpty()) {
            long silence = view.tick() - lastContactTick;
            return silence > 80 && view.tick() % 36 < 3 ? SensorMode.ACTIVE : SensorMode.PASSIVE;
        }

        Track t = target.get();
        long activeAge = view.tick() - t.lastActiveTick;
        boolean wounded = view.self().hp() <= hullHp * 0.45f;
        boolean gunReady = view.self().ammo() > 0 && view.tick() >= nextFireTick;
        boolean hasAcousticSolution = t.quality > 0.48f && t.uncertainty < 78.0f;
        boolean inWeaponEnvelope = rangeTo(view, t) > 65.0f && rangeTo(view, t) < maxShellRange - 8.0f;

        if (view.tick() - lastDamageTick < 18) return SensorMode.ACTIVE;
        if (gunReady && inWeaponEnvelope && activeAge > 9 && hasAcousticSolution) {
            activeBurstUntilTick = view.tick() + 2;
            return SensorMode.ACTIVE;
        }
        if (activeAge > 55 && (t.quality < 0.50f || wounded)) {
            activeBurstUntilTick = view.tick() + 2;
            return SensorMode.ACTIVE;
        }
        if (view.tick() % 95 == 0 && activeAge > 20 && t.pingerHeat < 0.20f) {
            return SensorMode.ACTIVE;
        }
        return SensorMode.PASSIVE;
    }

    private Helm chooseHelm(WorldView view, Optional<Track> target) {
        Vec2 self = view.self().pos();
        float throttle = 0.82f;
        float desired;

        if (target.isPresent()) {
            Track t = target.get();
            Vec2 targetPos = t.predict(view.tick(), tickHz);
            float range = Geometry.distance(self, targetPos);
            float tangent = circularBearingAround(targetPos, self, orbitDirection);
            float away = Geometry.bearingTo(targetPos, self);
            float toward = Geometry.bearingTo(self, targetPos);
            float standOff = desiredStandOff(view);

            if (range > standOff + 85.0f) {
                desired = blendBearing(toward, tangent, t.pingerHeat > 0.35f ? 0.42f : 0.25f);
                throttle = 1.0f;
            } else if (range < standOff - 80.0f) {
                desired = blendBearing(away, tangent, 0.34f);
                throttle = range < 95.0f ? 0.94f : 0.76f;
            } else {
                desired = blendBearing(tangent, range < standOff ? away : toward, 0.22f);
                throttle = t.pingerHeat > 0.45f ? 0.66f : 0.76f;
            }
        } else {
            Vec2 center = new Vec2(map.width() / 2.0f, map.height() / 2.0f);
            desired = circularBearingAround(center, self, orbitDirection);
            throttle = 0.72f;
            float centerRange = Geometry.distance(self, center);
            if (centerRange > Math.min(map.width(), map.height()) * 0.42f) {
                desired = blendBearing(desired, Geometry.bearingTo(self, center), 0.35f);
            }
        }

        desired = avoidWalls(self, desired);

        if (lastThreatPos != null && view.tick() < evasionUntilTick) {
            float threatAway = Geometry.bearingTo(lastThreatPos, self);
            desired = blendBearing(threatAway, desired, 0.35f);
        }

        float rudder = clamp(angleDelta(view.self().headingDeg(), desired) / 42.0f, -1.0f, 1.0f);

        if (view.tick() < evasionUntilTick) {
            rudder = clamp(rudder + orbitDirection * 0.45f, -1.0f, 1.0f);
            throttle = view.tick() < reverseUntilTick ? -0.72f : (view.tick() % 20 < 7 ? 0.38f : 1.0f);
        }

        if (nearWall(self, 55.0f)) {
            throttle = Math.min(throttle, 0.55f);
        }

        return new Helm(throttle, rudder);
    }

    private float desiredStandOff(WorldView view) {
        if (view.self().hp() <= hullHp * 0.35f) return Math.min(275.0f, maxShellRange - 18.0f);
        if (view.self().ammo() <= 4) return Math.min(265.0f, maxShellRange - 20.0f);
        return Math.min(230.0f, maxShellRange - 35.0f);
    }

    private void fireIfProfitable(WorldView view, Command command, Track target) {
        if (view.self().ammo() <= 0 || view.tick() < nextFireTick) return;

        long activeAge = view.tick() - target.lastActiveTick;
        float range = rangeTo(view, target);
        if (activeAge > 16 || target.quality < requiredShotQuality(view)) return;
        if (range < Math.max(45.0f, splashRadius * 3.0f) || range > maxShellRange - 6.0f) return;
        if (target.uncertainty > shotUncertaintyLimit(view)) return;

        Vec2 targetPos = target.predict(view.tick(), tickHz);
        Optional<Vec2> lead = Geometry.leadTarget(view.self().pos(), targetPos, target.velocity, shellSpeed);
        if (lead.isPresent() && Geometry.distance(view.self().pos(), lead.get()) > maxShellRange - 2.0f) {
            return;
        }

        command.fireAt(view.self().pos(), targetPos, target.velocity, shellSpeed);
        nextFireTick = view.tick() + cooldownTicks;
        activeBurstUntilTick = Math.max(activeBurstUntilTick, view.tick() + 1);
        target.quality = Math.max(0.25f, target.quality - 0.06f);
    }

    private float requiredShotQuality(WorldView view) {
        if (view.self().ammo() <= 5) return 0.68f;
        if (view.self().hp() <= hullHp * 0.35f) return 0.52f;
        return 0.58f;
    }

    private float shotUncertaintyLimit(WorldView view) {
        if (view.self().ammo() <= 5) return 34.0f;
        if (view.tick() - lastDamageTick < 30) return 58.0f;
        return 45.0f;
    }

    private void ageTracks(long tick) {
        Iterator<Track> it = tracks.iterator();
        while (it.hasNext()) {
            Track track = it.next();
            if (tick - track.lastSeenTick > 150 || track.quality < 0.05f) {
                it.remove();
            }
        }
    }

    private float rangeTo(WorldView view, Track track) {
        return Geometry.distance(view.self().pos(), track.predict(view.tick(), tickHz));
    }

    private Vec2 clampToMap(Vec2 pos, float margin) {
        return new Vec2(
                clamp(pos.x(), margin, map.width() - margin),
                clamp(pos.y(), margin, map.height() - margin));
    }

    private float avoidWalls(Vec2 self, float desired) {
        float margin = 105.0f;
        if (!nearWall(self, margin)) return desired;

        Vec2 center = new Vec2(map.width() / 2.0f, map.height() / 2.0f);
        float centerBearing = Geometry.bearingTo(self, center);
        float weight = nearWall(self, 45.0f) ? 0.88f : 0.62f;
        return blendBearing(desired, centerBearing, weight);
    }

    private boolean nearWall(Vec2 self, float margin) {
        return self.x() < margin
                || self.y() < margin
                || self.x() > map.width() - margin
                || self.y() > map.height() - margin;
    }

    private static float circularBearingAround(Vec2 center, Vec2 self, int direction) {
        float away = Geometry.bearingTo(center, self);
        return normalizeBearing(away + 90.0f * direction);
    }

    private static Vec2 pointOnBearing(Vec2 origin, float bearingDeg, float range) {
        Vec2 dir = unitFromBearing(bearingDeg);
        return new Vec2(origin.x() + dir.x() * range, origin.y() + dir.y() * range);
    }

    private static Vec2 unitFromBearing(float bearingDeg) {
        double radians = Math.toRadians(bearingDeg);
        return new Vec2((float) Math.sin(radians), (float) -Math.cos(radians));
    }

    private static Optional<Vec2> intersectBearingRays(
            Vec2 aOrigin, float aBearing, Vec2 bOrigin, float bBearing) {
        Vec2 r = unitFromBearing(aBearing);
        Vec2 s = unitFromBearing(bBearing);
        Vec2 delta = subtract(bOrigin, aOrigin);
        float denominator = cross(r, s);
        if (Math.abs(denominator) < 0.08f) return Optional.empty();

        float t = cross(delta, s) / denominator;
        float u = cross(delta, r) / denominator;
        if (t < 0.0f || u < 0.0f) return Optional.empty();
        return Optional.of(new Vec2(aOrigin.x() + r.x() * t, aOrigin.y() + r.y() * t));
    }

    private static Vec2 add(Vec2 a, Vec2 b) {
        return new Vec2(a.x() + b.x(), a.y() + b.y());
    }

    private static Vec2 subtract(Vec2 a, Vec2 b) {
        return new Vec2(a.x() - b.x(), a.y() - b.y());
    }

    private static Vec2 scale(Vec2 v, float scale) {
        return new Vec2(v.x() * scale, v.y() * scale);
    }

    private static float cross(Vec2 a, Vec2 b) {
        return a.x() * b.y() - a.y() * b.x();
    }

    private static Vec2 blend(Vec2 a, Vec2 b, float bWeight) {
        float aWeight = 1.0f - bWeight;
        return new Vec2(a.x() * aWeight + b.x() * bWeight, a.y() * aWeight + b.y() * bWeight);
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
        private Vec2 position;
        private Vec2 velocity;
        private long lastSeenTick;
        private long lastActiveTick = -9999;
        private float lastBearingDeg;
        private Vec2 lastObserverPos;
        private float rangeEstimate = 260.0f;
        private float quality = 0.25f;
        private float uncertainty = 120.0f;
        private float pingerHeat = 0.0f;

        private Track(Vec2 position, Vec2 velocity, long tick) {
            this.position = position;
            this.velocity = velocity;
            this.lastSeenTick = tick;
        }

        private void updateActive(WorldView view, Contact contact, int tickHz) {
            Vec2 predicted = predict(view.tick(), tickHz);
            float dt = Math.max(1.0f, view.tick() - lastSeenTick) / tickHz;
            Vec2 observedVelocity = scale(subtract(contact.pos(), position), 1.0f / dt);

            position = blend(predicted, contact.pos(), 0.78f);
            velocity = blend(velocity, observedVelocity, quality > 0.45f ? 0.42f : 0.24f);
            rangeEstimate = (float) contact.range().orElse(Geometry.distance(view.self().pos(), contact.pos()));
            lastSeenTick = view.tick();
            lastActiveTick = view.tick();
            quality = Math.min(1.0f, quality + 0.26f);
            uncertainty = Math.max(8.0f, uncertainty * 0.42f);
        }

        private void updatePassive(WorldView view, Contact contact, int tickHz, MapInfo map) {
            Vec2 predicted = predict(view.tick(), tickHz);
            float acousticRange = contact.confidence() >= 0.75f ? PASSIVE_PINGER_RANGE * 0.68f : PASSIVE_NEAR_RANGE * 0.82f;
            float workingRange = clamp(
                    rangeEstimate * 0.78f + acousticRange * 0.22f,
                    70.0f,
                    contact.confidence() >= 0.75f ? PASSIVE_PINGER_RANGE : PASSIVE_NEAR_RANGE + 70.0f);
            Vec2 bearingFix = pointOnBearing(view.self().pos(), contact.bearingDeg(), workingRange);
            Optional<Vec2> triangulated = Optional.empty();

            if (lastObserverPos != null && Geometry.distance(view.self().pos(), lastObserverPos) > 10.0f) {
                triangulated = intersectBearingRays(lastObserverPos, lastBearingDeg, view.self().pos(), contact.bearingDeg())
                        .filter(p -> insideMap(p, map, 25.0f))
                        .filter(p -> Geometry.distance(view.self().pos(), p) <= PASSIVE_PINGER_RANGE + 90.0f)
                        .filter(p -> Geometry.distance(p, predicted) <= Math.max(90.0f, uncertainty * 2.4f));
            }

            Vec2 observation = triangulated.orElse(bearingFix);
            float observationWeight = triangulated.isPresent() ? 0.46f : 0.18f;
            if (contact.confidence() >= 0.75f) observationWeight += 0.07f;

            float dt = Math.max(1.0f, view.tick() - lastSeenTick) / tickHz;
            Vec2 observedVelocity = scale(subtract(observation, position), 1.0f / dt);
            position = blend(predicted, observation, observationWeight);
            velocity = blend(velocity, observedVelocity, triangulated.isPresent() ? 0.24f : 0.08f);
            rangeEstimate = Geometry.distance(view.self().pos(), position);
            lastSeenTick = view.tick();
            lastObserverPos = view.self().pos();
            lastBearingDeg = contact.bearingDeg();
            if (contact.confidence() >= 0.75f) {
                pingerHeat = Math.min(1.0f, pingerHeat + 0.16f);
                quality = Math.min(0.86f, quality + (triangulated.isPresent() ? 0.11f : 0.055f));
            } else {
                quality = Math.min(0.72f, quality + (triangulated.isPresent() ? 0.07f : 0.025f));
            }
            uncertainty = Math.max(18.0f, uncertainty * (triangulated.isPresent() ? 0.72f : 0.91f));
        }

        private Vec2 predict(long tick, int tickHz) {
            float dt = (tick - lastSeenTick) / (float) tickHz;
            return add(position, scale(velocity, dt));
        }

        private boolean isStale(long tick) {
            return tick - lastSeenTick > 115 || quality < 0.08f;
        }

        private static boolean insideMap(Vec2 pos, MapInfo map, float margin) {
            return pos.x() >= margin
                    && pos.y() >= margin
                    && pos.x() <= map.width() - margin
                    && pos.y() <= map.height() - margin;
        }
    }

    public static void main(String[] args) {
        BotArgs parsed = BotArgs.parse(args, "acoustic-shadow");
        BotRunner.run(new AcousticShadowBot(), parsed.host(), parsed.port(), parsed.name());
    }
}
