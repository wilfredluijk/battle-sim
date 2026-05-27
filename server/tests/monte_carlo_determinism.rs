//! Two Monte Carlo runs with the same `(mc_seed, variance_mode, sim_config, roster)`
//! must produce bit-identical per-match outcomes. This is the headline guarantee that
//! lets the spectator UI report "bot X is statistically stronger" — if the runs aren't
//! deterministic the win rate is just noise.
//!
//! The test drives the room synchronously (no live tick loop) so it stays fast and
//! doesn't depend on wall-clock timing.

use std::net::{Ipv4Addr, SocketAddr};

use tokio::sync::oneshot;

use naval_server::monte_carlo::{McConfig, McStatus, VarianceMode};
use naval_server::protocol::{FireCommand, SensorMode, ServerMsg};
use naval_server::room::{
    BotRegistration, JoinError, McStartError, PendingCommand, Room, RoomEvent,
};

fn test_peer() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
}

fn connect(room: &mut Room, name: &str) -> Result<BotRegistration, JoinError> {
    let (tx, mut rx) = oneshot::channel();
    room.handle_event(RoomEvent::BotConnect {
        peer: test_peer(),
        name: name.into(),
        version: "1.0".into(),
        reply: tx,
    });
    rx.try_recv().expect("oneshot reply")
}

fn start_mc(room: &mut Room, config: McConfig) -> Result<String, McStartError> {
    let (tx, mut rx) = oneshot::channel();
    room.handle_event(RoomEvent::StartMonteCarlo { config, reply: tx });
    rx.try_recv().expect("oneshot reply")
}

fn status(room: &mut Room) -> McStatus {
    let (tx, mut rx) = oneshot::channel();
    room.handle_event(RoomEvent::QueryMonteCarloStatus { reply: tx });
    rx.try_recv().expect("oneshot reply")
}

fn drain(reg: &mut BotRegistration) {
    while reg.outbound.try_recv().is_ok() {}
}

/// Scripted bot: respond to every `tick` frame with one `BotCommand`. The "killer" version
/// drives forward at full throttle with active radar and fires every tick; the "victim"
/// stays put with passive sensors. Mirrors the integration test's bots.
struct ScriptedBot {
    bot_id: String,
    shoot: bool,
}

impl ScriptedBot {
    fn process(&mut self, reg: &mut BotRegistration, room: &mut Room) {
        while let Ok(msg) = reg.outbound.try_recv() {
            if let ServerMsg::Tick { tick, contacts, .. } = msg {
                // Killer fires at the first active-radar contact (which carries a
                // numeric range). Without a target it just drives forward.
                let fire = if self.shoot {
                    contacts.iter().find_map(|c| {
                        c.range.map(|r| FireCommand {
                            bearing_deg: c.bearing_deg,
                            range: r,
                        })
                    })
                } else {
                    None
                };
                let cmd = PendingCommand {
                    tick,
                    throttle: if self.shoot { 1.0 } else { 0.0 },
                    rudder: 0.0,
                    sensor_mode: if self.shoot {
                        SensorMode::Active
                    } else {
                        SensorMode::Passive
                    },
                    fire,
                    activate_powerup: None,
                };
                room.handle_event(RoomEvent::BotCommand {
                    bot_id: self.bot_id.clone(),
                    command: cmd,
                });
            }
        }
    }
}

/// Set up a 2-bot room with the standard killer/victim roster used across these tests.
fn make_two_bot_room(room_seed: u64) -> (Room, BotRegistration, BotRegistration) {
    let mut room = Room::new("test".into(), 1000.0, 1000.0, room_seed, 10, 80, 4);
    let r1 = connect(&mut room, "killer").expect("killer");
    let r2 = connect(&mut room, "victim").expect("victim");
    (room, r1, r2)
}

