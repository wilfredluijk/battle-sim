package com.battlesim.examples;

import com.battlesim.naval.Bot;
import com.battlesim.naval.BotRunner;
import com.battlesim.naval.Command;
import com.battlesim.naval.Geometry;
import com.battlesim.naval.protocol.Contact;
import com.battlesim.naval.protocol.ContactKind;
import com.battlesim.naval.protocol.FireCommand;
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
 * ApexDuelistBot: a Java L1/L2 hybrid tuned for one-on-one duels.
 *
 * <p>The doctrine is deliberately anti-acoustic: stay quiet, build bearing geometry
 * while passive, use tiny active confirmations when a shot is worth the exposure,
 * then change course so opponents cannot reuse a stale lead.
 */
public final class ApexDuelistBot extends Bot {
    private final HybridTracker tracker = new HybridTracker();
    private final SensorDirector sensors = new SensorDirector();
    private final MovementPlanner movement = new MovementPlanner();
    private final ShotDirector shots = new ShotDirector();

    private int tickHz = 10;
    private ShipSpecs specs = new ShipSpecs(9.0f, 2.0f, 3.5f, 20.0f, 100, 250, 15,
            8.0f, 70.0f, 300.0f, 15.0f, 25);
    private MapInfo map = new MapInfo(700, 700);

    private long activeBurstUntil = -1;
    private long evasionUntil = -1;
    private long reverseUntil = -1;
    private long lastDamageTick = -9999;
    private long lastPingTick = -9999;
    private int orbitDirection = 1;
    private int lastHp = -1;
    private Vec2 lastThreatPos = null;

    @Override
    public void onWelcome(Welcome welcome) {
        this.tickHz = welcome.tickHz();
        this.specs = welcome.shipSpecs();
        this.map = welcome.map();
        tracker.configure(tickHz, map);
        sensors.configure(specs, tickHz);
        movement.configure(specs, map);
        shots.configure(specs, tickHz);
    }

    @Override
    public Command onTick(WorldView view) {
        if (lastHp < 0) {
            lastHp = view.self().hp();
        }

        readBattleSignals(view);
        tracker.update(view);
        tracker.noteSplashes(view, specs);

        Optional<TrackModel> target = tracker.bestTarget(view, specs);
        SensorMode sensor = sensors.choose(view, target, tracker, shots, activeBurstUntil, lastDamageTick);
        if (sensor == SensorMode.ACTIVE) {
            lastPingTick = view.tick();
        }

        HelmOrder helm = movement.plan(view, target, orbitDirection, evasionUntil, reverseUntil, lastThreatPos);
        Command command = new Command()
                .throttle(helm.throttle())
                .rudder(helm.rudder())
                .sensorMode(sensor);

        if (target.isPresent() && shots.tryFire(view, command, target.get(), lastDamageTick)) {
            activeBurstUntil = Math.max(activeBurstUntil, view.tick() + 1);
            orbitDirection *= -1;
            target.get().quality = Math.max(0.20f, target.get().quality - 0.05f);
        }

        if (sensor == SensorMode.ACTIVE && view.tick() - lastPingTick <= 1) {
            orbitDirection = view.tick() % 2 == 0 ? orbitDirection : -orbitDirection;
        }

        lastHp = view.self().hp();
        return command;
    }

    @Override
    public void onError(String code, String message) {
        if ("cooldown_active".equals(code)) {
            shots.delayAfterCooldownError(lastTick());
        } else if ("no_ammo".equals(code)) {
            shots.disable();
        } else {
            super.onError(code, message);
        }
    }

    private void readBattleSignals(WorldView view) {
        if (view.self().hp() < lastHp) {
            markUnderFire(view, view.tick() + 46, view.tick() + 8, null);
        }

        for (TickEvent event : view.events()) {
            if (event instanceof TickEvent.Hit) {
                markUnderFire(view, view.tick() + 50, view.tick() + 7, null);
            } else if (event instanceof TickEvent.ShellSplash splash) {
                float d = Geometry.distance(view.self().pos(), splash.pos());
                if (d < 115.0f) {
                    long evadeTicks = d < specs.splashRadius() * 4.0f ? 44 : 24;
                    long reverseTicks = d < specs.splashRadius() * 2.3f ? 6 : 0;
                    markUnderFire(view, view.tick() + evadeTicks, view.tick() + reverseTicks, splash.pos());
                }
            }
        }
    }

