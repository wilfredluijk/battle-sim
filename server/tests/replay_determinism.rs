//! Phase 8.3 acceptance: a fixed-seed match recorded to the replay log can be re-played
//! into a fresh `Room` and produce a byte-identical final world state.

use std::io::Cursor;
use std::net::{Ipv4Addr, SocketAddr};

use tokio::sync::oneshot;

use naval_server::protocol::{FireCommand, SensorMode, ServerMsg};
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
        Some(ReplayRecord::Header(h)) => h.clone(),
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
                    };
                    replay_room.inject_replay_command(&c.bot_id, pending);
                }
                while replay_room.world.tick < t.tick {
                    replay_room.step_tick();
                }
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
