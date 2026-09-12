//! Behavioral regressions from the September review.
use glam::Vec2;
use naval_server::{
    monte_carlo::{McConfig, McStatus, VarianceMode},
    protocol::{MapInfo, SensorMode, ServerMsg, TickEvent},
    replay::{
        self, ReplayBot, ReplayEnd, ReplayHeader, ReplayRecord, ReplayWriter, REPLAY_FORMAT_VERSION,
    },
    room::{self, BotRegistration, PendingCommand, Room, RoomEvent, RoomState},
    sim::{PowerupId, SimConfig},
};
use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, oneshot};

fn room() -> Room {
    Room::new("test".into(), 700., 700., 42, 10, 80, 24)
}
fn bot(room: &mut Room, name: &str) -> BotRegistration {
    let (reply, mut rx) = oneshot::channel();
    room.handle_event(RoomEvent::BotConnect {
        peer: SocketAddr::from((Ipv4Addr::LOCALHOST, 1)),
        name: name.into(),
        version: "test".into(),
        reply,
    });
    let mut reg = rx.try_recv().unwrap().unwrap();
    reg.outbound.try_recv().unwrap();
    room.handle_event(RoomEvent::BotReady {
        bot_id: reg.bot_id.clone(),
    });
    reg
}
fn start(room: &mut Room) {
    let (reply, mut rx) = oneshot::channel();
    room.handle_event(RoomEvent::OperatorStart {
        room: room.name.clone(),
        reply,
    });
    rx.try_recv().unwrap().unwrap();
}
fn command(tick: u64) -> PendingCommand {
    PendingCommand {
        tick,
        throttle: 0.,
        rudder: 0.,
        fire: None,
        sensor_mode: SensorMode::Active,
        activate_powerup: None,
    }
}
fn mc(room: &mut Room) {
    let mut config = room.world.config;
    config.hull_hp = 333;
    let (reply, mut rx) = oneshot::channel();
    room.handle_event(RoomEvent::StartMonteCarlo {
        config: McConfig {
            n_matches: 2,
            mc_seed: 999,
            variance_mode: VarianceMode::Fixed,
            per_tick_timeout_ms: Some(1000),
            spectator_throttle: Some(1),
            sim_config: Some(config),
        },
        reply,
    });
    rx.try_recv().unwrap().unwrap();
}
fn status(room: &mut Room) -> McStatus {
    let (reply, mut rx) = oneshot::channel();
    room.handle_event(RoomEvent::QueryMonteCarloStatus { reply });
    rx.try_recv().unwrap()
}

#[test]
fn game_start_publishes_current_specs_after_lobby_changes() {
    let mut room = room();
    let mut bot = bot(&mut room, "alpha");
    let mut config = room.world.config;
    config.shell_speed = 123.;
    let (reply, mut rx) = oneshot::channel();
    room.handle_event(RoomEvent::OperatorConfigure { config, reply });
    rx.try_recv().unwrap().unwrap();
    let ServerMsg::Configuration {
        config_hash,
        configuration,
    } = bot.outbound.try_recv().unwrap()
    else {
        panic!("configuration must be published before readiness");
    };
    assert_eq!(configuration["sim_config"]["shell_speed"], 123.);
    assert!(!room.all_ready());
    room.handle_event(RoomEvent::BotReadyChecked {
        bot_id: bot.bot_id.clone(),
        config_hash,
    });
    start(&mut room);
    match bot.outbound.try_recv().unwrap() {
        ServerMsg::GameStart {
            ship_specs,
            simulation_dt,
            ..
        } => {
            assert_eq!(ship_specs.shell_speed, 123.);
            assert_eq!(simulation_dt, 0.1);
        }
        msg => panic!("expected game_start: {msg:?}"),
    }
}

#[test]
fn soft_stop_records_current_match_and_restores_operator_configuration() {
    let mut room = room();
    let _a = bot(&mut room, "alpha");
    let b = bot(&mut room, "bravo");
    let original = room.world.config;
    mc(&mut room);
    assert_ne!(room.seed, 42);
    let (reply, mut rx) = oneshot::channel();
    room.handle_event(RoomEvent::StopMonteCarlo {
        force_abort: false,
        reply,
    });
    rx.try_recv().unwrap().unwrap();
    assert!(room.in_lockstep());
    assert!(status(&mut room).running);
    room.world.ships.get_mut(&b.ship_id).unwrap().alive = false;
    room.step_tick();
    let result = status(&mut room);
    assert_eq!(result.completed, 1);
    assert_eq!(result.results.len(), 1);
    assert_eq!(result.ended_reason.as_deref(), Some("stopped"));
    assert_eq!(room.seed, 42);
    assert_eq!(room.world.config, original);
}

#[test]
fn natural_batch_completion_and_abort_restore_configuration() {
    for abort in [false, true] {
        let mut room = room();
        let _a = bot(&mut room, "alpha");
        let b = bot(&mut room, "bravo");
        let original = room.world.config;
        mc(&mut room);
        if abort {
            let (reply, mut rx) = oneshot::channel();
            room.handle_event(RoomEvent::OperatorAbort { reply });
            rx.try_recv().unwrap().unwrap();
        } else {
            for _ in 0..2 {
                room.world.ships.get_mut(&b.ship_id).unwrap().alive = false;
                room.step_tick();
            }
            assert_eq!(status(&mut room).completed, 2);
        }
        assert_eq!(room.seed, 42);
        assert_eq!(room.world.config, original);
        assert!(!room.in_lockstep());
    }
}

