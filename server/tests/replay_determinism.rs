//! Phase 8.3 acceptance: a fixed-seed match recorded to the replay log can be re-played
//! into a fresh `Room` and produce a byte-identical final world state.

use std::io::Cursor;
use std::net::{Ipv4Addr, SocketAddr};

use tokio::sync::oneshot;

use naval_server::protocol::{FireCommand, SensorMode, ServerMsg, SpectatorMsg};
use naval_server::replay::{
    self, rebuild_room_from_header, ReplayRecord, ReplayWriter, REPLAY_FORMAT_VERSION,
};
use naval_server::room::{BotRegistration, JoinError, PendingCommand, Room, RoomEvent, StartError};

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

fn start(room: &mut Room, name: &str) -> Result<(), StartError> {
    let (tx, mut rx) = oneshot::channel();
    room.handle_event(RoomEvent::OperatorStart {
        room: name.into(),
        reply: tx,
    });
    rx.try_recv().expect("oneshot reply")
}

fn cmd(
    tick: u64,
    throttle: f32,
    rudder: f32,
    mode: SensorMode,
    fire: Option<FireCommand>,
) -> PendingCommand {
    PendingCommand {
        tick,
        throttle,
        rudder,
        sensor_mode: mode,
        fire,
        activate_powerup: None,
    }
}

/// Drain a bot's outbound channel so its 32-deep buffer doesn't fill during long matches.
fn drain(reg: &mut BotRegistration) {
    while reg.outbound.try_recv().is_ok() {}
}

/// Drive a deterministic 200-tick mini-match between two bots: one sweeps a turn while
/// firing periodically, the other reverses with a slight rudder. The exact commands matter
/// less than that they're a varied, deterministic stream that exercises both physics and
/// combat.
fn run_match(room: &mut Room, r1: &mut BotRegistration, r2: &mut BotRegistration) {
    for tick in 0..200u64 {
        // Fire from r1 every 30 ticks at varying bearings; idle otherwise. Cooldown means
        // most of these will be rejected, which is fine — replay records the input either
        // way.
        let fire = if tick % 30 == 0 {
            Some(FireCommand {
                bearing_deg: (tick as f32 * 7.5) % 360.0,
                range: 200.0,
            })
        } else {
            None
        };
        let t = room.world.tick;
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r1.bot_id.clone(),
            command: cmd(t, 0.8, 0.4, SensorMode::Active, fire),
        });
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r2.bot_id.clone(),
            command: cmd(t, -0.3, -0.2, SensorMode::Passive, None),
        });
        room.step_tick();
        drain(r1);
        drain(r2);
    }
}

/// Snapshot the bits of `World` we want to compare across runs. Using `format!("{:?}", ..)`
/// is a deterministic-by-construction equality check: two worlds with bit-identical state
/// produce identical Debug output, and any drift shows up as a clear diff.
fn world_signature(room: &Room) -> String {
    format!(
        "tick={}\nnext_shell={}\nships={:?}\nshells={:?}",
        room.world.tick, room.world.next_shell_index, room.world.ships, room.world.shells,
    )
}

