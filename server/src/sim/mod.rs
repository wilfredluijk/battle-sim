//! Authoritative deterministic simulation. See `CLAUDE.md` for the determinism contract.
//!
//! This module must NOT import from `net.rs` or `protocol.rs` — the room translates between
//! simulation types and protocol types.

pub mod constants;
pub mod physics;
pub mod world;

pub use world::{BotId, Ship, ShipId, Shell, World};
