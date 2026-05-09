//! Library crate for `naval-server`. Exposes the modules so integration tests can drive
//! the server in-process; the binary in `main.rs` just composes them.

pub mod config;
pub mod control;
pub mod net;
pub mod protocol;
pub mod room;
pub mod sim;
