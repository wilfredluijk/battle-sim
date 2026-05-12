package com.battlesim.naval;

import static org.junit.jupiter.api.Assertions.*;

import com.battlesim.naval.protocol.*;
import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import org.junit.jupiter.api.Test;

class ProtocolTest {

    private static final ObjectMapper MAPPER = new ObjectMapper();
    private static final float EPS = 1e-3f;

    private static final String WELCOME = """
            {
              "type": "welcome",
              "bot_id": "b_1",
              "ship_id": "s_1",
              "map": { "width": 1000, "height": 1000 },
              "tick_hz": 10,
              "ship_specs": {
                "max_forward_speed": 6.0,
                "max_reverse_speed": 2.0,
                "acceleration": 1.5,
                "turn_rate_deg_per_s": 15.0,
                "hull_hp": 100,
                "max_ammo": 20,
                "gun_cooldown_ticks": 15,
                "hit_radius": 8.0,
                "shell_speed": 50.0,
                "max_shell_range": 300.0,
                "splash_radius": 15.0,
                "max_splash_damage": 25
              }
            }
            """;

    private static final String TICK = """
            {
              "type": "tick",
              "tick": 142,
              "deadline_ms": 80,
              "self": {
                "pos": [200.0, 500.0],
                "heading_deg": 90.0,
                "speed": 4.1,
                "hp": 78,
                "ammo": 14,
                "rudder": -0.3,
                "throttle": 0.8
              },
              "contacts": [
                { "id": "c_a1", "kind": "ship", "pos": [450.0, 510.0],
                  "bearing_deg": 88.0, "range": 247.0, "confidence": 0.85 },
                { "id": "c_a2", "kind": "ship", "pos": [300.0, 500.0],
                  "bearing_deg": 90.0, "confidence": 0.4 }
              ],
              "events": [
                { "type": "hit", "amount": 12 },
                { "type": "shell_splash", "pos": [220.0, 505.0] }
              ]
            }
            """;

    @Test
    void parseWelcome() throws Exception {
        JsonNode n = MAPPER.readTree(WELCOME);
        Welcome w = Welcome.from(n);
        assertEquals("b_1", w.botId());
        assertEquals(1000, w.map().width());
        assertEquals(50.0f, w.shipSpecs().shellSpeed(), EPS);
        assertEquals(20, w.shipSpecs().maxAmmo());
    }

    @Test
    void parseTick() throws Exception {
        JsonNode n = MAPPER.readTree(TICK);
        WorldView v = WorldView.from(n);
        assertEquals(142, v.tick());
        assertEquals(78, v.self().hp());
        assertEquals(2, v.contacts().size());
        assertFalse(v.contacts().get(1).range().isPresent(), "passive contact has no range");
        assertEquals(247.0, v.contacts().get(0).range().getAsDouble(), 1e-3);
        assertEquals(2, v.events().size());
        assertInstanceOf(TickEvent.Hit.class, v.events().get(0));
        assertEquals(12, ((TickEvent.Hit) v.events().get(0)).amount());
        assertInstanceOf(TickEvent.ShellSplash.class, v.events().get(1));
    }

    @Test
    void nearestContact() throws Exception {
        WorldView v = WorldView.from(MAPPER.readTree(TICK));
        assertEquals("c_a1", v.nearestContact().orElseThrow().id());
    }

    @Test
    void commandSerializes() {
        Command c = new Command().throttle(1.0f).rudder(-0.5f).sensorMode(SensorMode.PASSIVE);
        JsonNode n = c.toJson(10);
        assertEquals("command", n.get("type").asText());
        assertEquals(10, n.get("tick").asLong());
        assertEquals(1.0, n.get("throttle").asDouble(), EPS);
        assertEquals(-0.5, n.get("rudder").asDouble(), EPS);
        assertEquals("passive", n.get("sensor_mode").asText());
        assertFalse(n.has("fire"));
    }

    @Test
    void commandFireAtStationary() {
        // Shoot from (100, 100) at a stationary target due east (200, 100).
        Command c = new Command().fireAt(new Vec2(100, 100), new Vec2(200, 100));
        JsonNode n = c.toJson(1);
        assertTrue(n.has("fire"));
        assertEquals(90.0, n.get("fire").get("bearing_deg").asDouble(), 1e-2);
        assertEquals(100.0, n.get("fire").get("range").asDouble(), 1e-2);
    }

    @Test
    void commandFireAtLeadsMovingTarget() {
        // Target at (100, 0) moving in +y at 10; shell speed 50.
        Command c = new Command().fireAt(
                new Vec2(0, 0), new Vec2(100, 0), new Vec2(0, 10), 50.0f);
        JsonNode n = c.toJson(1);
        double bearing = n.get("fire").get("bearing_deg").asDouble();
        // Aim point is south-east of origin -> bearing between 90 and 180.
        assertTrue(bearing > 90.0 && bearing < 180.0, "bearing was " + bearing);
    }

    @Test
    void parseGameOverDraw() throws Exception {
        JsonNode n = MAPPER.readTree(
                "{\"winner\": null, \"final_tick\": 3000, \"replay_id\": \"draw\"}");
        GameOver g = GameOver.from(n);
        assertTrue(g.winner().isEmpty());
        assertEquals(3000, g.finalTick());
    }
}
