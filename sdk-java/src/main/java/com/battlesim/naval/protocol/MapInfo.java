package com.battlesim.naval.protocol;

import com.fasterxml.jackson.databind.JsonNode;

public record MapInfo(int width, int height) {
    public static MapInfo from(JsonNode node) {
        return new MapInfo(node.get("width").asInt(), node.get("height").asInt());
    }
}
