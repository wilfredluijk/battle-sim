package com.battlesim.naval.protocol;

import com.fasterxml.jackson.databind.JsonNode;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.Optional;

/** Bot-side view of a single {@code tick} message. */
public record WorldView(
        long tick,
        long deadlineMs,
        SelfState self,
        List<Contact> contacts,
        List<TickEvent> events) {

    public static WorldView from(JsonNode n) {
        List<Contact> contacts = new ArrayList<>();
        JsonNode c = n.get("contacts");
        if (c != null) {
            for (JsonNode item : c) contacts.add(Contact.from(item));
        }
        List<TickEvent> events = new ArrayList<>();
        JsonNode e = n.get("events");
        if (e != null) {
            for (JsonNode item : e) events.add(TickEvent.from(item));
        }
        return new WorldView(
                n.get("tick").asLong(),
                n.get("deadline_ms").asLong(),
                SelfState.from(n.get("self")),
                List.copyOf(contacts),
                List.copyOf(events));
    }

    /** Nearest contact with a known range, or empty if none. */
    public Optional<Contact> nearestContact() {
        return contacts.stream()
                .filter(ct -> ct.range().isPresent())
                .min(Comparator.comparingDouble(ct -> ct.range().getAsDouble()));
    }
}
