package com.battlesim.naval.protocol;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.Optional;

public record GameOver(Optional<String> winner, long finalTick, String replayId) {
    public static GameOver from(JsonNode n) {
        JsonNode w = n.get("winner");
        Optional<String> winner = (w == null || w.isNull())
                ? Optional.empty()
                : Optional.of(w.asText());
        return new GameOver(winner, n.get("final_tick").asLong(), n.get("replay_id").asText());
    }
}
