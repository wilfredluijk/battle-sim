package com.battlesim.naval.protocol;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.OptionalDouble;

public record Contact(
        String id,
        ContactKind kind,
        Vec2 pos,
        float bearingDeg,
        OptionalDouble range,
        float confidence) {

    public static Contact from(JsonNode n) {
        JsonNode rangeNode = n.get("range");
        OptionalDouble range = (rangeNode == null || rangeNode.isNull())
                ? OptionalDouble.empty()
                : OptionalDouble.of(rangeNode.asDouble());
        JsonNode kindNode = n.get("kind");
        ContactKind kind = (kindNode == null || kindNode.isNull())
                ? ContactKind.UNKNOWN
                : ContactKind.fromWire(kindNode.asText());
        JsonNode conf = n.get("confidence");
        return new Contact(
                n.get("id").asText(),
                kind,
                Vec2.from(n.get("pos")),
                (float) n.get("bearing_deg").asDouble(),
                range,
                conf == null ? 0.0f : (float) conf.asDouble());
    }
}
