//! The Agents destination: one roster of every session the daemon addresses, of every role,
//! where sessions start (the New menu and the New build sheet), and the one session page every
//! session opens in (`session`), with its transcript (`transcript`). Everything shown comes from
//! `model::agents` and `model::session`; this module lays it out per form and sends every press
//! to the model's reducers.

mod remove;
mod rewind;
mod roster;
mod roster_rows;
mod segs;
mod session;
mod start;
mod transcript;

pub(crate) use remove::sheet as remove_sheet;
pub(crate) use rewind::sheet as rewind_sheet;
pub use session::{
    Compact, compact as ask_session, page as live_session, phone as live_session_phone,
    session_view,
};
pub(crate) use start::sheet as start_sheet;

use nsh_core::prelude::*;

use crate::model::agents;
use crate::model::projects::FilterScreen;
use crate::state::AppState;

/// The Agents page (`pgAgents`): the page head with its counts, role tabs and New, the project
/// scope and Show, then the roster, one line per session; a row opens its session page.
pub fn screen(app: AppState) -> AnyElement {
    roster::screen(app)
}

/// Runs one of the session page's actions through its reducer, which applies its effects.
fn run(app: AppState, action: crate::model::session::Action) {
    app.reduce(app.session, move |state, feed, _, _| {
        state.act(action, feed)
    });
}

/// Runs one of the roster's, the rewind sheet's, the New build sheet's or the Remove sheet's
/// actions against the Agents project scope.
fn run_agents(app: AppState, action: agents::Action) {
    let filter = untracked(|| app.project_filter(FilterScreen::Agents));
    app.reduce(app.agents, move |state, feed, _, _| {
        state.act(action, feed, &filter)
    });
}

/// The roster as the model builds it now in the Agents scope (tracked).
fn roster_view(app: AppState) -> agents::Roster {
    let filter = app.project_filter(FilterScreen::Agents);
    app.feed.with(|feed| {
        app.agents
            .with(|state| agents::roster(state, feed, &filter))
    })
}
