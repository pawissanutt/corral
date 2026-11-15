//! Canonical settings edits and device connection models.

pub mod authority;
pub mod autopilot;
pub mod connect;
mod edits;
pub mod live;
pub mod profiles;
pub mod push;
pub mod qr;

pub use edits::{ADD_PROFILE, Edits, Pending, SUMMARY, device_slot, remove_slot};
pub use live::daemon_connection_details;

/// Device-local pending edits; notification preferences come from the daemon's push model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub ids: crate::model::id::IdSupply,
    pub edits: Edits,
}