    private void markUnderFire(WorldView view, long evadeTo, long reverseTo, Vec2 threat) {
        lastDamageTick = view.tick();
        evasionUntil = Math.max(evasionUntil, evadeTo);
        reverseUntil = Math.max(reverseUntil, reverseTo);
        activeBurstUntil = Math.max(activeBurstUntil, view.tick() + 3);
        orbitDirection *= -1;
        if (threat != null) {
            lastThreatPos = threat;
        }
    }

    public static void main(String[] args) {
        BotArgs parsed = BotArgs.parse(args, "apex-duelist");
        BotRunner.run(new ApexDuelistBot(), parsed.host(), parsed.port(), parsed.name());
    }

    private static final class HybridTracker {
        private static final float PASSIVE_NEAR_RANGE = 150.0f;
        private static final float PASSIVE_PINGER_RANGE = 500.0f;

        private final List<TrackModel> tracks = new ArrayList<>();
        private int tickHz = 10;
        private MapInfo map = new MapInfo(700, 700);
        private int nextId = 1;
        private long lastContactTick = -9999;

        void configure(int tickHz, MapInfo map) {
            this.tickHz = tickHz;
            this.map = map;
        }

        List<TrackModel> update(WorldView view) {
            List<Contact> active = new ArrayList<>();
            List<Contact> passive = new ArrayList<>();
            for (Contact contact : view.contacts()) {
                if (contact.kind() != ContactKind.SHIP && contact.kind() != ContactKind.UNKNOWN) {
                    continue;
                }
                if (contact.range().isPresent()) {
                    active.add(contact);
                } else {
                    passive.add(contact);
                }
            }

            active.sort(Comparator.comparing(Contact::id));
            passive.sort(Comparator.comparing(Contact::id));

            for (Contact contact : active) {
                TrackModel track = bestActiveMatch(view, contact)
                        .filter(t -> activeScore(view, t, contact) <= activeGate(t))
                        .orElseGet(() -> createFromActive(view, contact));
                track.updateActive(view, contact, tickHz);
                lastContactTick = view.tick();
            }

            for (Contact contact : passive) {
                TrackModel track = bestPassiveMatch(view, contact)
                        .filter(t -> passiveScore(view, t, contact) <= passiveGate(t, contact))
                        .orElseGet(() -> createFromPassive(view, contact));
                track.updatePassive(view, contact, tickHz, map);
                lastContactTick = view.tick();
            }

            for (TrackModel track : tracks) {
                if (track.lastSeenTick != view.tick()) {
                    track.quality *= 0.990f;
                    track.uncertainty = Math.min(220.0f, track.uncertainty + 0.90f);
                }
                track.pingerHeat *= 0.985f;
            }
            age(view.tick());
            return List.copyOf(tracks);
        }

        Optional<TrackModel> bestTarget(WorldView view, ShipSpecs specs) {
            return tracks.stream()
                    .filter(t -> !t.isStale(view.tick()))
                    .max(Comparator.comparingDouble(t -> targetScore(view, t, specs)));
        }

        boolean hasFreshTrack(long tick) {
            return tracks.stream().anyMatch(t -> tick - t.lastSeenTick <= 24 && t.quality > 0.30f);
        }

        long lastContactTick() {
            return lastContactTick;
        }

        private double targetScore(WorldView view, TrackModel track, ShipSpecs specs) {
            Vec2 predicted = track.predict(view.tick(), tickHz);
            float range = Geometry.distance(view.self().pos(), predicted);
            float standoff = view.self().hp() <= specs.hullHp() * 0.35f
                    ? Math.min(275.0f, specs.maxShellRange() - 18.0f)
                    : Math.min(235.0f, specs.maxShellRange() - 35.0f);
            double rangeScore = 1.0 - Math.min(1.0, Math.abs(range - standoff) / 260.0);
            double freshness = 1.0 - Math.min(1.0, (view.tick() - track.lastSeenTick) / 95.0);
            double activeFresh = view.tick() - track.lastActiveTick <= 14 ? 0.42 : 0.0;
            double shootable = range > 45.0f && range < specs.maxShellRange() - 4.0f ? 0.25 : -0.40;
            double damageBias = Math.min(0.65, track.estimatedDamage / 60.0);
            return track.quality * 3.0 + freshness + rangeScore + activeFresh
                    + Math.min(0.50, track.pingerHeat * 0.48) + shootable + damageBias
                    - track.uncertainty / 220.0;
        }

