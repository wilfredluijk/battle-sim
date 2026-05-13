package com.battlesim.naval.tactical;

import com.battlesim.naval.Command;
import com.battlesim.naval.Geometry;
import com.battlesim.naval.protocol.FireCommand;
import com.battlesim.naval.protocol.SelfState;
import com.battlesim.naval.protocol.ShipSpecs;
import com.battlesim.naval.protocol.Vec2;
import com.battlesim.naval.protocol.WorldView;

import java.util.Optional;

/**
 * Fire-control helper.
 *
 * <p>Wraps cooldown tracking, ammo accounting, lead-target computation, range
 * and time-of-flight feasibility, and a self-splash guard. {@link #solve} is
 * side-effect-free; callers call {@link #noteFired} when they actually attach
 * the resulting {@link FireCommand}. For one-call ergonomics, use
 * {@link #attempt(Command, SelfState, Track, WorldView)}.
 */
public class Gunner {

    public static final class Config {
        public float selfSplashMargin = 1.5f;
        public int maxActiveAgeTicks = 5;
        public boolean requireRecentActive = true;
    }

    private final ShipSpecs specs;
    private final int cooldown;
    private final float selfSplash;
    private final int maxAge;
    private final boolean requireActive;
    private long nextFireTick = 0;

    public Gunner(ShipSpecs specs) {
        this(specs, new Config());
    }

    public Gunner(ShipSpecs specs, Config config) {
        this.specs = specs;
        this.cooldown = specs.gunCooldownTicks();
        this.selfSplash = specs.splashRadius() * config.selfSplashMargin;
        this.maxAge = config.maxActiveAgeTicks;
        this.requireActive = config.requireRecentActive;
    }

    /** Return a vetted {@link FireSolution}, or empty if no shot is available. */
    public Optional<FireSolution> solve(SelfState me, Track track, WorldView view) {
        if (view.tick() < nextFireTick) return Optional.empty();
        if (me.ammo() <= 0) return Optional.empty();
        if (requireActive && (view.tick() - track.lastActiveTick()) > maxAge) {
            return Optional.empty();
        }

        Optional<Vec2> pred = Geometry.leadTarget(me.pos(), track.pos(), track.vel(), specs.shellSpeed());
        if (pred.isEmpty()) return Optional.empty();

        Vec2 aim = pred.get();
        float dx = aim.x() - me.pos().x();
        float dy = aim.y() - me.pos().y();
        float range = (float) Math.hypot(dx, dy);
        if (range > specs.maxShellRange()) return Optional.empty();
        if (range < selfSplash) return Optional.empty();

        float bearing = Geometry.bearingTo(me.pos(), aim);
        return Optional.of(new FireSolution(bearing, range, aim, track.trackId()));
    }

    /**
     * Convenience: solve and attach to {@code cmd}, recording cooldown.
     *
     * @return {@code true} if a shot was attached.
     */
    public boolean attempt(Command cmd, SelfState me, Track track, WorldView view) {
        Optional<FireSolution> sol = solve(me, track, view);
        if (sol.isEmpty()) return false;
        cmd.fire(toFireCommand(sol.get()));
        noteFired(view.tick());
        return true;
    }

    public static FireCommand toFireCommand(FireSolution sol) {
        return new FireCommand(sol.bearingDeg(), sol.range());
    }

    public void noteFired(long tick) {
        nextFireTick = tick + cooldown;
    }

    public long nextFireTick() { return nextFireTick; }

    public boolean canFire(WorldView view, SelfState me) {
        return view.tick() >= nextFireTick && me.ammo() > 0;
    }
}
