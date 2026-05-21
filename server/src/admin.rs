//! Admin state types shared between the room and the REST control plane.
//!
//! Trust model: admin actions arrive over `/api/*` routes that have already been
//! authenticated by the JWT layer (see [`crate::auth`]). The room re-validates by being
//! the single owner of the lifecycle state — it can refuse a reset outside `Ended`, etc.

use serde::{Deserialize, Serialize};

/// Compare two byte strings in constant time relative to the matching prefix length.
/// Returns false immediately on length mismatch (the token length itself is not secret).
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// State types
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdminServerMsg {
    State(AdminState),
    Ack { command: String },
    Error { code: String, message: String },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AdminState {
    pub room: String,
    pub state: String,
    pub tick: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_winner: Option<String>,
    pub bots: Vec<AdminBotInfo>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AdminBotInfo {
    pub bot_id: String,
    pub name: String,
    pub ship_id: String,
    pub ready: bool,
    pub alive: bool,
}

pub mod admin_error_code {
    pub const NOT_RUNNING: &str = "not_running";
    pub const NOT_ENDED: &str = "not_ended";
    pub const UNKNOWN_BOT: &str = "unknown_bot";
    pub const MALFORMED_JSON: &str = "malformed_json";
    pub const INVALID_MESSAGE: &str = "invalid_message";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches_string_eq() {
        assert!(constant_time_eq("alpha", "alpha"));
        assert!(!constant_time_eq("alpha", "alpha "));
        assert!(!constant_time_eq("alpha", "beta"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn admin_server_msg_roundtrips() {
        let state = AdminServerMsg::State(AdminState {
            room: "main".into(),
            state: "lobby".into(),
            tick: 0,
            last_winner: Some("b_1".into()),
            bots: vec![AdminBotInfo {
                bot_id: "b_1".into(),
                name: "alice".into(),
                ship_id: "s_1".into(),
                ready: true,
                alive: true,
            }],
        });
        let json = serde_json::to_string(&state).unwrap();
        let parsed: AdminServerMsg = serde_json::from_str(&json).unwrap();
        assert_eq!(state, parsed);
    }
}