        void noteSplashes(WorldView view, ShipSpecs specs) {
            for (TickEvent event : view.events()) {
                if (!(event instanceof TickEvent.ShellSplash splash)) {
                    continue;
                }
                for (TrackModel track : tracks) {
                    Vec2 predicted = track.predict(view.tick(), tickHz);
                    float d = Geometry.distance(predicted, splash.pos());
                    if (d <= specs.splashRadius() * 1.8f) {
                        float frac = Math.max(0.0f, 1.0f - d / specs.splashRadius());
                        track.estimatedDamage += Math.round(specs.maxSplashDamage() * frac);
                        track.quality = Math.min(1.0f, track.quality + 0.05f);
                    }
                }
            }
        }

        private Optional<TrackModel> bestActiveMatch(WorldView view, Contact contact) {
            return tracks.stream()
                    .filter(t -> t.lastSeenTick != view.tick())
                    .min(Comparator.comparingDouble(t -> activeScore(view, t, contact)));
        }

        private double activeScore(WorldView view, TrackModel track, Contact contact) {
            Vec2 predicted = track.predict(view.tick(), tickHz);
            long stale = view.tick() - track.lastSeenTick;
            return Geometry.distance(predicted, contact.pos()) + Math.max(0, stale - 45) * 1.25;
        }

        private float activeGate(TrackModel track) {
            return 52.0f + track.uncertainty * 0.35f + (1.0f - track.quality) * 24.0f;
        }

        private Optional<TrackModel> bestPassiveMatch(WorldView view, Contact contact) {
            return tracks.stream()
                    .filter(t -> t.lastSeenTick != view.tick())
                    .min(Comparator.comparingDouble(t -> passiveScore(view, t, contact)));
        }

        private double passiveScore(WorldView view, TrackModel track, Contact contact) {
            Vec2 predicted = track.predict(view.tick(), tickHz);
            float bearing = Geometry.bearingTo(view.self().pos(), predicted);
            float bearingError = Math.abs(Geometry.signedBearingDelta(contact.bearingDeg(), bearing));
            float predictedRange = Geometry.distance(view.self().pos(), predicted);
            float rangePenalty = 0.0f;
            if (contact.confidence() >= 0.75f && predictedRange > PASSIVE_PINGER_RANGE + 70.0f) {
                rangePenalty = (predictedRange - PASSIVE_PINGER_RANGE) / 48.0f;
            } else if (contact.confidence() < 0.75f && predictedRange > PASSIVE_NEAR_RANGE + 85.0f) {
                rangePenalty = (predictedRange - PASSIVE_NEAR_RANGE) / 36.0f;
            }
            return bearingError + rangePenalty + (1.0f - contact.confidence()) * 1.8f;
        }

        private float passiveGate(TrackModel track, Contact contact) {
            float base = contact.confidence() >= 0.75f ? 24.0f : 17.0f;
            return base + track.uncertainty * 0.035f + (1.0f - track.quality) * 5.0f;
        }

        private TrackModel createFromActive(WorldView view, Contact contact) {
            TrackModel track = new TrackModel(nextId++, contact.kind(), contact.pos(), view.tick());
            track.lastActiveTick = view.tick();
            track.rangeEstimate = (float) contact.range().orElse(Geometry.distance(view.self().pos(), contact.pos()));
            track.quality = 0.60f;
            track.uncertainty = 16.0f;
            tracks.add(track);
            return track;
        }

