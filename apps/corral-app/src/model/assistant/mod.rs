//! The one chat agent: what Ask adds to the session composer (the context and project chips,
//! the staged attachments), its attachment staging and readback, the assistant rows (`quick`
//! and `A-n`), and the chat operations corrald answers (`assistant.session`: new, end, profile).
//! The conversation itself is a session: Ask reads and sends through `model::session`.

mod actions;
mod ask;
mod attach;
mod live;
mod live_act;
mod project;
mod views;

pub use actions::*;
pub use ask::*;
pub use attach::*;
pub use live::*;
pub use live_act::*;
pub use project::*;
pub use views::*;

/// Device-local assistant interaction state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    pub ids: crate::model::id::IdSupply,
    pub live: LiveState,
}
