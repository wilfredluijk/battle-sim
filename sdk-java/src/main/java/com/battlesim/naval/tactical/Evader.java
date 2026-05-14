package com.battlesim.naval.tactical;

import com.battlesim.naval.Command;
import com.battlesim.naval.protocol.TickEvent;
import com.battlesim.naval.protocol.WorldView;

import java.util.Optional;

/**
 * Hit-triggered evasive maneuver state machine.
 *
 * <p>Returns an override {@link Command} while evading; empty otherwise. The
 * orchestrator (or your own {@code onTick}) treats a non-empty return as a
 * preempting command — evasion outranks the player's tactical intent.
 */
public class Evader {

    public enum State { IDLE, EVADING, COOLDOWN }

    private final int evasionTicks;
    private final int cooldownTicks;
    private final float throttle;
    private State state = State.IDLE;
    private long stateUntil = 0;
    private float rudderSign;

    public Evader() {
        this(15, 10, 1.0f, 1.0f);
    }

    public Evader(int evasionTicks, int cooldownTicks) {
        this(evasionTicks, cooldownTicks, 1.0f, 1.0f);
    }

    public Evader(int evasionTicks, int cooldownTicks, float throttle, float initialRudderSign) {
        this.evasionTicks = evasionTicks;
        this.cooldownTicks = cooldownTicks;
        this.throttle = throttle;
        this.rudderSign = initialRudderSign >= 0 ? 1.0f : -1.0f;
    }

    public State state() { return state; }

    /**
     * Advance the state machine. Returns an override {@link Command} while
     * evading, else empty.
     */
    public Optional<Command> update(WorldView view) {
        long tick = view.tick();

        // Timed transitions.
        if (state != State.IDLE && tick >= stateUntil) {
            if (state == State.EVADING) {
                state = State.COOLDOWN;
                stateUntil = tick + cooldownTicks;
            } else {
                state = State.IDLE;
            }
        }

        // React to fresh hits.
        boolean wasHit = false;
        for (TickEvent e : view.events()) {
            if (e instanceof TickEvent.Hit) {
                wasHit = true;
                break;
            }
        }
        if (wasHit && (state == State.IDLE || state == State.COOLDOWN)) {
            if (state == State.COOLDOWN) {
                rudderSign = -rudderSign;
            }
            state = State.EVADING;
            stateUntil = tick + evasionTicks;
        }

        if (state == State.EVADING) {
            return Optional.of(new Command().throttle(throttle).rudder(rudderSign));
        }
        return Optional.empty();
    }
}
