package com.battlesim.naval.tactical;

import com.battlesim.naval.Geometry;
import com.battlesim.naval.protocol.Contact;
import com.battlesim.naval.protocol.ContactKind;
import com.battlesim.naval.protocol.MapInfo;
import com.battlesim.naval.protocol.SelfState;
import com.battlesim.naval.protocol.ShipSpecs;
import com.battlesim.naval.protocol.TickEvent;
import com.battlesim.naval.protocol.Vec2;
import com.battlesim.naval.protocol.Welcome;
import com.battlesim.naval.protocol.WorldView;

import java.util.Collections;
import java.util.List;
import java.util.OptionalDouble;

/** Builders shared by the tactical-package tests. */
final class TacticalTestSupport {
    private TacticalTestSupport() {}

    static final ShipSpecs SPECS = new ShipSpecs(
            6.0f, 2.0f, 1.5f, 15.0f, 100, 20, 15, 2.0f,
            50.0f, 300.0f, 15.0f, 25);

    static final Welcome WELCOME = new Welcome(
            "b1", "s1", new MapInfo(800, 800), 10, SPECS);

    static SelfState me() {
        return me(new Vec2(0.0f, 0.0f), 0.0f, 100, 20);
    }

    static SelfState me(Vec2 pos, float headingDeg, int hp, int ammo) {
        return new SelfState(pos, headingDeg, 0.0f, hp, ammo, 0.0f, 0.0f);
    }

    static WorldView view(long tick, SelfState self) {
        return new WorldView(tick, 80L, self, List.of(), List.of());
    }

    static WorldView view(long tick, SelfState self, List<Contact> contacts) {
        return new WorldView(tick, 80L, self, contacts, List.of());
    }

    static WorldView viewWithEvents(long tick, SelfState self, List<TickEvent> events) {
        return new WorldView(tick, 80L, self, List.of(), events);
    }

    static Contact activeContact(Vec2 pos, Vec2 mePos) {
        float dx = pos.x() - mePos.x();
        float dy = pos.y() - mePos.y();
        float range = (float) Math.hypot(dx, dy);
        return new Contact(
                "x",
                ContactKind.SHIP,
                pos,
                Geometry.bearingTo(mePos, pos),
                OptionalDouble.of(range),
                1.0f);
    }

    static Contact passiveContact(float bearingDeg) {
        return new Contact("p", ContactKind.SHIP, new Vec2(0, 0),
                bearingDeg, OptionalDouble.empty(), 0.5f);
    }
}
