package com.battlesim.examples;

import com.battlesim.naval.BotRunner;
import com.battlesim.naval.protocol.Vec2;
import com.battlesim.naval.tactical.Intent;
import com.battlesim.naval.tactical.SensorPolicy;
import com.battlesim.naval.tactical.TacticalBot;
import com.battlesim.naval.tactical.TacticalContext;
import com.battlesim.naval.tactical.Track;

/**
 * Pure Layer-3 example: subclass {@link TacticalBot} and override
 * {@link #decide}. The framework wires up tracking, fire control, steering,
 * sensor scheduling, and evasion.
 */
public final class StrategistBot extends TacticalBot {
    private static final int LOW_HP = 30;
    private static final float MARGIN = 60.0f;

    public StrategistBot() {
        this.sensorPolicy = new SensorPolicy.PingWhenStale(12);
    }

    @Override
    public Intent decide(TacticalContext ctx) {
        if (ctx.me().hp() < LOW_HP && !ctx.threats().isEmpty()) {
            return Intent.retreatTo(safestCorner(ctx));
        }
        if (!ctx.threats().isEmpty()) {
            return Intent.engage(ctx.threats().nearest().orElseThrow());
        }
        return Intent.patrol(
                ctx.mapWidth() * 0.25f,
                ctx.mapHeight() * 0.25f,
                ctx.mapWidth() * 0.75f,
                ctx.mapHeight() * 0.75f);
    }

    private static Vec2 safestCorner(TacticalContext ctx) {
        Track threat = ctx.threats().nearest().orElseThrow();
        Vec2[] corners = new Vec2[] {
                new Vec2(MARGIN, MARGIN),
                new Vec2(ctx.mapWidth() - MARGIN, MARGIN),
                new Vec2(MARGIN, ctx.mapHeight() - MARGIN),
                new Vec2(ctx.mapWidth() - MARGIN, ctx.mapHeight() - MARGIN),
        };
        Vec2 best = corners[0];
        float bestSq = sq(best.x() - threat.pos().x()) + sq(best.y() - threat.pos().y());
        for (int i = 1; i < corners.length; i++) {
            float s = sq(corners[i].x() - threat.pos().x()) + sq(corners[i].y() - threat.pos().y());
            if (s > bestSq) {
                bestSq = s;
                best = corners[i];
            }
        }
        return best;
    }

    private static float sq(float v) { return v * v; }

    public static void main(String[] args) {
        BotArgs parsed = BotArgs.parse(args, "strategist");
        BotRunner.run(new StrategistBot(), parsed.host(), parsed.port(), parsed.name());
    }
}