        private TrackModel createFromPassive(WorldView view, Contact contact) {
            float initialRange = contact.confidence() >= 0.75f ? 330.0f : 125.0f;
            Vec2 estimate = clampToMap(pointOnBearing(view.self().pos(), contact.bearingDeg(), initialRange), map, 8.0f);
            TrackModel track = new TrackModel(nextId++, contact.kind(), estimate, view.tick());
            track.lastPassiveTick = view.tick();
            track.lastBearingDeg = contact.bearingDeg();
            track.lastObserverPos = view.self().pos();
            track.rangeEstimate = initialRange;
            track.quality = contact.confidence() >= 0.75f ? 0.34f : 0.25f;
            track.uncertainty = contact.confidence() >= 0.75f ? 118.0f : 72.0f;
            tracks.add(track);
            return track;
        }

        private void age(long tick) {
            Iterator<TrackModel> it = tracks.iterator();
            while (it.hasNext()) {
                TrackModel track = it.next();
                if (tick - track.lastSeenTick > 155 || track.quality < 0.045f) {
                    it.remove();
                }
            }
        }
    }

    private static final class SensorDirector {
        private ShipSpecs specs;
        private int tickHz;

        void configure(ShipSpecs specs, int tickHz) {
            this.specs = specs;
            this.tickHz = tickHz;
        }

        SensorMode choose(
                WorldView view,
                Optional<TrackModel> target,
                HybridTracker tracker,
                ShotDirector shots,
                long activeBurstUntil,
                long lastDamageTick) {
            if (view.tick() <= activeBurstUntil) {
                return SensorMode.ACTIVE;
            }
            if (view.self().ammo() <= 0) {
                return SensorMode.PASSIVE;
            }
            if (view.tick() - lastDamageTick < 15) {
                return SensorMode.ACTIVE;
            }
            if (target.isEmpty()) {
                long silence = view.tick() - tracker.lastContactTick();
                return silence > 70 && view.tick() % 34 < 3 ? SensorMode.ACTIVE : SensorMode.PASSIVE;
            }

            TrackModel t = target.get();
            long activeAge = view.tick() - t.lastActiveTick;
            float range = Geometry.distance(view.self().pos(), t.predict(view.tick(), tickHz));
            boolean gunReady = shots.canFire(view);
            boolean nearWeaponEnvelope = range > 55.0f && range < specs.maxShellRange() - 10.0f;
            boolean acousticGood = t.quality > 0.50f && t.uncertainty < 76.0f;

            if (gunReady && nearWeaponEnvelope && acousticGood && activeAge > 8) {
                return SensorMode.ACTIVE;
            }
            if (activeAge > 50 && (t.quality < 0.55f || t.uncertainty > 78.0f)) {
                return SensorMode.ACTIVE;
            }
            if (!tracker.hasFreshTrack(view.tick()) && view.tick() % 45 < 2) {
                return SensorMode.ACTIVE;
            }
            return SensorMode.PASSIVE;
        }
    }

    private final class MovementPlanner {
        private ShipSpecs specs;
        private MapInfo map;

        void configure(ShipSpecs specs, MapInfo map) {
            this.specs = specs;
            this.map = map;
        }

