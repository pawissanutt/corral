//! Bridge v1 client (corral to host over the generation's Unix socket, `ctl_cap` per request,
//! peer uid checked): hello, get_state, submit_input, cancel, events_since and shutdown.
//! Owner: T-252 child host-bridge (recovery extends reconnect and reconciliation). Any corral
//! process reconnects through the generation's `ctl.cap`; `assets` holds the asset checks
//! dispatch and the launcher share.
pub mod assets;
mod client;

pub use client::*;

/// The T-252 child that lands the bridge client.
pub const OWNER: &str = "host-bridge";

#[cfg(all(test, unix))]
mod tests;
