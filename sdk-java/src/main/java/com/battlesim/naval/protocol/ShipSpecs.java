package com.battlesim.naval.protocol;

import com.fasterxml.jackson.databind.JsonNode;

/** Gameplay constants delivered in the {@code welcome} message. */
public record ShipSpecs(
        float maxForwardSpeed,
        float maxReverseSpeed,
        float acceleration,
        float turnRateDegPerS,
        int hullHp,
        int maxAmmo,
        int gunCooldownTicks,
        float hitRadius,
        float shellSpeed,
        float maxShellRange,
        float splashRadius,
        int maxSplashDamage) {

    public static ShipSpecs from(JsonNode n) {
        return new ShipSpecs(
                (float) n.get("max_forward_speed").asDouble(),
                (float) n.get("max_reverse_speed").asDouble(),
                (float) n.get("acceleration").asDouble(),
                (float) n.get("turn_rate_deg_per_s").asDouble(),
                n.get("hull_hp").asInt(),
                n.get("max_ammo").asInt(),
                n.get("gun_cooldown_ticks").asInt(),
                (float) n.get("hit_radius").asDouble(),
                (float) n.get("shell_speed").asDouble(),
                (float) n.get("max_shell_range").asDouble(),
                (float) n.get("splash_radius").asDouble(),
                n.get("max_splash_damage").asInt());
    }
}
