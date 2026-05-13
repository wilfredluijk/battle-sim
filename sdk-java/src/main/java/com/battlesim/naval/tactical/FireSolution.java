package com.battlesim.naval.tactical;

import com.battlesim.naval.protocol.Vec2;

/** A vetted firing solution returned by {@link Gunner#solve}. */
public record FireSolution(
        float bearingDeg,
        float range,
        Vec2 aimPos,
        int targetId) {}