#[test]
fn sensor_effects_last_the_full_number_of_sweeps_and_emp_status_is_separate() {
    for powerup in [
        PowerupId::AwacsScan,
        PowerupId::SmokeScreen,
        PowerupId::EmpBurst,
        PowerupId::CounterBatteryTrace,
    ] {
        let mut room = room();
        let mut a = bot(&mut room, "alpha");
        let mut b = bot(&mut room, "bravo");
        start(&mut room);
        a.outbound.try_recv().unwrap();
        b.outbound.try_recv().unwrap();
        room.world.config.powerups.awacs_duration_ticks = 3;
        room.world.config.powerups.smoke_screen_duration_ticks = 3;
        room.world.config.powerups.emp_burst_duration_ticks = 3;
        let far = matches!(
            powerup,
            PowerupId::AwacsScan | PowerupId::CounterBatteryTrace
        );
        room.world.ships.get_mut(&a.ship_id).unwrap().pos = Vec2::new(100., 100.);
        room.world.ships.get_mut(&b.ship_id).unwrap().pos =
            Vec2::new(if far { 600. } else { 200. }, 100.);
        let actor = if powerup == PowerupId::AwacsScan {
            &a
        } else {
            &b
        };
        room.world
            .ships
            .get_mut(&actor.ship_id)
            .unwrap()
            .powerups
            .selected = vec![powerup];
        if powerup == PowerupId::EmpBurst {
            room.world
                .ships
                .get_mut(&a.ship_id)
                .unwrap()
                .powerups
                .selected = vec![PowerupId::EmpBurst];
        }
        if powerup == PowerupId::CounterBatteryTrace {
            let state = &mut room.world.ships.get_mut(&a.ship_id).unwrap().powerups;
            state.trace_attacker = Some(b.ship_id.clone());
            state.trace_reveal_until = 3;
        } else {
            naval_server::sim::powerups::activate(
                &mut room.world,
                &actor.ship_id,
                powerup,
                &mut room.rng,
            )
            .unwrap();
        }
        let mut detected = Vec::new();
        for tick in 0..4 {
            room.inject_replay_command(&a.bot_id, command(tick));
            room.step_tick();
            let msg = a.outbound.try_recv().unwrap();
            if let ServerMsg::Tick {
                contacts,
                self_state,
                ..
            } = msg
            {
                detected.push(!contacts.is_empty());
                if powerup == PowerupId::EmpBurst {
                    let status = &self_state.powerup_status[0];
                    assert!(!status.used);
                    assert_eq!(status.active_ticks_left, 0);
                }
            } else {
                panic!("expected tick");
            }
        }
        let expected = if far {
            vec![true, true, true, false]
        } else {
            vec![false, false, false, true]
        };
        assert_eq!(detected, expected, "effect {powerup:?}");
    }
}

#[test]
fn activation_events_use_visible_contact_ids_and_respect_awacs_stealth() {
    for distance in [100., 500.] {
        let mut room = room();
        let mut a = bot(&mut room, "alpha");
        let b = bot(&mut room, "bravo");
        start(&mut room);
        a.outbound.try_recv().unwrap();
        room.world.ships.get_mut(&a.ship_id).unwrap().pos = Vec2::new(100., 100.);
        room.world
            .ships
            .get_mut(&a.ship_id)
            .unwrap()
            .powerups
            .awacs_expires_at = 100;
        let target = room.world.ships.get_mut(&b.ship_id).unwrap();
        target.pos = Vec2::new(100. + distance, 100.);
        target.powerups.silent_running_expires_at = 100;
        target.powerups.selected = vec![PowerupId::Overdrive];
        room.inject_replay_command(&a.bot_id, command(0));
        let mut activation = command(0);
        activation.activate_powerup = Some(PowerupId::Overdrive);
        room.inject_replay_command(&b.bot_id, activation);
        room.step_tick();
        let message = a.outbound.try_recv().unwrap();
        let wire = serde_json::to_string(&message).unwrap();
        assert!(!wire.contains(&b.ship_id));
        if let ServerMsg::Tick {
            contacts, events, ..
        } = message
        {
            let event = events.iter().find_map(|e| match e {
                TickEvent::PowerupActivated {
                    own, contact_id, ..
                } => Some((*own, contact_id)),
                _ => None,
            });
            if distance == 100. {
                let (own, id) = event.expect("visible event");
                assert!(!own);
                assert!(contacts.iter().any(|c| Some(&c.id) == id.as_ref()));
            } else {
                assert!(contacts.is_empty());
                assert!(event.is_none());
            }
        }
    }
}

