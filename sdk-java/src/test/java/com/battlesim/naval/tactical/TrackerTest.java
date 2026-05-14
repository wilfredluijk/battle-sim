package com.battlesim.naval.tactical;

import static com.battlesim.naval.tactical.TacticalTestSupport.*;
import static org.junit.jupiter.api.Assertions.*;

import com.battlesim.naval.protocol.Contact;
import com.battlesim.naval.protocol.SelfState;
import com.battlesim.naval.protocol.Vec2;
import com.battlesim.naval.protocol.WorldView;

import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Random;
import java.util.Set;
import org.junit.jupiter.api.Test;

class TrackerTest {

    @Test
    void associatesMovingTargetWithNoise() {
        Tracker tracker = new Tracker(SPECS, 10);
        Random rng = new Random(42L);

        Vec2 mePos = new Vec2(0, 0);
        Vec2 basePos = new Vec2(100, 100);
        Vec2 vel = new Vec2(5, 0);

        Set<Integer> trackIds = new HashSet<>();
        for (int tick = 0; tick < 30; tick++) {
            float tx = basePos.x() + vel.x() * tick * 0.1f;
            float ty = basePos.y() + vel.y() * tick * 0.1f;
            Vec2 noisy = new Vec2(
                    tx + (float) (rng.nextDouble() * 4.0 - 2.0),
                    ty + (float) (rng.nextDouble() * 4.0 - 2.0));
            SelfState me = me(mePos, 0.0f, 100, 20);
            WorldView v = view(tick, me, List.of(activeContact(noisy, mePos)));
            List<Track> tracks = tracker.update(v);
            assertEquals(1, tracks.size(), "tick=" + tick);
            trackIds.add(tracks.get(0).trackId());
        }
        assertEquals(1, trackIds.size(), "track id should be stable");

        Track final_ = tracker.tracks().get(0);
        assertTrue(Math.abs(final_.vel().x() - 5.0f) < 2.0f, "vx=" + final_.vel().x());
        assertTrue(Math.abs(final_.vel().y()) < 2.0f, "vy=" + final_.vel().y());
    }

    @Test
    void foldsPassiveIntoExistingActiveTrack() {
        Tracker tracker = new Tracker(SPECS, 10);
        Vec2 mePos = new Vec2(0, 0);
        SelfState me = me(mePos, 0.0f, 100, 20);

        tracker.update(view(0, me, List.of(activeContact(new Vec2(100, 0), mePos))));
        tracker.update(view(1, me, List.of(activeContact(new Vec2(105, 0), mePos))));
        assertEquals(1, tracker.tracks().size());
        int trackId = tracker.tracks().get(0).trackId();
        long lastActive = tracker.tracks().get(0).lastActiveTick();

        // Passive bearing roughly east (90°).
        tracker.update(view(2, me, List.of(passiveContact(92.0f))));
        assertEquals(1, tracker.tracks().size());
        Track t = tracker.tracks().get(0);
        assertEquals(trackId, t.trackId());
        assertEquals(2L, t.lastSeenTick());
        assertEquals(lastActive, t.lastActiveTick());
        assertEquals("passive", t.source());
    }

    @Test
    void stalesUnseenTracks() {
        Tracker.Config cfg = new Tracker.Config();
        cfg.stalenessTicks = 5;
        Tracker tracker = new Tracker(SPECS, 10, cfg);
        Vec2 mePos = new Vec2(0, 0);
        SelfState me = me(mePos, 0.0f, 100, 20);

        tracker.update(view(0, me, List.of(activeContact(new Vec2(100, 0), mePos))));
        assertEquals(1, tracker.tracks().size());
        for (int t = 1; t < 7; t++) {
            tracker.update(view(t, me));
        }
        assertTrue(tracker.tracks().isEmpty());
    }

    @Test
    void doesNotSpawnFromPassiveOnly() {
        Tracker tracker = new Tracker(SPECS, 10);
        Vec2 mePos = new Vec2(0, 0);
        SelfState me = me(mePos, 0.0f, 100, 20);
        tracker.update(view(0, me, List.of(passiveContact(90.0f))));
        assertTrue(tracker.tracks().isEmpty());
    }

    @Test
    void spawnsSeparateTracksForDistantContacts() {
        Tracker tracker = new Tracker(SPECS, 10);
        Vec2 mePos = new Vec2(0, 0);
        SelfState me = me(mePos, 0.0f, 100, 20);
        List<Contact> contacts = new ArrayList<>();
        contacts.add(activeContact(new Vec2(100, 0), mePos));
        contacts.add(activeContact(new Vec2(-100, 0), mePos));
        List<Track> tracks = tracker.update(view(0, me, contacts));
        assertEquals(2, tracks.size());
        assertEquals(1, tracks.get(0).trackId());
        assertEquals(2, tracks.get(1).trackId());
    }
}
