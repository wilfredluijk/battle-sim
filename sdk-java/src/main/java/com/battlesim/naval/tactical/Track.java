package com.battlesim.naval.tactical;

import com.battlesim.naval.protocol.ContactKind;
import com.battlesim.naval.protocol.Vec2;

/**
 * A persistent target estimate maintained by {@link Tracker}.
 *
 * <p>{@code trackId} is stable across ticks (unlike {@code Contact.id} from the
 * wire). {@code pos} is the tracker's best estimate for the <em>current</em> tick
 * — either the most recent observation, or a dead-reckoned prediction when the
 * target was only seen passively (or not at all this tick). {@code observedPos}
 * is the raw last-active measurement; compare the two to see how far the
 * estimate has drifted from the last fix. {@code vel} is in units per second.
 *
 * <p>{@code source} is one of {@code "active"}, {@code "passive"}, or
 * {@code "dead_reckoned"}.
 */
public final class Track {
    private final int trackId;
    private ContactKind kind;
    private Vec2 pos;
    private Vec2 observedPos;
    private Vec2 vel;
    private long lastSeenTick;
    private final long firstSeenTick;
    private long lastActiveTick;
    private float confidence;
    private String source;

    Track(
            int trackId,
            ContactKind kind,
            Vec2 pos,
            Vec2 observedPos,
            Vec2 vel,
            long lastSeenTick,
            long firstSeenTick,
            long lastActiveTick,
            float confidence,
            String source) {
        this.trackId = trackId;
        this.kind = kind;
        this.pos = pos;
        this.observedPos = observedPos;
        this.vel = vel;
        this.lastSeenTick = lastSeenTick;
        this.firstSeenTick = firstSeenTick;
        this.lastActiveTick = lastActiveTick;
        this.confidence = confidence;
        this.source = source;
    }

    public int trackId() { return trackId; }
    public ContactKind kind() { return kind; }
    public Vec2 pos() { return pos; }
    public Vec2 observedPos() { return observedPos; }
    public Vec2 vel() { return vel; }
    public long lastSeenTick() { return lastSeenTick; }
    public long firstSeenTick() { return firstSeenTick; }
    public long lastActiveTick() { return lastActiveTick; }
    public float confidence() { return confidence; }
    public String source() { return source; }

    // Package-private mutators used by Tracker.
    void setKind(ContactKind k) { this.kind = k; }
    void setPos(Vec2 p) { this.pos = p; }
    void setObservedPos(Vec2 p) { this.observedPos = p; }
    void setVel(Vec2 v) { this.vel = v; }
    void setLastSeenTick(long t) { this.lastSeenTick = t; }
    void setLastActiveTick(long t) { this.lastActiveTick = t; }
    void setConfidence(float c) { this.confidence = c; }
    void setSource(String s) { this.source = s; }
}
