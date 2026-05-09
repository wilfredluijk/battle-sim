//! Wire protocol types. Every message that crosses the WebSocket boundary lives here.
//!
//! The shape of these types is the public contract with bot authors — keep them in sync
//! with `docs/PROTOCOL.md`. Internal simulation types should not leak into this module.

use serde::{Deserialize, Serialize};

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
    /// Spec values from `system-design.md` §5.2 and §5.4. Used in the `welcome` payload.
    pub const DEFAULT: ShipSpecs = ShipSpecs {
        max_forward_speed: 6.0,
        max_reverse_speed: 2.0,
        acceleration: 1.5,
        turn_rate_deg_per_s: 15.0,
        hull_hp: 100,
        max_ammo: 20,
        gun_cooldown_ticks: 15,
        hit_radius: 8.0,
        shell_speed: 50.0,
        max_shell_range: 300.0,
        splash_radius: 15.0,
        max_splash_damage: 25,
    };
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct SelfState {
    pub pos: Pos,
    pub heading_deg: f32,
    pub speed: f32,
    pub hp: u32,
    pub ammo: u32,
    pub rudder: f32,
    pub throttle: f32,
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
    Hit { amount: u32 },
    ShellSplash { pos: Pos },
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
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SpectatorShip {
    pub id: String,
    pub bot_name: String,
    pub pos: Pos,
    pub heading_deg: f32,
    pub hp: u32,
    pub alive: bool,
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
        });
        roundtrip(&BotMsg::Command {
            tick: 143,
            throttle: 0.0,
            rudder: 0.0,
            fire: None,
            sensor_mode: SensorMode::Passive,
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
            ship_specs: ShipSpecs::DEFAULT,
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
                hp: 78,
                alive: true,
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
            ],
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
}