#[test]
fn unique_replay_names_and_exclusive_creation_preserve_existing_bytes() {
    let first = replay::make_replay_id("test");
    assert_ne!(first, replay::make_replay_id("test"));
    let dir = std::env::temp_dir().join(first.clone());
    let writer = ReplayWriter::create_file(&dir, first.clone()).unwrap();
    let path = writer.path().unwrap().to_owned();
    drop(writer);
    std::fs::write(&path, b"keep me").unwrap();
    assert_eq!(
        ReplayWriter::create_file(&dir, first).unwrap_err().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"keep me");
    std::fs::remove_dir_all(dir).unwrap();
}

fn header() -> ReplayHeader {
    ReplayHeader {
        version: REPLAY_FORMAT_VERSION,
        replay_id: "test".into(),
        room: "test".into(),
        seed: 42,
        tick_hz: 10,
        tick_deadline_ms: 80,
        map: MapInfo {
            width: 700,
            height: 700,
        },
        max_bots: 24,
        sim_config: SimConfig::default(),
        bots: vec![ReplayBot {
            bot_id: "b_1".into(),
            ship_id: "s_1".into(),
            name: "alpha".into(),
            selected_powerups: vec![],
            spawn_pos: [350., 350.],
            spawn_heading_deg: 0.,
        }],
    }
}
#[test]
fn huge_replay_ticks_are_rejected_before_simulation() {
    let before = Instant::now();
    let records = vec![
        ReplayRecord::Header(Box::new(header())),
        ReplayRecord::End(ReplayEnd {
            outcome: None,
            end_reason: None,
            tick: u64::MAX,
            winner: None,
        }),
    ];
    assert!(matches!(
        replay::capture_replay(records.clone()),
        Err(replay::ReplayError::Header(_))
    ));
    assert!(matches!(
        replay::capture_perspective(records, "b_1"),
        Err(replay::ReplayError::Header(_))
    ));
    assert!(before.elapsed() < Duration::from_secs(1));
}

#[tokio::test(start_paused = true)]
async fn replay_with_silent_ticks_is_paced_one_simulation_step_at_a_time() {
    let dir = std::env::temp_dir().join(replay::make_replay_id("pace"));
    let mut writer = ReplayWriter::create_file(&dir, "pace".into()).unwrap();
    writer
        .write(&ReplayRecord::Header(Box::new(header())))
        .unwrap();
    writer
        .write(&ReplayRecord::End(ReplayEnd {
            outcome: None,
            end_reason: None,
            tick: 10,
            winner: None,
        }))
        .unwrap();
    let path = writer.path().unwrap().to_owned();
    drop(writer);
    let (tx, mut rx) = broadcast::channel(32);
    let (_shutdown, shutdown_rx) = broadcast::channel(1);
    let task = tokio::spawn(replay::run_replay(path, tx, shutdown_rx));
    let first: serde_json::Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
    assert_eq!(first["tick"], 1);
    assert!(
        rx.try_recv().is_err(),
        "silent ticks must not be emitted in a burst"
    );
    tokio::time::advance(Duration::from_millis(90)).await;
    assert!(rx.try_recv().is_err());
    tokio::time::advance(Duration::from_millis(10)).await;
    let second: serde_json::Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
    assert_eq!(second["tick"], 2);
    task.await.unwrap().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test(start_paused = true)]
async fn ready_message_flood_cannot_starve_tick_timer() {
    let room = room();
    let (tx, rx) = mpsc::channel(256);
    let (shutdown, shutdown_rx) = broadcast::channel(1);
    let runner = tokio::spawn(room::run_room(room, rx, shutdown_rx));
    let flood_tx = tx.clone();
    let flood = tokio::spawn(async move {
        loop {
            if flood_tx
                .send(RoomEvent::BotReady {
                    bot_id: "missing".into(),
                })
                .await
                .is_err()
            {
                break;
            }
        }
    });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    let (reply, response) = oneshot::channel();
    tx.send(RoomEvent::QueryState { reply }).await.unwrap();
    let snapshot = response.await.unwrap();
    assert!(
        snapshot.state.tick >= 2,
        "timer must advance despite a ready event queue"
    );
    flood.abort();
    shutdown.send(()).unwrap();
    runner.await.unwrap();
}

#[test]
fn lockstep_accepts_commands_after_the_normal_deadline() {
    let mut room = room();
    let mut a = bot(&mut room, "alpha");
    let _b = bot(&mut room, "bravo");
    mc(&mut room);
    a.outbound.try_recv().unwrap();
    room.step_tick();
    match a.outbound.try_recv().unwrap() {
        ServerMsg::Tick { deadline_ms, .. } => assert_eq!(deadline_ms, 1000),
        other => panic!("expected tick: {other:?}"),
    }
    // Both the advertised deadline and acceptance use the lockstep budget.
    // Wall-clock deadline belongs to orchestration, not simulation state.
    std::thread::sleep(Duration::from_millis(120));
    let mut cmd = command(room.world.tick);
    cmd.throttle = 1.;
    room.handle_event(RoomEvent::BotCommand {
        bot_id: a.bot_id.clone(),
        command: cmd,
    });
    room.step_tick();
    assert!(room.world.ships.get(&a.ship_id).unwrap().speed > 0.);
    assert_eq!(room.state, RoomState::Running);
}
