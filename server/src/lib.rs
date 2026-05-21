//! Library crate for `naval-server`. Exposes the modules so integration tests can drive
//! the server in-process; the binary in `main.rs` just composes them.

pub mod admin;
pub mod auth;
pub mod config;
pub mod net;
pub mod protocol;
pub mod replay;
pub mod room;
pub mod sim;
