package com.battlesim.naval;

import com.battlesim.naval.protocol.FireCommand;
import com.battlesim.naval.protocol.SensorMode;
import com.battlesim.naval.protocol.Vec2;
import com.fasterxml.jackson.databind.node.JsonNodeFactory;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.util.Optional;

/**
 * A bot's response to a single {@code tick}.
 *
 * <p>Builder-style: chain {@code .throttle(...).rudder(...).sensorMode(...).fireAt(...)} and
 * pass the result back from {@link Bot#onTick}. The runtime adds the wire-protocol
 * envelope and the matching {@code tick} number.
 */
public final class Command {
    private float throttle = 0.0f;
    private float rudder = 0.0f;
    private SensorMode sensorMode = SensorMode.ACTIVE;
    private FireCommand fire = null;

    public Command throttle(float v) {
        this.throttle = v;
        return this;
    }

    public Command rudder(float v) {
        this.rudder = v;
        return this;
    }

    public Command sensorMode(SensorMode mode) {
        this.sensorMode = mode;
        return this;
    }

    public Command fire(FireCommand f) {
        this.fire = f;
        return this;
    }

    /** Aim a shell directly at a stationary target from {@code shooterPos}. */
    public Command fireAt(Vec2 shooterPos, Vec2 targetPos) {
        return fireAt(shooterPos, targetPos, null, 50.0f, null);
    }

    /** Aim a shell at a moving target, leading by {@code shellSpeed}. */
    public Command fireAt(Vec2 shooterPos, Vec2 targetPos, Vec2 targetVel, float shellSpeed) {
        return fireAt(shooterPos, targetPos, targetVel, shellSpeed, null);
    }

    /**
     * Compute a {@link FireCommand}. {@code targetVel} of {@code null} or zero disables
     * leading. {@code rangeOverride} of {@code null} uses the distance to the aim point;
     * the server clamps to {@code maxShellRange}.
     */
    public Command fireAt(
            Vec2 shooterPos, Vec2 targetPos, Vec2 targetVel, float shellSpeed, Float rangeOverride) {
        Vec2 aim = targetPos;
        if (targetVel != null && (targetVel.x() != 0.0f || targetVel.y() != 0.0f)) {
            Optional<Vec2> predicted = Geometry.leadTarget(shooterPos, targetPos, targetVel, shellSpeed);
            if (predicted.isPresent()) aim = predicted.get();
        }
        float bearing = Geometry.bearingTo(shooterPos, aim);
        float range;
        if (rangeOverride != null) {
            range = rangeOverride;
        } else {
            float dx = aim.x() - shooterPos.x();
            float dy = aim.y() - shooterPos.y();
            range = (float) Math.hypot(dx, dy);
        }
        this.fire = new FireCommand(bearing, range);
        return this;
    }

    public ObjectNode toJson(long tick) {
        ObjectNode node = JsonNodeFactory.instance.objectNode();
        node.put("type", "command");
        node.put("tick", tick);
        node.put("throttle", throttle);
        node.put("rudder", rudder);
        node.put("sensor_mode", sensorMode.wire());
        if (fire != null) {
            ObjectNode f = node.putObject("fire");
            f.put("bearing_deg", fire.bearingDeg());
            f.put("range", fire.range());
        }
        return node;
    }

    public float throttleValue() {
        return throttle;
    }

    public float rudderValue() {
        return rudder;
    }

    public SensorMode sensorModeValue() {
        return sensorMode;
    }

    public Optional<FireCommand> fireValue() {
        return Optional.ofNullable(fire);
    }
}