        HelmOrder plan(
                WorldView view,
                Optional<TrackModel> target,
                int orbitDirection,
                long evasionUntil,
                long reverseUntil,
                Vec2 lastThreatPos) {
            Vec2 self = view.self().pos();
            float desired;
            float throttle;

            if (target.isPresent()) {
                TrackModel t = target.get();
                Vec2 targetPos = t.predict(view.tick(), tickHz);
                float range = Geometry.distance(self, targetPos);
                float toward = Geometry.bearingTo(self, targetPos);
                float away = Geometry.bearingTo(targetPos, self);
                float tangent = circularBearingAround(targetPos, self, orbitDirection);
                float standOff = desiredStandOff(view);

                if (view.self().hp() <= specs.hullHp() * 0.35f) {
                    desired = blendBearing(away, tangent, 0.28f);
                    throttle = 0.88f;
                } else if (range > standOff + 85.0f) {
                    desired = blendBearing(toward, tangent, 0.30f);
                    throttle = 1.0f;
                } else if (range < standOff - 80.0f) {
                    desired = blendBearing(away, tangent, 0.32f);
                    throttle = range < 95.0f ? 0.96f : 0.78f;
                } else {
                    float crossWeight = t.uncertainty > 65.0f ? 0.76f : 0.58f;
                    desired = blendBearing(tangent, range < standOff ? away : toward, 1.0f - crossWeight);
                    throttle = t.pingerHeat > 0.45f ? 0.66f : 0.76f;
                }
            } else {
                Vec2 center = new Vec2(map.width() / 2.0f, map.height() / 2.0f);
                desired = circularBearingAround(center, self, orbitDirection);
                throttle = 0.74f;
                if (Geometry.distance(self, center) > Math.min(map.width(), map.height()) * 0.40f) {
                    desired = blendBearing(desired, Geometry.bearingTo(self, center), 0.45f);
                }
            }

            desired = avoidWalls(self, desired);

            if (lastThreatPos != null && view.tick() < evasionUntil) {
                float threatAway = Geometry.bearingTo(lastThreatPos, self);
                desired = blendBearing(threatAway, desired, 0.35f);
            }

            float rudder = Geometry.clamp(Geometry.signedBearingDelta(desired, view.self().headingDeg()) / 38.0f, -1.0f, 1.0f);
            if (view.tick() < evasionUntil) {
                rudder = Geometry.clamp(rudder + orbitDirection * 0.48f, -1.0f, 1.0f);
                throttle = view.tick() < reverseUntil ? -0.72f : (view.tick() % 18 < 6 ? 0.36f : 1.0f);
            }

            if (nearWall(self, 55.0f)) {
                throttle = Math.min(throttle, 0.52f);
            }
            return new HelmOrder(throttle, rudder);
        }

        private float desiredStandOff(WorldView view) {
            if (view.self().hp() <= specs.hullHp() * 0.35f) {
                return Math.min(278.0f, specs.maxShellRange() - 18.0f);
            }
            if (view.self().ammo() <= 5) {
                return Math.min(268.0f, specs.maxShellRange() - 22.0f);
            }
            return Math.min(238.0f, specs.maxShellRange() - 35.0f);
        }

        private float avoidWalls(Vec2 self, float desired) {
            if (!nearWall(self, 110.0f)) {
                return desired;
            }
            Vec2 center = new Vec2(map.width() / 2.0f, map.height() / 2.0f);
            float centerBearing = Geometry.bearingTo(self, center);
            float weight = nearWall(self, 55.0f) ? 0.90f : 0.64f;
            return blendBearing(desired, centerBearing, weight);
        }

        private boolean nearWall(Vec2 self, float margin) {
            return self.x() < margin
                    || self.y() < margin
                    || self.x() > map.width() - margin
                    || self.y() > map.height() - margin;
        }
    }

    private static final class ShotDirector {
        private ShipSpecs specs;
        private int tickHz;
        private long nextFireTick = 0;

        void configure(ShipSpecs specs, int tickHz) {
            this.specs = specs;
            this.tickHz = tickHz;
        }

        boolean canFire(WorldView view) {
            return view.self().ammo() > 0 && view.tick() >= nextFireTick;
        }

        boolean tryFire(WorldView view, Command command, TrackModel target, long lastDamageTick) {
            if (!canFire(view)) {
                return false;
            }

            long activeAge = view.tick() - target.lastActiveTick;
            boolean lowAmmo = view.self().ammo() <= 5;
            if (activeAge > (lowAmmo ? 5 : 10)) {
                boolean exceptionalPassive = target.quality > 0.82f && target.uncertainty < 24.0f;
                if (!exceptionalPassive) {
                    return false;
                }
            }

            float uncertaintyLimit = lowAmmo ? 32.0f : (view.tick() - lastDamageTick < 30 ? 58.0f : 45.0f);
            if (target.uncertainty > uncertaintyLimit) {
                return false;
            }
            if (target.quality < (lowAmmo ? 0.68f : 0.56f)) {
                return false;
            }

            Vec2 targetPos = target.predict(view.tick(), tickHz);
            Optional<Vec2> lead = Geometry.leadTarget(view.self().pos(), targetPos, target.velocity, specs.shellSpeed());
            if (lead.isEmpty()) {
                return false;
            }
            Vec2 aim = lead.get();
            float range = Geometry.distance(view.self().pos(), aim);
            if (range < Math.max(45.0f, specs.splashRadius() * 3.0f) || range > specs.maxShellRange() - 5.0f) {
                return false;
            }
            if (Geometry.distance(view.self().pos(), aim) < specs.splashRadius() * 2.0f) {
                return false;
            }

            command.fire(new FireCommand(Geometry.bearingTo(view.self().pos(), aim), range));
            nextFireTick = view.tick() + specs.gunCooldownTicks();
            return true;
        }

