package com.battlesim.naval.protocol;

import com.fasterxml.jackson.databind.JsonNode;

public record Welcome(
        String botId,
        String shipId,
        MapInfo map,
        int tickHz,
        ShipSpecs shipSpecs) {

    public static Welcome from(JsonNode n) {
        return new Welcome(
                n.get("bot_id").asText(),
                n.get("ship_id").asText(),
                MapInfo.from(n.get("map")),
                n.get("tick_hz").asInt(),
                ShipSpecs.from(n.get("ship_specs")));
    }
}
