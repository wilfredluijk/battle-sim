package com.battlesim.naval.protocol;

import com.fasterxml.jackson.databind.JsonNode;

public record GameStart(long tick, Vec2 startingPosition, float startingHeadingDeg) {
    public static GameStart from(JsonNode n) {
        return new GameStart(
                n.get("tick").asLong(),
                Vec2.from(n.get("starting_position")),
                (float) n.get("starting_heading_deg").asDouble());
    }
}