/// Drive an MC batch to completion synchronously and return the final status snapshot.
fn run_mc_to_completion(n_matches: u32, mc_seed: u64, variance_mode: VarianceMode) -> McStatus {
    let (mut room, mut r1, mut r2) = make_two_bot_room(1234);
    let mut bot1 = ScriptedBot {
        bot_id: r1.bot_id.clone(),
        shoot: true,
    };
    let mut bot2 = ScriptedBot {
        bot_id: r2.bot_id.clone(),
        shoot: false,
    };
    drain(&mut r1);
    drain(&mut r2);

    // Mark both bots ready, then start the MC batch.
    room.handle_event(RoomEvent::BotReady {
        bot_id: r1.bot_id.clone(),
    });
    room.handle_event(RoomEvent::BotReady {
        bot_id: r2.bot_id.clone(),
    });
    let cfg = McConfig {
        n_matches,
        mc_seed,
        variance_mode,
        per_tick_timeout_ms: None,
        spectator_throttle: Some(0),
        sim_config: None,
    };
    start_mc(&mut room, cfg).expect("mc start");

    // Each match is capped at MATCH_TIMEOUT_TICKS = 3000 ticks; ample headroom.
    let max_iter = 3500u32 * n_matches.max(1);
    for _ in 0..max_iter {
        bot1.process(&mut r1, &mut room);
        bot2.process(&mut r2, &mut room);
        room.step_tick();
        bot1.process(&mut r1, &mut room);
        bot2.process(&mut r2, &mut room);
        let st = status(&mut room);
        if !st.running {
            return st;
        }
    }
    let st = status(&mut room);
    panic!(
        "monte carlo run did not finalize within {max_iter} iterations: \
         running={} completed={}/{} current_tick={}",
        st.running, st.completed, st.total, st.current_match_tick,
    );
}

#[test]
fn two_runs_with_same_seed_produce_identical_results() {
    let a = run_mc_to_completion(5, 42, VarianceMode::Fixed);
    let b = run_mc_to_completion(5, 42, VarianceMode::Fixed);

    assert_eq!(a.completed, 5, "all 5 matches finished");
    assert_eq!(b.completed, 5);
    assert_eq!(a.wins, b.wins, "winner counts must be identical");
    assert_eq!(a.draws, b.draws);
    assert_eq!(
        a.results.len(),
        b.results.len(),
        "result tail length must match",
    );
    for (r1, r2) in a.results.iter().zip(b.results.iter()) {
        assert_eq!(r1.match_index, r2.match_index);
        assert_eq!(r1.seed, r2.seed, "per-match seed must be identical");
        assert_eq!(r1.winner, r2.winner, "winner per match must be identical");
        assert_eq!(
            r1.duration_ticks, r2.duration_ticks,
            "duration per match must be identical",
        );
    }
}

#[test]
fn two_runs_with_same_seed_in_shuffled_mode_are_also_identical() {
    let a = run_mc_to_completion(4, 99, VarianceMode::Shuffled);
    let b = run_mc_to_completion(4, 99, VarianceMode::Shuffled);
    assert_eq!(a.completed, 4);
    assert_eq!(a.wins, b.wins);
    for (r1, r2) in a.results.iter().zip(b.results.iter()) {
        assert_eq!(r1.seed, r2.seed);
        assert_eq!(r1.winner, r2.winner);
        assert_eq!(r1.duration_ticks, r2.duration_ticks);
    }
}

#[test]
fn different_seeds_produce_different_per_match_seeds() {
    let a = run_mc_to_completion(3, 1, VarianceMode::Rotated);
    let b = run_mc_to_completion(3, 2, VarianceMode::Rotated);
    let a_seeds: Vec<u64> = a.results.iter().map(|r| r.seed).collect();
    let b_seeds: Vec<u64> = b.results.iter().map(|r| r.seed).collect();
    assert_ne!(
        a_seeds, b_seeds,
        "different mc_seed must produce different per-match seeds",
    );
}

#[test]
fn run_finishes_in_completed_state() {
    let status = run_mc_to_completion(2, 17, VarianceMode::Fixed);
    assert!(
        !status.running,
        "status.running must clear after the run finishes"
    );
    assert_eq!(
        status.ended_reason.as_deref(),
        Some("completed"),
        "ended_reason must say `completed` for a natural finish",
    );
    assert_eq!(status.completed, 2);
    assert_eq!(status.total, 2);
}

#[test]
fn variance_mode_propagates_through_to_status() {
    // Sanity check that the chosen variance_mode is preserved in the status snapshot.
    let status = run_mc_to_completion(2, 7, VarianceMode::Rotated);
    assert_eq!(status.variance_mode, VarianceMode::Rotated);
    assert_eq!(status.mc_seed, 7);
}
