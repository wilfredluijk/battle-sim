//! Credentials are read from a protected roster file. Replacing it revokes live sessions.
use crate::{admin::constant_time_eq, protocol::validate_bot_name};
use serde::Deserialize;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Participant {
    pub identity: String,
    pub token: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
}
fn enabled() -> bool {
    true
}

#[derive(Default)]
pub struct Participants {
    pub path: Option<PathBuf>,
    pub allow_unauthenticated: bool,
    // Only registered identities get entries; bounded by the roster (max 256).
    pub(crate) sessions: Mutex<HashMap<String, (bool, Instant)>>,
}

impl Participants {
    pub fn new(path: Option<PathBuf>, allow_unauthenticated: bool) -> Self {
        Self {
            path,
            allow_unauthenticated,
            ..Default::default()
        }
    }
    pub fn read(&self) -> Result<Vec<Participant>, String> {
        let path = self.path.as_ref().ok_or("participant roster is required")?;
        let bytes = std::fs::read(path).map_err(|_| "cannot read participant roster")?;
        if bytes.len() > 128 * 1024 {
            return Err("roster exceeds 128 KiB".into());
        }
        let roster: Vec<Participant> =
            serde_json::from_slice(&bytes).map_err(|_| "invalid participant roster")?;
        if roster.is_empty() || roster.len() > 256 {
            return Err("roster must contain 1..256 identities".into());
        }
        let mut ids = std::collections::HashSet::new();
        let mut tokens = std::collections::HashSet::new();
        for p in &roster {
            if validate_bot_name(&p.identity).is_err()
                || p.token.len() < 32
                || !ids.insert(&p.identity)
                || !tokens.insert(&p.token)
            {
                return Err("roster requires unique valid identities and unique tokens of at least 32 characters".into());
            }
        }
        Ok(roster)
    }

    pub fn valid(&self, identity: &str, token: &str) -> bool {
        if self.path.is_none() {
            return self.allow_unauthenticated;
        }
        self.read().is_ok_and(|r| {
            r.iter()
                .any(|p| p.enabled && p.identity == identity && constant_time_eq(&p.token, token))
        })
    }

    pub fn acquire(self: &Arc<Self>, token: &str, name: &str) -> Option<Session> {
        let identity = if self.path.is_none() && self.allow_unauthenticated {
            name.to_owned()
        } else {
            self.read()
                .ok()?
                .into_iter()
                .find(|p| p.enabled && constant_time_eq(&p.token, token))?
                .identity
        };
        let mut sessions = self.sessions.lock().ok()?;
        sessions.retain(|_, (active, last)| *active || last.elapsed() < Duration::from_secs(10));
        if sessions
            .get(&identity)
            .is_some_and(|(active, last)| *active || last.elapsed() < Duration::from_secs(2))
        {
            return None;
        }
        sessions.insert(identity.clone(), (true, Instant::now()));
        Some(Session {
            identity,
            owner: self.clone(),
        })
    }
}

pub struct Session {
    pub identity: String,
    owner: Arc<Participants>,
}
impl Drop for Session {
    fn drop(&mut self) {
        if let Ok(mut sessions) = self.owner.sessions.lock() {
            sessions.insert(self.identity.clone(), (false, Instant::now()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roster_reserves_identity_and_live_revocation_is_observed() {
        let path =
            std::env::temp_dir().join(format!("roster-{}.json", crate::replay::unique_suffix()));
        let token = "test-credential-with-more-than-32-characters";
        let write = |enabled: bool| {
            std::fs::write(
                &path,
                serde_json::json!([
                    {"identity":"reserved-player", "token":token, "enabled":enabled}
                ])
                .to_string(),
            )
            .unwrap()
        };
        write(true);
        let roster = Arc::new(Participants::new(Some(path.clone()), false));
        assert!(roster.acquire("wrong", "reserved-player").is_none());
        let session = roster.acquire(token, "impersonated-name").unwrap();
        assert_eq!(session.identity, "reserved-player");
        assert!(roster.acquire(token, "different-name").is_none());
        assert!(roster.valid(&session.identity, token));
        write(false);
        assert!(!roster.valid(&session.identity, token));
        drop(session);
        std::fs::remove_file(path).unwrap();
    }
}