#[test]
fn replay_produces_byte_identical_final_state() {
    // ---- Live run ----
    let mut live_room = Room::new("test".into(), 1000.0, 1000.0, 12345, 10, 80, 4);
    let (writer, buf) = ReplayWriter::in_memory("match_test_12345".into());
    live_room.set_replay_writer(writer);

    let mut r1 = connect(&mut live_room, "alice").expect("alice connect");
    let mut r2 = connect(&mut live_room, "bob").expect("bob connect");
    drain(&mut r1);
    drain(&mut r2);
    live_room.handle_event(RoomEvent::BotReady {
        bot_id: r1.bot_id.clone(),
    });
    live_room.handle_event(RoomEvent::BotReady {
        bot_id: r2.bot_id.clone(),
    });
    start(&mut live_room, "test").expect("start");
    drain(&mut r1);
    drain(&mut r2);

    run_match(&mut live_room, &mut r1, &mut r2);

    let live_signature = world_signature(&live_room);
    let live_writer = live_room.take_replay_writer().expect("writer was set");
    drop(live_writer); // flush

    let log_bytes = buf.lock().unwrap().clone();
    assert!(!log_bytes.is_empty(), "replay log should not be empty");

    // ---- Replay run ----
    let records = replay::read_records_from(Cursor::new(log_bytes.clone())).expect("read records");
    let header = match records.first() {
        Some(ReplayRecord::Header(h)) => (**h).clone(),
        other => panic!("expected header, got {other:?}"),
    };
    assert_eq!(header.version, REPLAY_FORMAT_VERSION);
    assert_eq!(header.seed, 12345);
    assert_eq!(header.bots.len(), 2);
    assert_eq!(header.bots[0].bot_id, r1.bot_id);
    assert_eq!(header.bots[1].bot_id, r2.bot_id);

    let mut replay_room = rebuild_room_from_header(&header).expect("rebuild");

    for record in records.iter().skip(1) {
        match record {
            ReplayRecord::Tick(t) => {
                for c in &t.commands {
                    let pending = PendingCommand {
                        tick: t.tick,
                        throttle: c.throttle,
                        rudder: c.rudder,
                        sensor_mode: c.sensor_mode,
                        fire: c.fire,
                        activate_powerup: c.activate_powerup,
                    };
                    replay_room.inject_replay_command(&c.bot_id, pending);
                }
                while replay_room.world.tick < t.tick {
                    replay_room.step_tick();
                }
            }
            ReplayRecord::Disconnect(d) => {
                while replay_room.world.tick < d.tick {
                    replay_room.step_tick();
                }
                replay_room.remove_bot_and_ship(&d.bot_id);
            }
            ReplayRecord::End(end) => {
                while replay_room.world.tick < end.tick {
                    replay_room.step_tick();
                }
            }
            ReplayRecord::Header(_) => panic!("unexpected mid-stream header"),
        }
    }

    let replay_signature = world_signature(&replay_room);
    assert_eq!(
        live_signature, replay_signature,
        "replay state diverged from live state"
    );
}

/// Sim-relevant fields of every ship in a `SpectatorMsg::World`, sorted by id. Excludes the
/// observability-only fields (`commands_per_sec`, `ready`) that can differ between the live
/// run and a replay without indicating a determinism break.
fn spectator_ship_sig(msg: &SpectatorMsg) -> String {
    let SpectatorMsg::World {
        tick,
        ships,
        shells,
        ..
    } = msg;
    let mut ships: Vec<_> = ships
        .iter()
        .map(|s| {
            (
                s.id.clone(),
                s.pos,
                s.heading_deg,
                s.speed,
                s.hp,
                s.ammo,
                s.throttle,
                s.rudder,
                s.alive,
            )
        })
        .collect();
    ships.sort_by(|a, b| a.0.cmp(&b.0));
    format!("tick={tick}\nships={ships:?}\nshells={shells:?}")
}