        void delayAfterCooldownError(long tick) {
            nextFireTick = Math.max(nextFireTick, tick + Math.max(3, specs.gunCooldownTicks() / 2));
        }

        void disable() {
            nextFireTick = Long.MAX_VALUE / 2;
        }
    }

    private static final class TrackModel {
        private final int id;
        private ContactKind kind;
        private Vec2 position;
        private Vec2 velocity = new Vec2(0.0f, 0.0f);
        private long lastSeenTick;
        private long lastActiveTick = -9999;
        private long lastPassiveTick = -9999;
        private float lastBearingDeg = 0.0f;
        private Vec2 lastObserverPos = null;
        private float rangeEstimate = 240.0f;
        private float quality = 0.25f;
        private float uncertainty = 120.0f;
        private float pingerHeat = 0.0f;
        private int estimatedDamage = 0;

        TrackModel(int id, ContactKind kind, Vec2 position, long tick) {
            this.id = id;
            this.kind = kind;
            this.position = position;
            this.lastSeenTick = tick;
        }

        Vec2 predict(long tick, int tickHz) {
            float dt = Math.max(0.0f, (tick - lastSeenTick) / (float) tickHz);
            return add(position, scale(velocity, dt));
        }

        boolean isStale(long tick) {
            return tick - lastSeenTick > 120 || quality < 0.07f;
        }

        void updateActive(WorldView view, Contact contact, int tickHz) {
            Vec2 predicted = predict(view.tick(), tickHz);
            float dt = Math.max(1.0f, view.tick() - lastSeenTick) / tickHz;
            Vec2 observedVelocity = scale(subtract(contact.pos(), position), 1.0f / dt);

            position = blend(predicted, contact.pos(), 0.78f);
            velocity = blend(velocity, observedVelocity, quality > 0.45f ? 0.40f : 0.24f);
            rangeEstimate = (float) contact.range().orElse(Geometry.distance(view.self().pos(), contact.pos()));
            lastSeenTick = view.tick();
            lastActiveTick = view.tick();
            if (kind == ContactKind.UNKNOWN) {
                kind = contact.kind();
            }
            quality = Math.min(1.0f, quality + 0.27f);
            uncertainty = Math.max(8.0f, uncertainty * 0.40f);
        }

        void updatePassive(WorldView view, Contact contact, int tickHz, MapInfo map) {
            Vec2 predicted = predict(view.tick(), tickHz);
            float acousticRange = contact.confidence() >= 0.75f ? 340.0f : 125.0f;
            float workingRange = Geometry.clamp(
                    rangeEstimate * 0.76f + acousticRange * 0.24f,
                    65.0f,
                    contact.confidence() >= 0.75f ? 500.0f : 220.0f);
            Vec2 bearingFix = pointOnBearing(view.self().pos(), contact.bearingDeg(), workingRange);

            Optional<Vec2> triangulated = Optional.empty();
            if (lastObserverPos != null && Geometry.distance(view.self().pos(), lastObserverPos) >= 10.0f) {
                triangulated = intersectBearingRays(lastObserverPos, lastBearingDeg, view.self().pos(), contact.bearingDeg())
                        .filter(p -> insideMap(p, map, 20.0f))
                        .filter(p -> Geometry.distance(view.self().pos(), p) <= 590.0f)
                        .filter(p -> Geometry.distance(p, predicted) <= Math.max(95.0f, uncertainty * 2.5f));
            }

            Vec2 observation = triangulated.orElse(bearingFix);
            float observationWeight = triangulated.isPresent() ? 0.48f : 0.17f;
            if (contact.confidence() >= 0.75f) {
                observationWeight += 0.07f;
            }

            float dt = Math.max(1.0f, view.tick() - lastSeenTick) / tickHz;
            Vec2 observedVelocity = scale(subtract(observation, position), 1.0f / dt);
            position = blend(predicted, observation, observationWeight);
            velocity = blend(velocity, observedVelocity, triangulated.isPresent() ? 0.23f : 0.075f);
            rangeEstimate = Geometry.distance(view.self().pos(), position);
            lastSeenTick = view.tick();
            lastPassiveTick = view.tick();
            lastObserverPos = view.self().pos();
            lastBearingDeg = contact.bearingDeg();

            if (contact.confidence() >= 0.75f) {
                pingerHeat = Math.min(1.0f, pingerHeat + 0.17f);
                quality = Math.min(0.88f, quality + (triangulated.isPresent() ? 0.12f : 0.058f));
            } else {
                quality = Math.min(0.74f, quality + (triangulated.isPresent() ? 0.075f : 0.026f));
            }
            uncertainty = Math.max(18.0f, uncertainty * (triangulated.isPresent() ? 0.70f : 0.91f));
        }
    }

