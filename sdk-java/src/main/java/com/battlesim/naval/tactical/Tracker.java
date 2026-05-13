package com.battlesim.naval.tactical;

import com.battlesim.naval.Geometry;
import com.battlesim.naval.protocol.Contact;
import com.battlesim.naval.protocol.ContactKind;
import com.battlesim.naval.protocol.ShipSpecs;
import com.battlesim.naval.protocol.Vec2;
import com.battlesim.naval.protocol.WorldView;

import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashSet;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * Multi-target tracker.
 *
 * <p>Stitches per-tick {@code Contact} reports (which carry an unstable per-tick
 * id) into persistent {@link Track} objects with smoothed position and velocity.
 * Mirrors the Python SDK's {@code naval_sdk.tactical.Tracker}.
 *
 * <p>Determinism note: this code runs in the bot process, not the server. It is
 * free to use ordering-sensitive data structures; replay determinism is the
 * server's concern.
 */
public class Tracker {

    public static final class Config {
        public float activeGate = 60.0f;
        public float passiveBearingGateDeg = 20.0f;
        public float velocityAlpha = 0.3f;
        public int velocityWindowTicks = 10;
        public int stalenessTicks = 40;
    }

    private final ShipSpecs specs;
    private final double dt;
    private final Config cfg;
    private final Map<Integer, Track> tracks = new LinkedHashMap<>();
    private final Map<Integer, List<Observation>> history = new LinkedHashMap<>();
    private int nextId = 1;

    private record Observation(long tick, Vec2 pos) {}

    public Tracker(ShipSpecs specs, int tickHz) {
        this(specs, tickHz, new Config());
    }

    public Tracker(ShipSpecs specs, int tickHz, Config config) {
        this.specs = specs;
        this.dt = 1.0 / (double) tickHz;
        this.cfg = config;
    }

    /** Fold {@code view.contacts()} into the track set and return current tracks. */
    public List<Track> update(WorldView view) {
        long tick = view.tick();

        List<Contact> activeContacts = new ArrayList<>();
        List<Contact> passiveContacts = new ArrayList<>();
        for (Contact c : view.contacts()) {
            if (c.range().isPresent()) {
                activeContacts.add(c);
            } else {
                passiveContacts.add(c);
            }
        }

        Set<Integer> matched = new HashSet<>();

        // 1. Active association — greedy nearest predicted position.
        for (Contact contact : activeContacts) {
            Integer bestId = null;
            float bestDistance = cfg.activeGate;
            for (Map.Entry<Integer, Track> e : tracks.entrySet()) {
                if (matched.contains(e.getKey())) continue;
                Vec2 pred = predict(e.getValue(), tick);
                float dx = pred.x() - contact.pos().x();
                float dy = pred.y() - contact.pos().y();
                float d = (float) Math.hypot(dx, dy);
                if (d < bestDistance) {
                    bestDistance = d;
                    bestId = e.getKey();
                }
            }
            if (bestId != null) {
                foldActive(bestId, contact, tick);
                matched.add(bestId);
            } else {
                int newId = spawn(contact, tick);
                matched.add(newId);
            }
        }

        // 2. Passive association — bearing-only, never spawns.
        Vec2 mePos = view.self().pos();
        for (Contact contact : passiveContacts) {
            Integer bestId = null;
            float bestBearing = cfg.passiveBearingGateDeg;
            for (Map.Entry<Integer, Track> e : tracks.entrySet()) {
                if (matched.contains(e.getKey())) continue;
                Vec2 pred = predict(e.getValue(), tick);
                float predBearing = Geometry.bearingTo(mePos, pred);
                float delta = Math.abs(Geometry.signedBearingDelta(contact.bearingDeg(), predBearing));
                if (delta < bestBearing) {
                    bestBearing = delta;
                    bestId = e.getKey();
                }
            }
            if (bestId != null) {
                foldPassive(bestId, contact, tick);
                matched.add(bestId);
            }
        }

        // 3. Refresh ``pos`` for tracks that didn't get an active fold this tick.
        for (Track t : tracks.values()) {
            if (t.lastActiveTick() != tick) {
                t.setPos(predict(t, tick));
                if (t.lastSeenTick() != tick) {
                    t.setSource("dead_reckoned");
                }
            }
        }

        // 4. Stale GC.
        Iterator<Map.Entry<Integer, Track>> it = tracks.entrySet().iterator();
        while (it.hasNext()) {
            Map.Entry<Integer, Track> e = it.next();
            if (tick - e.getValue().lastSeenTick() > cfg.stalenessTicks) {
                history.remove(e.getKey());
                it.remove();
            }
        }

        return tracks();
    }

    /** Current tracks, sorted by {@code trackId} for stable iteration. */
    public List<Track> tracks() {
        List<Track> out = new ArrayList<>(tracks.values());
        out.sort(Comparator.comparingInt(Track::trackId));
        return out;
    }

    public Track get(int trackId) {
        return tracks.get(trackId);
    }

    // ---- internals -----------------------------------------------------

    private Vec2 predict(Track track, long tick) {
        long ticksElapsed = tick - track.lastActiveTick();
        if (ticksElapsed <= 0) return track.observedPos();
        double t = ticksElapsed * dt;
        return new Vec2(
                (float) (track.observedPos().x() + track.vel().x() * t),
                (float) (track.observedPos().y() + track.vel().y() * t));
    }

    private void foldActive(int trackId, Contact contact, long tick) {
        Track track = tracks.get(trackId);
        List<Observation> hist = history.get(trackId);
        hist.add(new Observation(tick, contact.pos()));
        while (hist.size() > cfg.velocityWindowTicks) {
            hist.remove(0);
        }

        if (hist.size() >= 2) {
            Observation oldest = hist.get(0);
            double dtTotal = (tick - oldest.tick()) * dt;
            if (dtTotal > 0) {
                float instVx = (float) ((contact.pos().x() - oldest.pos().x()) / dtTotal);
                float instVy = (float) ((contact.pos().y() - oldest.pos().y()) / dtTotal);
                Vec2 prev = track.vel();
                if (prev.x() == 0.0f && prev.y() == 0.0f) {
                    track.setVel(new Vec2(instVx, instVy));
                } else {
                    float a = cfg.velocityAlpha;
                    track.setVel(new Vec2(
                            a * instVx + (1.0f - a) * prev.x(),
                            a * instVy + (1.0f - a) * prev.y()));
                }
            }
        }

        track.setObservedPos(contact.pos());
        track.setPos(contact.pos());
        track.setLastSeenTick(tick);
        track.setLastActiveTick(tick);
        track.setConfidence(contact.confidence());
        track.setSource("active");
        if (track.kind() == ContactKind.UNKNOWN) {
            track.setKind(contact.kind());
        }
    }

    private void foldPassive(int trackId, Contact contact, long tick) {
        Track track = tracks.get(trackId);
        track.setLastSeenTick(tick);
        track.setConfidence(contact.confidence());
        track.setSource("passive");
        // Bearing-only: do not modify observedPos, pos, or vel.
    }

    private int spawn(Contact contact, long tick) {
        int id = nextId++;
        tracks.put(id, new Track(
                id,
                contact.kind(),
                contact.pos(),
                contact.pos(),
                new Vec2(0.0f, 0.0f),
                tick,
                tick,
                tick,
                contact.confidence(),
                "active"));
        List<Observation> hist = new ArrayList<>();
        hist.add(new Observation(tick, contact.pos()));
        history.put(id, hist);
        return id;
    }
}
