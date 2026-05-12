package com.battlesim.examples;

import com.battlesim.naval.Bot;
import com.battlesim.naval.BotRunner;
import com.battlesim.naval.Command;
import com.battlesim.naval.Geometry;
import com.battlesim.naval.protocol.Contact;
import com.battlesim.naval.protocol.ContactKind;
import com.battlesim.naval.protocol.SensorMode;
import com.battlesim.naval.protocol.Vec2;
import com.battlesim.naval.protocol.Welcome;
import com.battlesim.naval.protocol.WorldView;
import java.util.Comparator;
import java.util.Optional;

/**
 * Demonstrates data association across pings. Active contacts provide range, so
 * the bot updates position and velocity from them. Passive contacts only provide
 * bearing, so the bot keeps the old range estimate and nudges the predicted
 * track onto the newest line of bearing until the next active ping.
 */
public final class TrackingCircleBot extends Bot {
    private Track track;
    private int tickHz = 10;
    private int cooldownTicks = 15;
    private float shellSpeed = 50.0f;
    private float maxRange = 300.0f;
    private long nextFireTick = 0;



    @Override
    public void onWelcome(Welcome welcome) {
        tickHz = welcome.tickHz();
        cooldownTicks = welcome.shipSpecs().gunCooldownTicks();
        shellSpeed = welcome.shipSpecs().shellSpeed();
        maxRange = welcome.shipSpecs().maxShellRange();
    }

    @Override
    public Command onTick(WorldView view) {
        SensorMode mode = shouldPing(view) ? SensorMode.ACTIVE : SensorMode.PASSIVE;
        updateTrack(view);

        Command command = new Command()
                .throttle(0.72f)
                .rudder(0.36f)
                .sensorMode(mode);

        if (shouldFire(view)) {
            Vec2 predicted = track.predict(view.tick(), tickHz);
            command.fireAt(view.self().pos(), predicted, track.velocity, shellSpeed);
            nextFireTick = view.tick() + cooldownTicks;
        }

        return command;
    }

    @Override
    public void onError(String code, String message) {
        if ("cooldown_active".equals(code)) {
            nextFireTick = lastTick() + Math.max(3, cooldownTicks / 2);
        } else {
            super.onError(code, message);
        }
    }

    private boolean shouldPing(WorldView view) {
        if (track == null) return true;
        long age = view.tick() - track.lastActiveTick;
        return age > 24 || view.tick() % 40 < 3;
    }

    private void updateTrack(WorldView view) {
        Optional<Contact> active = view.contacts().stream()
                .filter(this::isLikelyShip)
                .filter(c -> c.range().isPresent())
                .min(Comparator.comparingDouble(c -> scoreActiveContact(view, c)));

        if (active.isPresent()) {
            updateFromActive(view, active.get());
            return;
        }

        Optional<Contact> passive = view.contacts().stream()
                .filter(this::isLikelyShip)
                .filter(c -> c.range().isEmpty())
                .min(Comparator.comparingDouble(c -> scorePassiveContact(view, c)));

        passive.ifPresent(c -> updateFromPassive(view, c));
    }

    private boolean isLikelyShip(Contact contact) {
        return contact.kind() == ContactKind.SHIP || contact.kind() == ContactKind.UNKNOWN;
    }

    private double scoreActiveContact(WorldView view, Contact contact) {
        if (track == null) return contact.range().orElse(9999.0);
        return Geometry.distance(track.predict(view.tick(), tickHz), contact.pos());
    }

    private double scorePassiveContact(WorldView view, Contact contact) {
        if (track == null) return 0.0;
        float predictedBearing = Geometry.bearingTo(view.self().pos(), track.predict(view.tick(), tickHz));
        return Math.abs(angleDelta(predictedBearing, contact.bearingDeg()));
    }

    private void updateFromActive(WorldView view, Contact contact) {
        if (track == null) {
            track = new Track(contact.pos(), new Vec2(0.0f, 0.0f), view.tick(), view.tick());
            return;
        }

        float dt = Math.max(1.0f, view.tick() - track.lastSeenTick) / tickHz;
        Vec2 observedVelocity = new Vec2(
                (contact.pos().x() - track.position.x()) / dt,
                (contact.pos().y() - track.position.y()) / dt);
        track.position = blend(track.predict(view.tick(), tickHz), contact.pos(), 0.70f);
        track.velocity = blend(track.velocity, observedVelocity, 0.45f);
        track.lastSeenTick = view.tick();
        track.lastActiveTick = view.tick();
        track.quality = Math.min(1.0f, track.quality + 0.22f);
    }

    private void updateFromPassive(WorldView view, Contact contact) {
        if (track == null) {
            track = new Track(pointOnBearing(view.self().pos(), contact.bearingDeg(), 260.0f),
                    new Vec2(0.0f, 0.0f), view.tick(), -999);
            track.quality = 0.22f;
            return;
        }

        Vec2 predicted = track.predict(view.tick(), tickHz);
        float estimatedRange = Math.max(80.0f, Geometry.distance(view.self().pos(), predicted));
        Vec2 bearingOnlyFix = pointOnBearing(view.self().pos(), contact.bearingDeg(), estimatedRange);
        track.position = blend(predicted, bearingOnlyFix, 0.25f);
        track.lastSeenTick = view.tick();
        track.quality = Math.max(0.15f, track.quality - 0.02f);
    }

    private boolean shouldFire(WorldView view) {
        if (track == null || view.self().ammo() <= 0 || view.tick() < nextFireTick) return false;
        if (view.tick() - track.lastActiveTick > 16 || track.quality < 0.45f) return false;

        Vec2 predicted = track.predict(view.tick(), tickHz);
        float range = Geometry.distance(view.self().pos(), predicted);
        return range > 35.0f && range <= maxRange;
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

    private static float angleDelta(float from, float to) {
        return ((to - from + 540.0f) % 360.0f) - 180.0f;
    }

    private static final class Track {
        private Vec2 position;
        private Vec2 velocity;
        private long lastSeenTick;
        private long lastActiveTick;
        private float quality = 0.45f;

        private Track(Vec2 position, Vec2 velocity, long lastSeenTick, long lastActiveTick) {
            this.position = position;
            this.velocity = velocity;
            this.lastSeenTick = lastSeenTick;
            this.lastActiveTick = lastActiveTick;
        }

        private Vec2 predict(long tick, int tickHz) {
            float dt = (tick - lastSeenTick) / (float) tickHz;
            return new Vec2(position.x() + velocity.x() * dt, position.y() + velocity.y() * dt);
        }
    }

    public static void main(String[] args) {
        BotArgs parsed = BotArgs.parse(args, "tracking-circle");
        BotRunner.run(new TrackingCircleBot(), parsed.host(), parsed.port(), parsed.name());
    }
}
