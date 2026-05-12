package com.battlesim.naval.protocol;

import com.fasterxml.jackson.databind.JsonNode;

public record SelfState(
        Vec2 pos,
        float headingDeg,
        float speed,
        int hp,
        int ammo,
        float rudder,
        float throttle) {

    public static SelfState from(JsonNode n) {
        return new SelfState(
                Vec2.from(n.get("pos")),
                (float) n.get("heading_deg").asDouble(),
                (float) n.get("speed").asDouble(),
                n.get("hp").asInt(),
                n.get("ammo").asInt(),
                (float) n.get("rudder").asDouble(),
                (float) n.get("throttle").asDouble());
    }
}
