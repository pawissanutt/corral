//! Settings: the workspace configuration and its editor, this daemon, devices and pairing,
//! push, and this device's connection.

pub mod connect;
pub mod document;
pub mod editor;
mod edits;
pub mod extensions;
pub mod form;
pub mod harnesses;
pub mod inherit;
pub mod live;
pub mod pi_startup;
pub mod profiles;
pub mod push;
pub mod qr;
pub mod routing;

pub use edits::{Edits, Pending, device_slot};
pub use live::daemon_connection_details;

/// Device-local pending edits; notification preferences come from the daemon's push model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub ids: crate::model::id::IdSupply,
    pub edits: Edits,
    /// `corral.toml` and its unsaved draft.
    pub config: editor::Editor,
}
