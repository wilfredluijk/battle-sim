use naval_server::{
    ingress::CommandSlot,
    protocol::{SensorMode, ServerMsg},
    room::{BotRegistration, PendingCommand, Room, RoomEvent, MATCH_TIMEOUT_TICKS},
    sim::PowerupId,
};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

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
fn bot(room: &mut Room, name: &str) -> BotRegistration {
    let (reply, mut rx) = oneshot::channel();
    room.handle_event(RoomEvent::BotConnect {
        peer: "127.0.0.1:1234".parse().unwrap(),
        name: name.into(),
        version: "3.0".into(),
        reply,
    });
    let reg = rx.try_recv().unwrap().unwrap();
    room.handle_event(RoomEvent::BotReadyChecked {
        bot_id: reg.bot_id.clone(),
        config_hash: room.config_hash(),
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
fn room() -> Room {
    Room::new("test".into(), 700., 700., 42, 10, 80, 8)
}

#[test]
fn ingress_is_exact_first_wins_and_uses_completed_message_time() {
    let slot = CommandSlot::default();
    let cutoff = Instant::now() + Duration::from_millis(10);
    slot.open("match-a", 4, cutoff);
    assert_eq!(slot.submit("old", command(4), cutoff), Err("wrong_match"));
    assert_eq!(
        slot.submit("match-a", command(3), cutoff),
        Err("wrong_tick")
    );
    assert_eq!(
        slot.submit("match-a", command(5), cutoff),
        Err("wrong_tick")
    );
    assert_eq!(
        slot.submit("match-a", command(4), cutoff + Duration::from_nanos(1)),
        Err("late_command")
    );
    // Processing may happen later; admission uses the completed-message timestamp.
    std::thread::sleep(Duration::from_millis(15));
    assert_eq!(slot.submit("match-a", command(4), cutoff), Ok(()));
    assert_eq!(
        slot.submit("match-a", command(4), cutoff),
        Err("duplicate_command")
    );
    assert_eq!(slot.close().unwrap().tick, 4);
    assert_eq!(
        slot.submit("match-a", command(4), cutoff),
        Err("not_active")
    );
    slot.open("match-b", 4, Instant::now());
    assert_eq!(
        slot.submit("match-a", command(4), cutoff),
        Err("wrong_match")
    );
}

#[test]
fn changed_configuration_invalidates_previous_acknowledgement() {
    let mut room = room();
    let mut b = bot(&mut room, "one");
    let old = room.config_hash();
    b.outbound.try_recv().unwrap();
    let mut config = room.world.config;
    config.hull_hp += 1;
    let (reply, mut rx) = oneshot::channel();
    room.handle_event(RoomEvent::OperatorConfigure { config, reply });
    rx.try_recv().unwrap().unwrap();
    assert!(!room.all_ready());
    let ServerMsg::Configuration {
        config_hash,
        configuration,
    } = b.outbound.try_recv().unwrap()
    else {
        panic!("configuration");
    };
    assert_ne!(old, config_hash);
    assert_eq!(configuration["sim_config"]["hull_hp"], config.hull_hp);
    room.handle_event(RoomEvent::BotReadyChecked {
        bot_id: b.bot_id.clone(),
        config_hash: old,
    });
    assert!(!room.all_ready());
    room.handle_event(RoomEvent::BotReadyChecked {
        bot_id: b.bot_id,
        config_hash,
    });
    assert!(room.all_ready());
}

#[test]
fn forfeit_retains_hull_and_owner_and_eliminated_bots_stop_scouting() {
    let mut room = room();
    let mut a = bot(&mut room, "one");
    let _b = bot(&mut room, "two");
    let c = bot(&mut room, "three");
    start(&mut room);
    room.step_tick();
    while a.outbound.try_recv().is_ok() {}
    room.world.ships.get_mut(&a.ship_id).unwrap().alive = false;
    room.step_tick();
    assert!(a.outbound.try_recv().is_err());
    assert_eq!(
        a.ingress
            .submit("anything", command(room.world.tick), Instant::now()),
        Err("not_active")
    );
    room.forfeit_bot(&c.bot_id);
    let ship = &room.world.ships[&c.ship_id];
    assert!(!ship.alive);
    assert_eq!(ship.hp, 0);
    assert_eq!(ship.bot_id, c.bot_id);
    assert_eq!(room.bot_count(), 3);
}

#[test]
fn equal_timeout_scores_are_a_draw() {
    let mut room = room();
    let mut a = bot(&mut room, "one");
    let _b = bot(&mut room, "two");
    start(&mut room);
    room.world.tick = MATCH_TIMEOUT_TICKS - 1;
    room.step_tick();
    let mut winner = Some("not-ended".into());
    while let Ok(msg) = a.outbound.try_recv() {
        if let ServerMsg::GameOver { winner: w, .. } = msg {
            winner = w;
        }
    }
    assert_eq!(winner, None);
}

#[test]
fn simultaneous_emp_applies_before_all_shots() {
    fn cooldown(emp_first: bool) -> u32 {
        let mut room = room();
        let a = bot(&mut room, "a");
        let b = bot(&mut room, "b");
        let (emp, shooter) = if emp_first { (&a, &b) } else { (&b, &a) };
        room.handle_event(RoomEvent::BotSelectPowerups {
            bot_id: emp.bot_id.clone(),
            powerups: vec![PowerupId::EmpBurst, PowerupId::RepairDrones],
        });
        start(&mut room);
        room.world.ships.get_mut(&a.ship_id).unwrap().pos = glam::Vec2::new(300., 300.);
        room.world.ships.get_mut(&b.ship_id).unwrap().pos = glam::Vec2::new(310., 300.);
        let mut e = command(0);
        e.activate_powerup = Some(PowerupId::EmpBurst);
        let mut f = command(0);
        f.fire = Some(naval_server::protocol::FireCommand {
            bearing_deg: 0.,
            range: 200.,
        });
        room.inject_replay_command(&emp.bot_id, e);
        room.inject_replay_command(&shooter.bot_id, f);
        room.step_tick();
        room.world.ships[&shooter.ship_id].gun_cooldown
    }
    assert_eq!(cooldown(true), cooldown(false));
}

#[test]
fn tournament_starts_get_fresh_seeds_and_hidden_random_layouts() {
    let mut seeds = Vec::new();
    for _ in 0..2 {
        let mut room = room();
        room.tournament = true;
        let _bots: Vec<_> = (0..8)
            .map(|i| bot(&mut room, &format!("player{i}")))
            .collect();
        start(&mut room);
        seeds.push(room.seed);
        let ships: Vec<_> = room.world.ships.values().collect();
        for (i, a) in ships.iter().enumerate() {
            for b in &ships[i + 1..] {
                assert!(a.pos.distance(b.pos) >= 80.);
            }
        }
    }
    assert_ne!(seeds[0], seeds[1]);
    assert!(!seeds.contains(&42));
}

#[test]
fn kick_followed_by_socket_cleanup_records_one_replayable_forfeit() {
    use naval_server::replay::{self, ReplayRecord, ReplayWriter};
    let mut room = room();
    let a = bot(&mut room, "one");
    let _b = bot(&mut room, "two");
    let _c = bot(&mut room, "three");
    let (writer, bytes) = ReplayWriter::in_memory("kick-replay".into());
    room.set_replay_writer(writer);
    start(&mut room);
    let (reply, mut rx) = oneshot::channel();
    room.handle_event(RoomEvent::OperatorKick {
        bot_id: a.bot_id.clone(),
        reply,
    });
    rx.try_recv().unwrap().unwrap();
    room.handle_event(RoomEvent::BotDisconnect { bot_id: a.bot_id });
    room.step_tick();
    let (reply, mut rx) = oneshot::channel();
    room.handle_event(RoomEvent::OperatorAbort { reply });
    rx.try_recv().unwrap().unwrap();
    let records =
        replay::read_records_from(std::io::Cursor::new(bytes.lock().unwrap().clone())).unwrap();
    assert_eq!(
        records
            .iter()
            .filter(|r| matches!(r, ReplayRecord::Disconnect(_)))
            .count(),
        1
    );
    replay::capture_replay(records).unwrap();
}

#[test]
fn training_history_preserves_named_rounds_reports_and_diagnostics_across_lobby_and_restart() {
    use naval_server::training::{TrainingRequest, TrainingStore};
    let dir = std::env::temp_dir().join(format!(
        "naval-session-integration-{}",
        naval_server::replay::unique_suffix()
    ));
    let mut room = room();
    room.set_replay_dir(dir.clone());
    let a = bot(&mut room, "Atlas");
    let _b = bot(&mut room, "Echo");
    let request: TrainingRequest = serde_json::from_value(serde_json::json!({"revision":0,"action":"create","name":"Workshop","expected_teams":["Atlas","Echo","Absent"],"scoring":{"win":3,"draw":1,"loss":0}})).unwrap();
    let (reply, mut rx) = oneshot::channel();
    room.handle_event(RoomEvent::UpdateTraining { request, reply });
    rx.try_recv().unwrap().unwrap();
    start(&mut room);
    room.step_tick();
    let snapshot = room.snapshot();
    assert_eq!(snapshot.round.as_ref().unwrap().name, "Round 1");
    let sent = Instant::now();
    a.ingress.sent(room.world.tick, sent);
    assert!(a
        .ingress
        .submit(&snapshot.match_id, command(room.world.tick), sent)
        .is_ok());
    room.step_tick();
    let (reply, mut rx) = oneshot::channel();
    room.handle_event(RoomEvent::OperatorAbort { reply });
    rx.try_recv().unwrap().unwrap();
    let (reply, mut rx) = oneshot::channel();
    room.handle_event(RoomEvent::OperatorReset { reply });
    rx.try_recv().unwrap().unwrap();
    let restored = TrainingStore::load(dir.join("training-history.json"));
    let saved = restored.active().unwrap();
    assert_eq!(saved.next_round, "Round 2");
    assert_eq!(saved.rounds.len(), 1);
    let report = saved.rounds[0].report.as_ref().unwrap();
    assert_eq!(report.outcome, "aborted");
    assert_eq!(report.match_id, snapshot.match_id);
    assert_eq!(
        report
            .bots
            .iter()
            .find(|b| b.name == "Atlas")
            .unwrap()
            .diagnostics
            .accepted,
        1
    );
    assert_eq!(room.snapshot().session_expected_teams.unwrap().len(), 3);
    drop(room);
    std::fs::remove_dir_all(dir).unwrap();
}