/// F-01: a replay log with tick gaps — ticks where *no* bot issued a command, which the
/// writer omits — must re-simulate byte-identically. Bot 1 commands only every 3rd tick and
/// bot 2 goes silent for a 40-tick stretch mid-match, so consecutive records straddle gaps.
///
/// Before the fix, `capture_replay` injected each record's commands and then stepped the
/// whole gap, so `step_tick` (which drains `pending_command` unconditionally) consumed those
/// commands up to N-1 ticks early — the replayed trajectory diverged from the live one. This
/// test fails before the fix and passes after.
#[test]
fn replay_with_tick_gaps_is_byte_identical() {
    let mut live = Room::new("test".into(), 1000.0, 1000.0, 4242, 10, 80, 4);
    let (writer, buf) = ReplayWriter::in_memory("match_gap_4242".into());
    live.set_replay_writer(writer);

    let mut r1 = connect(&mut live, "alice").expect("alice connect");
    let mut r2 = connect(&mut live, "bob").expect("bob connect");
    drain(&mut r1);
    drain(&mut r2);
    live.handle_event(RoomEvent::BotReady {
        bot_id: r1.bot_id.clone(),
    });
    live.handle_event(RoomEvent::BotReady {
        bot_id: r2.bot_id.clone(),
    });
    start(&mut live, "test").expect("start");
    drain(&mut r1);
    drain(&mut r2);

    // Drive 118 ticks. The step producing `t` reads controls queued when `world.tick == t-1`.
    // Bot 1 commands only when `t % 3 == 1` (ticks 1, 4, ..., 118); bot 2 does likewise but
    // stays silent through the 40..=80 window. Ticks where neither commands produce no
    // replay record, so the log has gaps of ~3 plus bot 2's long silence — exactly the shape
    // that broke replay before F-01. The final tick (118) commands bot 1, so the last record
    // is at tick 118 and `capture_replay` stops there, matching the live snapshot below.
    for t in 1u64..=118 {
        let now = live.world.tick; // == t - 1
        if t % 3 == 1 {
            // Vary the controls each command so early consumption visibly diverges.
            let throttle = if (t / 3) % 2 == 0 { 0.9 } else { -0.7 };
            let rudder = (((t as f32) * 0.017).sin()).clamp(-1.0, 1.0);
            live.handle_event(RoomEvent::BotCommand {
                bot_id: r1.bot_id.clone(),
                command: cmd(now, throttle, rudder, SensorMode::Active, None),
            });
            if !(40..=80).contains(&t) {
                live.handle_event(RoomEvent::BotCommand {
                    bot_id: r2.bot_id.clone(),
                    command: cmd(now, -0.5, -rudder, SensorMode::Passive, None),
                });
            }
        }
        live.step_tick();
        drain(&mut r1);
        drain(&mut r2);
    }

    let live_final = live.spectator_world_snapshot();
    let live_sig = spectator_ship_sig(&live_final);
    drop(live.take_replay_writer()); // flush

    let bytes = buf.lock().unwrap().clone();
    let records = replay::read_records_from(Cursor::new(bytes)).expect("read records back");

    // The log must actually contain a gap, otherwise the test proves nothing.
    let tick_ticks: Vec<u64> = records
        .iter()
        .filter_map(|r| match r {
            ReplayRecord::Tick(t) => Some(t.tick),
            _ => None,
        })
        .collect();
    assert!(
        tick_ticks.windows(2).any(|w| w[1] - w[0] > 1),
        "test setup: expected a tick gap in the log, got records at {tick_ticks:?}"
    );

    let captured = replay::capture_replay(records).expect("capture replay");
    let replay_final = captured.frames.last().expect("at least one frame");
    let replay_sig = spectator_ship_sig(replay_final);

    assert_eq!(
        live_sig, replay_sig,
        "replay diverged from live across a tick gap"
    );
}

