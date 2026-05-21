//! Admin authentication for the REST control plane.
//!
//! The model is deliberately minimal (see `CLAUDE.md` "Hackathon mode"): a single shared
//! admin password. `POST /api/login` checks the password and mints a short-lived JWT;
//! every mutating `/api/*` route then requires that token as a `Bearer` credential.
//!
//! The JWT signing secret is random per process — sessions do not survive a server
//! restart, which is fine for a LAN-only game server in an ephemeral container.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use rand::distributions::{Alphanumeric, DistString};
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::admin::constant_time_eq;

/// JWT claim set. `sub` is always `"admin"` (the only principal); `exp` is the standard
/// expiry, validated automatically by `jsonwebtoken`.
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: u64,
}

/// Shared authentication state: the admin password plus the per-process JWT secret.
pub struct AuthState {
    admin_password: String,
    encoding: EncodingKey,
    decoding: DecodingKey,
    ttl_secs: u64,
}

impl std::fmt::Debug for AuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthState")
            .field("ttl_secs", &self.ttl_secs)
            .finish_non_exhaustive()
    }
}

impl AuthState {
    /// Build auth state from the resolved admin password and token lifetime. The JWT
    /// signing secret is generated fresh here.
    pub fn new(admin_password: String, ttl_secs: u64) -> Arc<Self> {
        let mut secret = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut secret);
        Arc::new(Self {
            admin_password,
            encoding: EncodingKey::from_secret(&secret),
            decoding: DecodingKey::from_secret(&secret),
            ttl_secs,
        })
    }

    /// Constant-time check of an operator-supplied password against the configured one.
    pub fn verify_password(&self, candidate: &str) -> bool {
        constant_time_eq(candidate, &self.admin_password)
    }

    /// Mint a signed JWT for the admin principal. Returns the token and its unix expiry.
    pub fn issue_token(&self) -> Result<(String, u64), jsonwebtoken::errors::Error> {
        let exp = now_unix() + self.ttl_secs;
        let claims = Claims {
            sub: "admin".to_string(),
            exp,
        };
        let token = encode(&Header::default(), &claims, &self.encoding)?;
        Ok((token, exp))
    }

    /// Validate a bearer token. Returns `true` only for a well-formed, unexpired token
    /// signed with this process's secret.
    pub fn verify_token(&self, token: &str) -> bool {
        decode::<Claims>(token, &self.decoding, &Validation::default()).is_ok()
    }
}

/// Generate a random admin password. Used when the operator did not supply one — the value
/// is logged once at startup, exactly like the old rotating admin token.
pub fn generate_admin_password() -> String {
    Alphanumeric.sample_string(&mut rand::thread_rng(), 16)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issued_token_verifies() {
        let auth = AuthState::new("hunter2".into(), 3600);
        let (token, exp) = auth.issue_token().expect("issue");
        assert!(exp > now_unix());
        assert!(auth.verify_token(&token));
    }

    #[test]
    fn wrong_password_rejected() {
        let auth = AuthState::new("hunter2".into(), 3600);
        assert!(auth.verify_password("hunter2"));
        assert!(!auth.verify_password("hunter3"));
        assert!(!auth.verify_password(""));
    }

    #[test]
    fn token_from_other_secret_rejected() {
        let a = AuthState::new("pw".into(), 3600);
        let b = AuthState::new("pw".into(), 3600);
        let (token, _) = a.issue_token().expect("issue");
        // `b` has an independent random secret, so `a`'s token must not validate.
        assert!(!b.verify_token(&token));
        assert!(!a.verify_token("not-a-jwt"));
    }

    #[test]
    fn expired_token_rejected() {
        let auth = AuthState::new("pw".into(), 0);
        let (token, _) = auth.issue_token().expect("issue");
        // exp == iat; jsonwebtoken's default leeway is 60s, so an exp in the past by more
        // than the leeway is required to observe rejection. Forge one directly instead.
        let claims = Claims {
            sub: "admin".into(),
            exp: now_unix().saturating_sub(7200),
        };
        let forged = encode(&Header::default(), &claims, &auth.encoding).expect("encode");
        assert!(!auth.verify_token(&forged));
        // A freshly issued (ttl=0) token is still inside the leeway window.
        let _ = token;
    }
}
