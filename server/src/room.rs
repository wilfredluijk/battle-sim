//! Room: a single match. Owns the world, the RNG, and the tick loop.
//!
//! The state machine is stubbed at `Lobby` for Phase 3 — bot lifecycle lands in Phase 4.

use std::time::Duration;

use rand::SeedableRng;
use rand_pcg::Pcg64;
use tokio::sync::broadcast;
use tokio::time::{interval, MissedTickBehavior};
use tracing::{debug, info};

use crate::sim::{physics, World};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomState {
    Lobby,
    Running,
    Ended,
}

#[derive(Debug)]
pub struct Room {
    pub name: String,
    pub world: World,
    pub state: RoomState,
    pub rng: Pcg64,
}

impl Room {
    pub fn new(name: String, width: f32, height: f32, seed: u64) -> Self {
        Self {
            name,
            world: World::new(width, height),
            state: RoomState::Lobby,
            rng: Pcg64::seed_from_u64(seed),
        }
    }

    /// Advance the simulation by one fixed timestep and bump the tick counter.
    pub fn step_tick(&mut self) {
        physics::step_world(&mut self.world);
        self.world.tick = self.world.tick.saturating_add(1);
    }
}

/// Drive a room's tick loop at `tick_hz` until the shutdown channel fires.
///
/// Phase 3 has no bots, so this is just an empty-world heartbeat that proves the loop
/// pacing and shutdown plumbing work. Phase 4 wires bot commands into `step_tick`.
pub async fn run_room(
    mut room: Room,
    tick_hz: u32,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> u64 {
    let period = Duration::from_secs_f64(1.0 / f64::from(tick_hz.max(1)));
    let mut ticker = interval(period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let name = room.name.clone();
    info!(room = %name, tick_hz, "room started");

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!(room = %name, final_tick = room.world.tick, "room: shutdown");
                break;
            }
            _ = ticker.tick() => {
                room.step_tick();
                debug!(room = %name, tick = room.world.tick, "tick");
            }
        }
    }
    room.world.tick
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_tick_increments_world_tick() {
        let mut room = Room::new("test".into(), 1000.0, 1000.0, 42);
        assert_eq!(room.world.tick, 0);
        room.step_tick();
        assert_eq!(room.world.tick, 1);
        for _ in 0..10 {
            room.step_tick();
        }
        assert_eq!(room.world.tick, 11);
    }
}
