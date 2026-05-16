//! Admin lifecycle plane. Types for the gated `/admin` WebSocket and helpers for
//! generating / comparing the rotating password the server prints on startup.
//!
//! Trust model: admin commands always travel over a connection that has already been
//! authenticated at the HTTP head (the `?token=` query param is validated before the
//! WebSocket upgrade). The room re-validates by being the single owner of the lifecycle
//! state — it can refuse `OperatorReset` outside the `Ended` state, etc.

use rand::distributions::{Alphanumeric, DistString};
use serde::{Deserialize, Serialize};

/// Length of the auto-generated admin token. 16 alphanumerics ≈ 95 bits of entropy —
/// plenty for a LAN-only password that rotates on every server restart.
pub const ADMIN_TOKEN_LEN: usize = 16;

/// Generate a random alphanumeric token. Uses `rand::thread_rng()` because this runs
/// once at startup outside the deterministic simulation path.
pub fn generate_admin_token() -> String {
    Alphanumeric.sample_string(&mut rand::thread_rng(), ADMIN_TOKEN_LEN)
}

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
// Wire types
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdminMsg {
    Start,
    Abort,
    Reset,
    Kick { bot_id: String },
}

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
    fn generated_token_has_expected_length_and_charset() {
        let token = generate_admin_token();
        assert_eq!(token.len(), ADMIN_TOKEN_LEN);
        assert!(token.bytes().all(|b| b.is_ascii_alphanumeric()));
    }

    #[test]
    fn constant_time_eq_matches_string_eq() {
        assert!(constant_time_eq("alpha", "alpha"));
        assert!(!constant_time_eq("alpha", "alpha "));
        assert!(!constant_time_eq("alpha", "beta"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn admin_msg_roundtrips() {
        let messages = [
            AdminMsg::Start,
            AdminMsg::Abort,
            AdminMsg::Reset,
            AdminMsg::Kick {
                bot_id: "b_3".into(),
            },
        ];
        for msg in &messages {
            let json = serde_json::to_string(msg).unwrap();
            let parsed: AdminMsg = serde_json::from_str(&json).unwrap();
            assert_eq!(*msg, parsed, "roundtrip mismatch: {json}");
        }
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
