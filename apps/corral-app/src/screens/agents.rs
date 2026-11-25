//! Where sessions start (the New menu and the New build sheet), the one session page every
//! session opens in (`session`) with its transcript (`transcript`), and the sheets every shell
//! mounts once: rewind and Remove. Sessions lists them (`screens::sessions`). Everything shown
//! comes from `model::agents` and `model::session`; this module lays it out per form and sends
//! every press to the model's reducers.

mod remove;
mod rewind;
mod session;
mod start;
mod transcript;
pub(crate) use transcript::watch as watch_transcript;

pub(crate) use remove::sheet as remove_sheet;
pub(crate) use rewind::sheet as rewind_sheet;
pub use session::{
    Compact, ask_view, compact as ask_session, page as live_session, phone as live_session_phone,
};
pub(crate) use start::{new_menu, sheet as start_sheet};

use nsh_core::prelude::*;

use crate::model::agents;
use crate::state::AppState;

/// Runs one of the session page's actions through its reducer, which applies its effects.
fn run(app: AppState, action: crate::model::session::Action) {
    app.session_input(action, false);
}

/// Runs one of the New build sheet's, the rewind sheet's or the Remove sheet's actions against
/// the project scope.
pub(crate) fn run_agents(app: AppState, action: agents::Action) {
    let filter = untracked(|| app.scope_filter());
    app.reduce(app.agents, move |state, feed, _, _| {
        state.act(action, feed, &filter)
    });
}
