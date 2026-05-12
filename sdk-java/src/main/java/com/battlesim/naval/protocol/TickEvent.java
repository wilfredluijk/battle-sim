package com.battlesim.naval.protocol;

import com.fasterxml.jackson.databind.JsonNode;

/**
 * One event in a {@code tick} message that the bot can perceive.
 * Forward-compatible: unknown event types parse into {@link Unknown}.
 */
public sealed interface TickEvent permits TickEvent.Hit, TickEvent.ShellSplash, TickEvent.Unknown {

    record Hit(int amount) implements TickEvent {}

    record ShellSplash(Vec2 pos) implements TickEvent {}

    record Unknown(String type, JsonNode raw) implements TickEvent {}

    static TickEvent from(JsonNode n) {
        String type = n.get("type").asText();
        return switch (type) {
            case "hit" -> new Hit(n.get("amount").asInt());
            case "shell_splash" -> new ShellSplash(Vec2.from(n.get("pos")));
            default -> new Unknown(type, n);
        };
    }
}
