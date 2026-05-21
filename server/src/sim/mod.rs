//! Authoritative deterministic simulation. See `CLAUDE.md` for the determinism contract.
//!
//! This module must NOT import from `net.rs` or `protocol.rs` — the room translates between
//! simulation types and protocol types.

pub mod combat;
pub mod config;
pub mod constants;
pub mod physics;
pub mod sensors;
pub mod world;

pub use config::SimConfig;
pub use world::{BotId, Shell, Ship, ShipId, World};
