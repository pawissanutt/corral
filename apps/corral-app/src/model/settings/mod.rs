//! Settings: the workspace configuration as corrald read it, devices and pairing, push, and
//! this device's connection.

pub mod config;
pub mod connect;
mod edits;
pub mod live;
pub mod push;
pub mod qr;

pub use edits::{Edits, Pending, device_slot};
pub use live::daemon_connection_details;

/// Device-local pending edits; notification preferences come from the daemon's push model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub ids: crate::model::id::IdSupply,
    pub edits: Edits,
}
