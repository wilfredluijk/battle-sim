package com.battlesim.naval.tactical;

import com.battlesim.naval.Geometry;
import com.battlesim.naval.protocol.Vec2;

import java.util.Iterator;
import java.util.List;
import java.util.Optional;

/** Iterable view of threat tracks with cheap tactical queries. */
public final class ThreatList implements Iterable<Track> {
    private final List<Track> tracks;
    private final Vec2 mePos;

    public ThreatList(List<Track> tracks, Vec2 mePos) {
        this.tracks = tracks;
        this.mePos = mePos;
    }

    public List<Track> tracks() { return tracks; }

    public int size() { return tracks.size(); }

    public boolean isEmpty() { return tracks.isEmpty(); }

    @Override
    public Iterator<Track> iterator() { return tracks.iterator(); }

    public Optional<Track> nearest() {
        if (tracks.isEmpty()) return Optional.empty();
        Track best = tracks.get(0);
        float bestD = Geometry.distance(mePos, best.pos());
        for (int i = 1; i < tracks.size(); i++) {
            float d = Geometry.distance(mePos, tracks.get(i).pos());
            if (d < bestD) {
                bestD = d;
                best = tracks.get(i);
            }
        }
        return Optional.of(best);
    }

    public Optional<Track> farthest() {
        if (tracks.isEmpty()) return Optional.empty();
        Track best = tracks.get(0);
        float bestD = Geometry.distance(mePos, best.pos());
        for (int i = 1; i < tracks.size(); i++) {
            float d = Geometry.distance(mePos, tracks.get(i).pos());
            if (d > bestD) {
                bestD = d;
                best = tracks.get(i);
            }
        }
        return Optional.of(best);
    }

    public Optional<Track> byId(int trackId) {
        for (Track t : tracks) {
            if (t.trackId() == trackId) return Optional.of(t);
        }
        return Optional.empty();
    }
}