/// F-02: a bot dropping mid-match removes its ship immediately, which shifts the shared RNG
/// stream and (in a 2-bot match) ends the match by last-ship-standing. The `Disconnect`
/// record must reproduce that removal at the exact tick so the replay's final state and
/// recorded winner match the live run. Before the fix the record didn't exist: replay kept
/// bob's ghost ship, never triggered last-ship-standing, and diverged completely.
#[test]
fn disconnect_mid_match_replays_byte_identically() {
    let mut live = Room::new("test".into(), 1000.0, 1000.0, 777, 10, 80, 4);
    let (writer, buf) = ReplayWriter::in_memory("match_dc_777".into());
    live.set_replay_writer(writer);

    let mut r1 = connect(&mut live, "alice").expect("alice connect");
    let mut r2 = connect(&mut live, "bob").expect("bob connect");
    drain(&mut r1);
    drain(&mut r2);
    live.handle_event(RoomEvent::BotReady {
        bot_id: r1.bot_id.clone(),
    });
    live.handle_event(RoomEvent::BotReady {
        bot_id: r2.bot_id.clone(),
    });
    start(&mut live, "test").expect("start");
    drain(&mut r1);
    drain(&mut r2);

    // Drive 15 ticks so both ships move and draw sensor RNG.
    for _ in 0..15 {
        let t = live.world.tick;
        live.handle_event(RoomEvent::BotCommand {
            bot_id: r1.bot_id.clone(),
            command: cmd(t, 0.7, 0.3, SensorMode::Active, None),
        });
        live.handle_event(RoomEvent::BotCommand {
            bot_id: r2.bot_id.clone(),
            command: cmd(t, 0.5, -0.2, SensorMode::Passive, None),
        });
        live.step_tick();
        drain(&mut r1);
        drain(&mut r2);
    }

    // Bob drops. With one ship left, the next step ends the match (alice last standing).
    live.handle_event(RoomEvent::BotDisconnect {
        bot_id: r2.bot_id.clone(),
    });
    let t = live.world.tick;
    live.handle_event(RoomEvent::BotCommand {
        bot_id: r1.bot_id.clone(),
        command: cmd(t, 0.7, 0.3, SensorMode::Active, None),
    });
    live.step_tick();

    // Confirm the match actually ended and capture the announced winner.
    let mut live_winner: Option<String> = None;
    let mut saw_game_over = false;
    while let Ok(msg) = r1.outbound.try_recv() {
        if let ServerMsg::GameOver { winner, .. } = msg {
            saw_game_over = true;
            live_winner = winner;
        }
    }
    assert!(
        saw_game_over,
        "match should have ended after the disconnect"
    );
    assert_eq!(
        live_winner,
        Some(r1.bot_id.clone()),
        "alice should win as last ship standing"
    );

    let live_final = live.spectator_world_snapshot();
    let live_sig = spectator_ship_sig(&live_final);
    drop(live.take_replay_writer()); // flush

    let bytes = buf.lock().unwrap().clone();
    let records = replay::read_records_from(Cursor::new(bytes)).expect("read records back");

    // The log must actually carry the disconnect record for exactly bob.
    let disconnects: Vec<_> = records
        .iter()
        .filter_map(|r| match r {
            ReplayRecord::Disconnect(d) => Some((d.tick, d.bot_id.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        disconnects,
        vec![(15u64, r2.bot_id.clone())],
        "expected a single disconnect record for bob at tick 15"
    );

    let captured = replay::capture_replay(records).expect("capture replay");
    let replay_final = captured.frames.last().expect("at least one frame");
    assert_eq!(
        live_sig,
        spectator_ship_sig(replay_final),
        "replay diverged from live after a mid-match disconnect"
    );
    let end = captured.end.expect("match ended with an end record");
    assert_eq!(
        end.winner,
        Some(r1.bot_id.clone()),
        "replay must reproduce the recorded winner"
    );
}

/// F-02 backward compatibility: a v4 log (no `Disconnect` records) still loads and replays.
/// Older logs simply predate the record type, so `version <= REPLAY_FORMAT_VERSION` accepts
/// them and the driver never encounters a disconnect.
#[test]
fn v4_log_without_disconnect_still_loads() {
    // Drive a short natural-end match, then relabel the header version to 4 — the exact shape
    // of a pre-F-02 log: a header, tick records, an end record, and no disconnect record.
    let mut room = Room::new("test".into(), 1000.0, 1000.0, 55, 10, 80, 4);
    let (writer, buf) = ReplayWriter::in_memory("match_v4_55".into());
    room.set_replay_writer(writer);
    let mut r1 = connect(&mut room, "alice").expect("a");
    let mut r2 = connect(&mut room, "bob").expect("b");
    drain(&mut r1);
    drain(&mut r2);
    room.handle_event(RoomEvent::BotReady {
        bot_id: r1.bot_id.clone(),
    });
    room.handle_event(RoomEvent::BotReady {
        bot_id: r2.bot_id.clone(),
    });
    start(&mut room, "test").expect("start");
    drain(&mut r1);
    drain(&mut r2);

    // Point-blank kill so the match ends quickly with an end record.
    room.world.ships.get_mut(&r1.ship_id).unwrap().pos = glam::Vec2::new(500.0, 500.0);
    room.world.ships.get_mut(&r2.ship_id).unwrap().pos = glam::Vec2::new(700.0, 500.0);
    room.world.ships.get_mut(&r2.ship_id).unwrap().hp = 1;
    for _ in 0..50 {
        let t = room.world.tick;
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r1.bot_id.clone(),
            command: cmd(
                t,
                0.0,
                0.0,
                SensorMode::Passive,
                Some(FireCommand {
                    bearing_deg: 90.0,
                    range: 200.0,
                }),
            ),
        });
        room.step_tick();
        let mut over = false;
        while let Ok(msg) = r1.outbound.try_recv() {
            if matches!(msg, ServerMsg::GameOver { .. }) {
                over = true;
            }
        }
        if over {
            break;
        }
    }
    drop(room.take_replay_writer());

    let bytes = buf.lock().unwrap().clone();
    let mut records = replay::read_records_from(Cursor::new(bytes)).expect("read records");
    // Relabel to v4.
    if let Some(ReplayRecord::Header(h)) = records.first_mut() {
        h.version = 4;
    } else {
        panic!("first record must be a header");
    }
    assert!(
        !records
            .iter()
            .any(|r| matches!(r, ReplayRecord::Disconnect(_))),
        "a v4 log must not contain disconnect records"
    );

    // A v4 log loads and re-simulates without error.
    let captured = replay::capture_replay(records).expect("v4 log should still load");
    assert_eq!(captured.header.version, 4);
    assert!(captured.end.is_some(), "match ended with an end record");
}

/// Sanity check: the writer emits a header line followed by tick lines, and the final
/// frame for a match that ended naturally is an `end` record. Acceptance criterion for §8.1.
#[test]
fn replay_log_has_header_then_ticks_then_end_when_match_ends() {
    let mut room = Room::new("test".into(), 1000.0, 1000.0, 99, 10, 80, 4);
    let (writer, buf) = ReplayWriter::in_memory("match_test_99".into());
    room.set_replay_writer(writer);

    let mut r1 = connect(&mut room, "alpha").expect("alpha connect");
    let mut r2 = connect(&mut room, "beta").expect("beta connect");
    drain(&mut r1);
    drain(&mut r2);
    room.handle_event(RoomEvent::BotReady {
        bot_id: r1.bot_id.clone(),
    });
    room.handle_event(RoomEvent::BotReady {
        bot_id: r2.bot_id.clone(),
    });
    start(&mut room, "test").expect("start");
    drain(&mut r1);
    drain(&mut r2);

    // Force a near-instant kill: pre-damage r2 to 1 HP and have r1 fire at point-blank.
    room.world.ships.get_mut(&r1.ship_id).unwrap().pos = glam::Vec2::new(500.0, 500.0);
    room.world.ships.get_mut(&r2.ship_id).unwrap().pos = glam::Vec2::new(700.0, 500.0);
    room.world.ships.get_mut(&r2.ship_id).unwrap().hp = 1;
    let t0 = room.world.tick;
    room.handle_event(RoomEvent::BotCommand {
        bot_id: r1.bot_id.clone(),
        command: cmd(
            t0,
            0.0,
            0.0,
            SensorMode::Passive,
            Some(FireCommand {
                bearing_deg: 90.0,
                range: 200.0,
            }),
        ),
    });
    for _ in 0..50 {
        let t = room.world.tick;
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r2.bot_id.clone(),
            command: cmd(t, 0.0, 0.0, SensorMode::Passive, None),
        });
        room.step_tick();
        drain(&mut r1);
        // Don't drain r2 here — we want to see the game_over.
        if matches!(r2.outbound.try_recv(), Ok(ServerMsg::GameOver { .. })) {
            break;
        }
        drain(&mut r2);
    }

    // Drop the writer to flush the BufWriter.
    drop(room.take_replay_writer());

    let bytes = buf.lock().unwrap().clone();
    let records = replay::read_records_from(Cursor::new(bytes)).expect("read records back");

    assert!(
        records.len() >= 3,
        "header + tick + end at minimum, got {}",
        records.len()
    );
    assert!(matches!(records.first(), Some(ReplayRecord::Header(_))));
    assert!(matches!(records.last(), Some(ReplayRecord::End(_))));
    // No record between the first and last should be a header.
    for r in &records[1..records.len() - 1] {
        assert!(
            matches!(r, ReplayRecord::Tick(_)),
            "unexpected mid-stream {r:?}"
        );
    }
}

