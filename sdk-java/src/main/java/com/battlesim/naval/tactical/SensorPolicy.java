package com.battlesim.naval.tactical;

import com.battlesim.naval.protocol.SensorMode;
import com.battlesim.naval.protocol.WorldView;

/**
 * Plug-in interface for deciding the per-tick sensor mode.
 *
 * <p>Ships with four implementations: {@link AlwaysActive}, {@link AlwaysPassive},
 * {@link DutyCycle}, {@link PingWhenStale}. Implement your own if you need
 * custom logic.
 */
@FunctionalInterface
public interface SensorPolicy {
    SensorMode choose(WorldView view, Tracker tracker);

    final class AlwaysActive implements SensorPolicy {
        @Override
        public SensorMode choose(WorldView view, Tracker tracker) {
            return SensorMode.ACTIVE;
        }
    }

    final class AlwaysPassive implements SensorPolicy {
        @Override
        public SensorMode choose(WorldView view, Tracker tracker) {
            return SensorMode.PASSIVE;
        }
    }

    /** Cycle: {@code activeTicks} of active, then {@code passiveTicks} of passive. */
    final class DutyCycle implements SensorPolicy {
        private final int activeTicks;
        private final int passiveTicks;

        public DutyCycle(int activeTicks, int passiveTicks) {
            this.activeTicks = activeTicks;
            this.passiveTicks = passiveTicks;
        }

        @Override
        public SensorMode choose(WorldView view, Tracker tracker) {
            int cycle = Math.max(1, activeTicks + passiveTicks);
            long phase = view.tick() % cycle;
            return phase < activeTicks ? SensorMode.ACTIVE : SensorMode.PASSIVE;
        }
    }

    /**
     * Active only when no track has a fresh fix: passive while
     * {@code min(tick - lastSeen) < threshold}; active otherwise (including
     * when the track set is empty).
     */
    final class PingWhenStale implements SensorPolicy {
        private final int staleThresholdTicks;

        public PingWhenStale(int staleThresholdTicks) {
            this.staleThresholdTicks = staleThresholdTicks;
        }

        @Override
        public SensorMode choose(WorldView view, Tracker tracker) {
            var tracks = tracker.tracks();
            if (tracks.isEmpty()) return SensorMode.ACTIVE;
            long freshestGap = Long.MAX_VALUE;
            for (Track t : tracks) {
                long gap = view.tick() - t.lastSeenTick();
                if (gap < freshestGap) freshestGap = gap;
            }
            return freshestGap >= staleThresholdTicks ? SensorMode.ACTIVE : SensorMode.PASSIVE;
        }
    }
}
