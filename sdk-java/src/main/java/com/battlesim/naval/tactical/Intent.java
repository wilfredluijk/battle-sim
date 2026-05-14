package com.battlesim.naval.tactical;

import com.battlesim.naval.Command;
import com.battlesim.naval.protocol.Vec2;

/**
 * High-level tactical intent returned from {@link TacticalBot#decide}.
 *
 * <p>Construct via the static helpers. The orchestrator translates each variant
 * into a concrete {@link Command} via the L2 subsystems.
 */
public sealed interface Intent
        permits Intent.Engage, Intent.Patrol, Intent.RetreatTo, Intent.Hold, Intent.Custom {

    record Engage(Track target) implements Intent {}

    /** Patrol the corners of an axis-aligned rectangle {@code (x1, y1, x2, y2)}. */
    record Patrol(float x1, float y1, float x2, float y2) implements Intent {}

    record RetreatTo(Vec2 point) implements Intent {}

    record Hold() implements Intent {}

    /** Escape hatch: hand the orchestrator a raw {@link Command} for one tick. */
    record Custom(Command command) implements Intent {}

    static Intent engage(Track target) { return new Engage(target); }

    static Intent patrol(float x1, float y1, float x2, float y2) {
        return new Patrol(x1, y1, x2, y2);
    }

    static Intent retreatTo(Vec2 point) { return new RetreatTo(point); }

    static Intent hold() { return new Hold(); }

    static Intent custom(Command cmd) { return new Custom(cmd); }
}