#[test]
fn powerup_activations_replay_byte_identically() {
    use naval_server::sim::PowerupId;

    // Live match: each bot picks a loadout, drives around, activates a powerup, then
    // fires a few shots. No direct state mutation — every change goes through commands,
    // so the replay log fully captures the run.
    let mut room = Room::new("test".into(), 1000.0, 1000.0, 31337, 10, 80, 4);
    let mut r1 = connect(&mut room, "alice").expect("a");
    let mut r2 = connect(&mut room, "bob").expect("b");
    drain(&mut r1);
    drain(&mut r2);

    room.handle_event(RoomEvent::BotSelectPowerups {
        bot_id: r1.bot_id.clone(),
        powerups: vec![PowerupId::Overdrive, PowerupId::HeavyShell],
    });
    room.handle_event(RoomEvent::BotSelectPowerups {
        bot_id: r2.bot_id.clone(),
        powerups: vec![PowerupId::ReinforcedHull, PowerupId::SmokeScreen],
    });

    let (writer, buf) = ReplayWriter::in_memory("match_pup_31337".into());
    room.set_replay_writer(writer);

    room.handle_event(RoomEvent::BotReady {
        bot_id: r1.bot_id.clone(),
    });
    room.handle_event(RoomEvent::BotReady {
        bot_id: r2.bot_id.clone(),
    });
    start(&mut room, "test").expect("start");
    drain(&mut r1);
    drain(&mut r2);

    // Tick 0: both bots activate one of their picks while throttling forward.
    let t = room.world.tick;
    room.handle_event(RoomEvent::BotCommand {
        bot_id: r1.bot_id.clone(),
        command: PendingCommand {
            tick: t,
            throttle: 1.0,
            rudder: 0.2,
            sensor_mode: SensorMode::Active,
            fire: None,
            activate_powerup: Some(PowerupId::Overdrive),
        },
    });
    room.handle_event(RoomEvent::BotCommand {
        bot_id: r2.bot_id.clone(),
        command: PendingCommand {
            tick: t,
            throttle: 1.0,
            rudder: -0.2,
            sensor_mode: SensorMode::Passive,
            fire: None,
            activate_powerup: Some(PowerupId::SmokeScreen),
        },
    });
    room.step_tick();
    drain(&mut r1);
    drain(&mut r2);

    // Drive a varied 80 ticks of motion + occasional fire. The pattern is arbitrary —
    // what matters is that it's reproducible.
    for i in 0..80 {
        let t = room.world.tick;
        let fire_r1 = if i % 10 == 5 {
            Some(FireCommand {
                bearing_deg: 90.0 + (i as f32) * 5.0,
                range: 250.0,
            })
        } else {
            None
        };
        let fire_r2 = if i % 12 == 3 {
            Some(FireCommand {
                bearing_deg: 270.0 - (i as f32) * 4.0,
                range: 220.0,
            })
        } else {
            None
        };
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r1.bot_id.clone(),
            command: cmd(t, 0.8, 0.15, SensorMode::Active, fire_r1),
        });
        room.handle_event(RoomEvent::BotCommand {
            bot_id: r2.bot_id.clone(),
            command: cmd(t, 0.6, -0.1, SensorMode::Passive, fire_r2),
        });
        // Second powerup activations mid-match.
        if i == 20 {
            let t = room.world.tick;
            room.handle_event(RoomEvent::BotCommand {
                bot_id: r1.bot_id.clone(),
                command: PendingCommand {
                    tick: t,
                    throttle: 0.8,
                    rudder: 0.15,
                    sensor_mode: SensorMode::Active,
                    fire: None,
                    activate_powerup: Some(PowerupId::HeavyShell),
                },
            });
        }
        if i == 30 {
            let t = room.world.tick;
            room.handle_event(RoomEvent::BotCommand {
                bot_id: r2.bot_id.clone(),
                command: PendingCommand {
                    tick: t,
                    throttle: 0.6,
                    rudder: -0.1,
                    sensor_mode: SensorMode::Passive,
                    fire: None,
                    activate_powerup: Some(PowerupId::ReinforcedHull),
                },
            });
        }
        room.step_tick();
        drain(&mut r1);
        drain(&mut r2);
    }

    let live_final_tick = room.world.tick;
    let live_ship_states: Vec<_> = room
        .world
        .ships
        .values()
        .map(|s| {
            (
                s.id.clone(),
                s.pos,
                s.heading_deg,
                s.speed,
                s.hp,
                s.ammo,
                s.alive,
                s.powerups.used.clone(),
                s.powerups.selected.clone(),
            )
        })
        .collect();
    drop(room.take_replay_writer());
    let bytes = buf.lock().unwrap().clone();

    // ---- Replay run ----
    let records = replay::read_records_from(Cursor::new(bytes)).expect("read records back");
    let header = match records.first() {
        Some(ReplayRecord::Header(h)) => (**h).clone(),
        other => panic!("expected header, got {other:?}"),
    };
    // Sanity: header carries each bot's loadout.
    assert!(
        header.bots.iter().any(|b| !b.selected_powerups.is_empty()),
        "header should record at least one bot's loadout"
    );

    let mut replay_room = rebuild_room_from_header(&header).expect("rebuild");
    for record in records.iter().skip(1) {
        match record {
            ReplayRecord::Tick(t) => {
                for c in &t.commands {
                    let pending = PendingCommand {
                        tick: t.tick,
                        throttle: c.throttle,
                        rudder: c.rudder,
                        sensor_mode: c.sensor_mode,
                        fire: c.fire,
                        activate_powerup: c.activate_powerup,
                    };
                    replay_room.inject_replay_command(&c.bot_id, pending);
                }
                while replay_room.world.tick < t.tick {
                    replay_room.step_tick();
                }
            }
            ReplayRecord::Disconnect(d) => {
                while replay_room.world.tick < d.tick {
                    replay_room.step_tick();
                }
                replay_room.remove_bot_and_ship(&d.bot_id);
            }
            ReplayRecord::End(_) => {}
            ReplayRecord::Header(_) => panic!("unexpected mid-stream header"),
        }
    }
    while replay_room.world.tick < live_final_tick {
        replay_room.step_tick();
    }

    assert_eq!(
        replay_room.world.tick, live_final_tick,
        "final tick differs between live and replay"
    );
    let replay_ship_states: Vec<_> = replay_room
        .world
        .ships
        .values()
        .map(|s| {
            (
                s.id.clone(),
                s.pos,
                s.heading_deg,
                s.speed,
                s.hp,
                s.ammo,
                s.alive,
                s.powerups.used.clone(),
                s.powerups.selected.clone(),
            )
        })
        .collect();
    assert_eq!(
        live_ship_states, replay_ship_states,
        "ship state diverged between live and replay"
    );
}