    private record HelmOrder(float throttle, float rudder) {}

    private static Vec2 add(Vec2 a, Vec2 b) {
        return new Vec2(a.x() + b.x(), a.y() + b.y());
    }

    private static Vec2 subtract(Vec2 a, Vec2 b) {
        return new Vec2(a.x() - b.x(), a.y() - b.y());
    }

    private static Vec2 scale(Vec2 v, float scale) {
        return new Vec2(v.x() * scale, v.y() * scale);
    }

    private static Vec2 blend(Vec2 a, Vec2 b, float bWeight) {
        float aWeight = 1.0f - bWeight;
        return new Vec2(a.x() * aWeight + b.x() * bWeight, a.y() * aWeight + b.y() * bWeight);
    }

    private static float blendBearing(float a, float b, float bWeight) {
        return Geometry.wrapBearing(a + Geometry.signedBearingDelta(b, a) * bWeight);
    }

    private static float circularBearingAround(Vec2 center, Vec2 self, int direction) {
        float away = Geometry.bearingTo(center, self);
        return Geometry.wrapBearing(away + 90.0f * direction);
    }

    private static Vec2 pointOnBearing(Vec2 origin, float bearingDeg, float range) {
        double radians = Math.toRadians(bearingDeg);
        return new Vec2(
                origin.x() + (float) Math.sin(radians) * range,
                origin.y() - (float) Math.cos(radians) * range);
    }

    private static Optional<Vec2> intersectBearingRays(Vec2 aOrigin, float aBearing, Vec2 bOrigin, float bBearing) {
        Vec2 r = unitFromBearing(aBearing);
        Vec2 s = unitFromBearing(bBearing);
        Vec2 delta = subtract(bOrigin, aOrigin);
        float denom = cross(r, s);
        if (Math.abs(denom) < 0.08f) {
            return Optional.empty();
        }
        float t = cross(delta, s) / denom;
        float u = cross(delta, r) / denom;
        if (t < 0.0f || u < 0.0f) {
            return Optional.empty();
        }
        return Optional.of(add(aOrigin, scale(r, t)));
    }

    private static Vec2 unitFromBearing(float bearingDeg) {
        double radians = Math.toRadians(bearingDeg);
        return new Vec2((float) Math.sin(radians), (float) -Math.cos(radians));
    }

    private static float cross(Vec2 a, Vec2 b) {
        return a.x() * b.y() - a.y() * b.x();
    }

    private static Vec2 clampToMap(Vec2 pos, MapInfo map, float margin) {
        return new Vec2(
                Geometry.clamp(pos.x(), margin, map.width() - margin),
                Geometry.clamp(pos.y(), margin, map.height() - margin));
    }

    private static boolean insideMap(Vec2 pos, MapInfo map, float margin) {
        return pos.x() >= margin
                && pos.y() >= margin
                && pos.x() <= map.width() - margin
                && pos.y() <= map.height() - margin;
    }
}
