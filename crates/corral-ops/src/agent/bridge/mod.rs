//! Bridge v1 client (corral to host over the generation's Unix socket, `ctl_cap` per request,
//! peer uid checked): hello, get_state, submit_input, cancel, events_since and shutdown.
//! Owner: T-252 child host-bridge (recovery extends reconnect and reconciliation). Any corral
//! process reconnects through the generation's `ctl.cap`; `assets` holds the asset checks
//! dispatch and the launcher share. Recovery adds `liveness`, which probes a bound host and
//! records its loss, `interrupted`, which classifies the calls a lost host left behind,
//! `notice`, which sizes the recovery notice and settles the envelope a lost host left open, and
//! `cancel`, which binds a turn cancel to the turn the host cancelled.
pub mod assets;
pub mod cancel;
mod client;
pub mod interrupted;
pub mod liveness;
pub mod notice;

pub use client::*;

/// The T-252 child that lands the bridge client.
pub const OWNER: &str = "host-bridge";

#[cfg(all(test, target_os = "linux"))]
pub(crate) mod fixture;
#[cfg(test)]
mod interrupted_tests;
#[cfg(all(test, target_os = "linux"))]
mod liveness_tests;
#[cfg(all(test, unix))]
mod tests;
