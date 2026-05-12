package com.battlesim.examples;

import com.battlesim.naval.Bot;
import com.battlesim.naval.BotRunner;
import com.battlesim.naval.Command;
import com.battlesim.naval.Geometry;
import com.battlesim.naval.protocol.Contact;
import com.battlesim.naval.protocol.SensorMode;
import com.battlesim.naval.protocol.Vec2;
import com.battlesim.naval.protocol.WorldView;
import java.util.ArrayList;
import java.util.List;

/**
 * Demonstrates data association across ticks.
 *
 * <p>The wire-level {@code contact.id} is rebuilt every tick, so a bot that
 * wants to lead a moving target has to stitch successive pings into a single
 * track itself. This bot keeps a small list of internal tracks, associates
 * each incoming contact to the closest predicted track position, and uses
 * the smoothed velocity to lead its shots.
 *
 * <p>It also drives in a lazy circle so the demo is visually obvious: even
 * though the platform is constantly turning, the tracker still produces a
 * usable firing solution.
 */
public final class TrackingBot extends Bot {

    /** One stitched track. Reset when a contact goes missing too long. */
    private static final class Track {
        Vec2 pos;
        Vec2 vel = new Vec2(0f, 0f);
        long lastSeenTick;
        int hits;

        Track(Vec2 pos, long tick) {
            this.pos = pos;
            this.lastSeenTick = tick;
            this.hits = 1;
        }
    }

    /** Max distance (units) between a prediction and a new ping to call it the same track. */
    private static final float ASSOCIATION_GATE = 40f;
    /** Drop a track that has not been pinged for this many ticks. */
    private static final int STALE_AFTER_TICKS = 30;
    /** EMA weight applied to a fresh velocity sample. */
    private static final float VELOCITY_ALPHA = 0.4f;

    private final List<Track> tracks = new ArrayList<>();
    private long lastTickSeen = -1L;

    @Override
    public Command onTick(WorldView view) {
        updateTracks(view);

        Command cmd = new Command()
                .throttle(0.6f)
                .rudder(0.3f)
                .sensorMode(SensorMode.ACTIVE);

        Track best = pickBestTrack(view.self().pos());
        if (best != null && best.hits >= 2 && view.self().ammo() > 0) {
            float shellSpeed = welcome().shipSpecs().shellSpeed();
            float maxRange = welcome().shipSpecs().maxShellRange();
            if (Geometry.distance(view.self().pos(), best.pos) < maxRange) {
                cmd.fireAt(view.self().pos(), best.pos, best.vel, shellSpeed);
            }
        }
        return cmd;
    }

    private void updateTracks(WorldView view) {
        long tick = view.tick();
        float dt = (lastTickSeen < 0) ? 0f : (tick - lastTickSeen) / (float) welcome().tickHz();
        lastTickSeen = tick;

        // Predict each existing track forward to the current tick.
        for (Track t : tracks) {
            t.pos = new Vec2(t.pos.x() + t.vel.x() * dt, t.pos.y() + t.vel.y() * dt);
        }

        // Greedy nearest-neighbour association.
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
                t.hits++;
                usedTracks[bestIdx] = true;
            } else {
                tracks.add(new Track(c.pos(), tick));
            }
        }

        // Drop stale tracks.
        tracks.removeIf(t -> tick - t.lastSeenTick > STALE_AFTER_TICKS);
    }

    private Track pickBestTrack(Vec2 self) {
        Track best = null;
        float bestScore = Float.POSITIVE_INFINITY;
        for (Track t : tracks) {
            // Prefer close, well-confirmed tracks.
            float score = Geometry.distance(self, t.pos) - 5f * Math.min(t.hits, 10);
            if (score < bestScore) {
                bestScore = score;
                best = t;
            }
        }
        return best;
    }

    public static void main(String[] args) {
        String host = args.length > 0 ? args[0] : "localhost";
        int port    = args.length > 1 ? Integer.parseInt(args[1]) : 7878;
        String name = args.length > 2 ? args[2] : "tracker";
        BotRunner.run(new TrackingBot(), host, port, name);
    }
}
