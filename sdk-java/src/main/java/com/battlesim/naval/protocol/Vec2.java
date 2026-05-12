package com.battlesim.naval.protocol;

import com.fasterxml.jackson.databind.JsonNode;

/** 2D point or velocity. Serialized over the wire as a {@code [x, y]} JSON array. */
public record Vec2(float x, float y) {
    public static Vec2 from(JsonNode arr) {
        return new Vec2((float) arr.get(0).asDouble(), (float) arr.get(1).asDouble());
    }

    public float distanceTo(Vec2 other) {
        float dx = other.x - x;
        float dy = other.y - y;
        return (float) Math.hypot(dx, dy);
    }
}
