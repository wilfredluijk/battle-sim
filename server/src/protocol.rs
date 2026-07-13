//! Wire protocol types. Every message that crosses the WebSocket boundary lives here.
//!
//! The shape of these types is the public contract with bot authors — keep them in sync
//! with `docs/PROTOCOL.md`. Internal simulation types should not leak into this module.

use serde::{Deserialize, Serialize};

use crate::sim::PowerupId;

/// 2D position / velocity, serialized as a 2-element JSON array `[x, y]`.
pub type Pos = [f32; 2];
pub type Vel = [f32; 2];

// ---------------------------------------------------------------------------
// Bot -> Server
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BotMsg {
    Hello {
        name: String,
        version: String,
    },
    Ready,
    Command {
        tick: u64,
        throttle: f32,
        rudder: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fire: Option<FireCommand>,
        sensor_mode: SensorMode,
        /// One-off powerup activation for this tick. Server validates that the bot picked
        /// the powerup before the match and has not yet used it; an invalid id earns a
        /// typed `error` frame.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        activate_powerup: Option<PowerupId>,
    },
    /// Declare the (up to 2 distinct) powerups the bot will use for this match. May only
    /// be sent while the room is in `lobby` and before `ready`. Sending it twice replaces
    /// the previous selection; an invalid loadout earns a typed `error` frame and leaves
    /// the previous selection (if any) intact.
    SelectPowerups {
        powerups: Vec<PowerupId>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct FireCommand {
    pub bearing_deg: f32,
    pub range: f32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SensorMode {
    Active,
    Passive,
}

// ---------------------------------------------------------------------------
// Server -> Bot
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Welcome {
        bot_id: String,
        ship_id: String,
        map: MapInfo,
        tick_hz: u32,
        ship_specs: ShipSpecs,
        /// Catalog of powerup ids this server understands. Bots use this to discover what
        /// they may pass to `select_powerups`. Forward-compatible: future servers may add
        /// entries; SDKs that don't recognise an id should leave it untouched.
        available_powerups: Vec<PowerupId>,
    },
    GameStart {
        tick: u64,
        starting_position: Pos,
        starting_heading_deg: f32,
    },
    Tick {
        tick: u64,
        deadline_ms: u64,
        #[serde(rename = "self")]
        self_state: SelfState,
        contacts: Vec<Contact>,
        events: Vec<TickEvent>,
    },
    GameOver {
        winner: Option<String>,
        final_tick: u64,
        replay_id: String,
    },
    /// Emitted by the room when it transitions back to the lobby after a match. Bots
    /// that wish to participate in the next match should re-send `ready`. `tick` is
    /// always `0` and is included only to give the message a numeric field for parity
    /// with `GameStart`.
    Lobby {
        tick: u64,
    },
    Error {
        code: String,
        message: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapInfo {
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct ShipSpecs {
    pub max_forward_speed: f32,
    pub max_reverse_speed: f32,
    pub acceleration: f32,
    pub turn_rate_deg_per_s: f32,
    pub hull_hp: u32,
    pub max_ammo: u32,
    pub gun_cooldown_ticks: u32,
    pub hit_radius: f32,
    pub shell_speed: f32,
    pub max_shell_range: f32,
    pub splash_radius: f32,
    pub max_splash_damage: u32,
}

impl ShipSpecs {
    /// Derive the `welcome`-payload spec from a match's [`SimConfig`]. Bots base their
    /// physics models on what we send here, so this must always reflect the parameters the
    /// simulation will actually run with.
    pub fn from_config(config: &crate::sim::SimConfig) -> ShipSpecs {
        ShipSpecs {
            max_forward_speed: config.max_forward_speed,
            max_reverse_speed: config.max_reverse_speed,
            acceleration: config.acceleration,
            turn_rate_deg_per_s: config.turn_rate_deg_per_s,
            hull_hp: config.hull_hp,
            max_ammo: config.max_ammo,
            gun_cooldown_ticks: config.gun_cooldown_ticks,
            hit_radius: config.hit_radius,
            shell_speed: config.shell_speed,
            max_shell_range: config.max_shell_range,
            splash_radius: config.splash_radius,
            max_splash_damage: config.max_splash_damage,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SelfState {
    pub pos: Pos,
    pub heading_deg: f32,
    pub speed: f32,
    pub hp: u32,
    pub ammo: u32,
    pub rudder: f32,
    pub throttle: f32,
    /// Loadout the bot picked for the match (in pick order, up to 2 entries). Sent on
    /// every tick so a bot reconnecting after a brief drop can rehydrate without state.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_powerups: Vec<PowerupId>,
    /// One entry per picked powerup, in the same order as `selected_powerups`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub powerup_status: Vec<PowerupStatus>,
}

/// Live status for one of the bot's picked powerups. Sent inside `tick.self.powerup_status`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct PowerupStatus {
    pub id: PowerupId,
    /// True once the bot has activated this powerup; activating a used powerup is rejected.
    pub used: bool,
    /// Ticks remaining of the active effect. `0` when not currently active.
    pub active_ticks_left: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Contact {
    pub id: String,
    pub kind: ContactKind,
    pub pos: Pos,
    pub bearing_deg: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<f32>,
    pub confidence: f32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContactKind {
    Ship,
    Shell,
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TickEvent {
    Hit {
        amount: u32,
    },
    ShellSplash {
        pos: Pos,
    },
    /// A powerup was activated. Sent to the activating bot for every own activation
    /// (`contact_id` is `null` — you are not a contact to yourself), and to other bots
    /// only when the activating ship actually appeared in that viewer's sensor sweep this
    /// tick — tagged with the same per-tick anonymized `c_<n>` contact id the bot sees in
    /// `contacts`. Never carries a ground-truth `ship_id`: like contacts, the event is
    /// re-anonymized every tick so a bot can't track a specific opponent across ticks.
    /// Counter-battery trace reveals come in via regular `contacts`, not this event.
    PowerupActivated {
        #[serde(skip_serializing_if = "Option::is_none")]
        contact_id: Option<String>,
        powerup: PowerupId,
    },
}

// ---------------------------------------------------------------------------
// Server -> Spectator
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SpectatorMsg {
    World {
        tick: u64,
        ships: Vec<SpectatorShip>,
        shells: Vec<SpectatorShell>,
        events: Vec<SpectatorEvent>,
        /// Live smoke clouds on the battlefield. Empty when no `smoke_screen` is active.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        smoke_clouds: Vec<SpectatorSmokeCloud>,
        /// Live decoy phantoms.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        decoys: Vec<SpectatorDecoy>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SpectatorShip {
    pub id: String,
    pub bot_name: String,
    pub pos: Pos,
    pub heading_deg: f32,
    /// Signed scalar speed: positive = ahead, negative = reverse.
    pub speed: f32,
    pub hp: u32,
    pub ammo: u32,
    /// Last commanded throttle in `[-1, 1]`.
    pub throttle: f32,
    /// Last commanded rudder in `[-1, 1]`.
    pub rudder: f32,
    pub alive: bool,
    /// Lobby readiness flag. `true` once the bot has sent `ready`.
    pub ready: bool,
    /// Commands accepted by the room over the most recent rolling 1-second window
    /// (1s of *sim* time, i.e. `tick_hz` ticks). Stays 0 while the room is in lobby.
    pub commands_per_sec: f32,
    /// Last commanded sensor mode. Used by the renderer to draw an active-radar ring.
    pub sensor_mode: SensorMode,
    /// Powerups this bot picked for the match (in pick order).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_powerups: Vec<PowerupId>,
    /// Per-pick status for the spectator HUD: which are used, which are active, and how
    /// many ticks of effect remain.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub powerup_status: Vec<PowerupStatus>,
}

/// One smoke cloud on the spectator wire.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct SpectatorSmokeCloud {
    pub pos: Pos,
    pub radius: f32,
    pub expires_at: u64,
}

/// One decoy phantom on the spectator wire.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SpectatorDecoy {
    pub fake_id: u32,
    pub owner: String,
    pub pos: Pos,
    pub heading_deg: f32,
    pub expires_at: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct SpectatorShell {
    pub id_index: u32,
    pub pos: Pos,
    pub vel: Vel,
    pub ttl_ticks: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SpectatorEvent {
    Hit { ship_id: String, amount: u32 },
    ShellSplash { pos: Pos },
    Death { ship_id: String },
    PowerupActivated { ship_id: String, powerup: PowerupId },
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Standardized error codes sent inside `ServerMsg::Error`. Keep this list small and
/// document each code in `docs/PROTOCOL.md`.
pub mod error_code {
    pub const MALFORMED_JSON: &str = "malformed_json";
    pub const INVALID_MESSAGE: &str = "invalid_message";
    pub const BINARY_FRAMES_UNSUPPORTED: &str = "binary_frames_unsupported";
    pub const TOO_MANY_VIOLATIONS: &str = "too_many_violations";
    pub const LATE_COMMAND: &str = "late_command";
    pub const COOLDOWN_ACTIVE: &str = "cooldown_active";
    pub const NO_AMMO: &str = "no_ammo";
    pub const INVALID_NAME: &str = "invalid_name";
    pub const DUPLICATE_NAME: &str = "duplicate_name";
    pub const STALE_COMMAND: &str = "stale_command";
    pub const NON_FINITE_VALUE: &str = "non_finite_value";
    pub const HANDSHAKE_TIMEOUT: &str = "handshake_timeout";
    pub const CONNECTION_LIMIT: &str = "connection_limit";
    // --- Powerups ----------------------------------------------------------
    pub const POWERUP_UNKNOWN: &str = "powerup_unknown";
    pub const POWERUP_DUPLICATE: &str = "powerup_duplicate";
    pub const POWERUP_WRONG_COUNT: &str = "powerup_wrong_count";
    pub const POWERUP_LOBBY_ONLY: &str = "powerup_lobby_only";
    pub const POWERUP_NOT_SELECTED: &str = "powerup_not_selected";
    pub const POWERUP_ALREADY_USED: &str = "powerup_already_used";
}

/// Maximum length of a bot's `hello.name`. Names are also restricted to
/// `[A-Za-z0-9 _-]`; see [`validate_bot_name`].
pub const MAX_BOT_NAME_LEN: usize = 32;

/// Returns `Ok(())` if `name` is a valid bot identifier:
/// - 1..=`MAX_BOT_NAME_LEN` characters,
/// - every byte is ASCII alphanumeric, space, underscore, or hyphen.
///
/// Restricting the charset keeps replay logs greppable and stops spectator-UI injection
/// via unicode controls / RTL overrides.
pub fn validate_bot_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("name must not be empty");
    }
    if name.len() > MAX_BOT_NAME_LEN {
        return Err("name exceeds 32 bytes");
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b' ' || b == b'_' || b == b'-')
    {
        return Err("name may only contain A-Z, a-z, 0-9, space, underscore, or hyphen");
    }
    Ok(())
}

/// Build a `ServerMsg::Error`.
pub fn error_msg(code: &str, message: impl Into<String>) -> ServerMsg {
    ServerMsg::Error {
        code: code.to_string(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<T>(value: &T)
    where
        T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).expect("serialize");
        let parsed: T = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(value, &parsed, "roundtrip mismatch: {json}");
    }

    #[test]
    fn bot_msg_roundtrips() {
        roundtrip(&BotMsg::Hello {
            name: "captain_kirk".into(),
            version: "1.0".into(),
        });
        roundtrip(&BotMsg::Ready);
        roundtrip(&BotMsg::Command {
            tick: 142,
            throttle: 0.8,
            rudder: -0.3,
            fire: Some(FireCommand {
                bearing_deg: 47.5,
                range: 300.0,
            }),
            sensor_mode: SensorMode::Active,
            activate_powerup: None,
        });
        roundtrip(&BotMsg::Command {
            tick: 143,
            throttle: 0.0,
            rudder: 0.0,
            fire: None,
            sensor_mode: SensorMode::Passive,
            activate_powerup: Some(PowerupId::Overdrive),
        });
        roundtrip(&BotMsg::SelectPowerups {
            powerups: vec![PowerupId::RapidFire, PowerupId::HeavyShell],
        });
    }

    #[test]
    fn server_msg_roundtrips() {
        roundtrip(&ServerMsg::Welcome {
            bot_id: "b_3".into(),
            ship_id: "s_3".into(),
            map: MapInfo {
                width: 1000,
                height: 1000,
            },
            tick_hz: 10,
            ship_specs: ShipSpecs::from_config(&crate::sim::SimConfig::default()),
            available_powerups: PowerupId::all().to_vec(),
        });
        roundtrip(&ServerMsg::GameStart {
            tick: 0,
            starting_position: [120.0, 340.0],
            starting_heading_deg: 90.0,
        });
        roundtrip(&ServerMsg::Tick {
            tick: 142,
            deadline_ms: 80,
            self_state: SelfState {
                pos: [203.4, 511.7],
                heading_deg: 92.3,
                speed: 4.1,
                hp: 78,
                ammo: 14,
                rudder: -0.3,
                throttle: 0.8,
                selected_powerups: vec![PowerupId::Overdrive, PowerupId::RapidFire],
                powerup_status: vec![
                    PowerupStatus {
                        id: PowerupId::Overdrive,
                        used: false,
                        active_ticks_left: 0,
                    },
                    PowerupStatus {
                        id: PowerupId::RapidFire,
                        used: true,
                        active_ticks_left: 12,
                    },
                ],
            },
            contacts: vec![Contact {
                id: "c_a1".into(),
                kind: ContactKind::Ship,
                pos: [450.0, 510.0],
                bearing_deg: 88.0,
                range: Some(247.0),
                confidence: 0.85,
            }],
            events: vec![
                TickEvent::Hit { amount: 12 },
                TickEvent::ShellSplash {
                    pos: [220.0, 505.0],
                },
            ],
        });
        roundtrip(&ServerMsg::GameOver {
            winner: Some("b_3".into()),
            final_tick: 1843,
            replay_id: "match_20260508_171203".into(),
        });
        roundtrip(&ServerMsg::GameOver {
            winner: None,
            final_tick: 3000,
            replay_id: "match_draw".into(),
        });
        roundtrip(&error_msg(
            error_code::LATE_COMMAND,
            "command for tick 142 arrived after deadline",
        ));
    }

    #[test]
    fn spectator_msg_roundtrips() {
        roundtrip(&SpectatorMsg::World {
            tick: 142,
            ships: vec![SpectatorShip {
                id: "s_1".into(),
                bot_name: "captain_kirk".into(),
                pos: [203.4, 511.7],
                heading_deg: 92.3,
                speed: 4.1,
                hp: 78,
                ammo: 14,
                throttle: 0.8,
                rudder: -0.3,
                alive: true,
                ready: true,
                commands_per_sec: 10.0,
                sensor_mode: SensorMode::Active,
                selected_powerups: vec![PowerupId::SmokeScreen, PowerupId::AwacsScan],
                powerup_status: vec![
                    PowerupStatus {
                        id: PowerupId::SmokeScreen,
                        used: true,
                        active_ticks_left: 0,
                    },
                    PowerupStatus {
                        id: PowerupId::AwacsScan,
                        used: false,
                        active_ticks_left: 0,
                    },
                ],
            }],
            shells: vec![SpectatorShell {
                id_index: 22,
                pos: [310.0, 500.0],
                vel: [40.0, 5.0],
                ttl_ticks: 18,
            }],
            events: vec![
                SpectatorEvent::Hit {
                    ship_id: "s_1".into(),
                    amount: 12,
                },
                SpectatorEvent::ShellSplash {
                    pos: [220.0, 505.0],
                },
                SpectatorEvent::Death {
                    ship_id: "s_2".into(),
                },
                SpectatorEvent::PowerupActivated {
                    ship_id: "s_1".into(),
                    powerup: PowerupId::SmokeScreen,
                },
            ],
            smoke_clouds: vec![SpectatorSmokeCloud {
                pos: [203.4, 511.7],
                radius: 60.0,
                expires_at: 222,
            }],
            decoys: vec![],
        });
    }

    #[test]
    fn parses_canonical_command_from_spec() {
        // Lifted verbatim from system-design.md §4.1
        let json = r#"{
            "type": "command",
            "tick": 142,
            "throttle": 0.8,
            "rudder": -0.3,
            "fire": { "bearing_deg": 47.5, "range": 300.0 },
            "sensor_mode": "active"
        }"#;
        let parsed: BotMsg = serde_json::from_str(json).expect("parse");
        match parsed {
            BotMsg::Command {
                tick,
                throttle,
                rudder,
                fire,
                sensor_mode,
                activate_powerup,
            } => {
                assert_eq!(tick, 142);
                assert!((throttle - 0.8).abs() < 1e-6);
                assert!((rudder - -0.3).abs() < 1e-6);
                assert_eq!(
                    fire,
                    Some(FireCommand {
                        bearing_deg: 47.5,
                        range: 300.0
                    })
                );
                assert_eq!(sensor_mode, SensorMode::Active);
                assert!(activate_powerup.is_none());
            }
            other => panic!("expected Command, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_message_type() {
        let json = r#"{"type":"explode_now"}"#;
        let result: Result<BotMsg, _> = serde_json::from_str(json);
        assert!(result.is_err(), "should reject unknown message type");
    }

    #[test]
    fn rejects_empty_object() {
        let result: Result<BotMsg, _> = serde_json::from_str("{}");
        assert!(result.is_err(), "empty object has no `type` discriminant");
    }

    #[test]
    fn validate_bot_name_accepts_typical_names() {
        for ok in ["alice", "Bot_42", "captain kirk", "ship-1", "A"] {
            assert!(validate_bot_name(ok).is_ok(), "should accept {ok:?}");
        }
    }

    #[test]
    fn validate_bot_name_rejects_garbage() {
        assert!(validate_bot_name("").is_err(), "empty name rejected");
        let too_long = "x".repeat(MAX_BOT_NAME_LEN + 1);
        assert!(
            validate_bot_name(&too_long).is_err(),
            "33-byte name rejected"
        );
        for bad in ["alice\n", "bob\t", "🛳️ ship", "<script>", "../etc/passwd"] {
            assert!(validate_bot_name(bad).is_err(), "should reject {bad:?}");
        }
    }
}
