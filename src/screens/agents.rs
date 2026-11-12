//! The Agents destination: the wall of agents on the workstation and one agent's session, its
//! transcript beside the run, or its terminal. Everything shown comes from `model::agents`; this
//! module lays it out per form and sends every press to the model's reducer.

mod console;
mod session;
mod transcript;
mod wall;

use nsh_core::prelude::*;

use crate::model::agents::{self, Action};
use crate::screens::PhonePage;
use crate::state::AppState;

/// The Agents page. Desktop: the page head with the Wall | List switch, the machine line, the
/// quick thread tile and one tile per agent in three columns, the assistant sessions and Filed
/// from chat blocks, the footer note; it scrolls inside itself. Phone: the large title, the
/// quick thread row, the machine section and one two-line row per agent, then the assistant
/// blocks, scrolling itself with the Ask clearance. Terminal: one line per agent between the
/// same blocks.
pub fn screen(app: AppState) -> AnyElement {
    wall::screen(app)
}

/// The session page (`Page::Session`) on desktop and terminal: the header with back to Agents,
/// the Transcript | Terminal switch (the `` ` `` key toggles it too), then the transcript with
/// its reply box beside the run, or the terminal surface.
pub fn session(app: AppState) -> AnyElement {
    session::page(app)
}

/// The session page on the phone: the segmented switch over the transcript or the terminal,
/// the reply box (or the terminal's keys) docked as the page's bar, no floating Ask.
pub fn session_phone(app: AppState) -> PhonePage {
    session::phone(app)
}

/// Runs one of the model's actions through the app state, which applies its effects.
fn run(app: AppState, action: Action) {
    app.act(app.agents, move |state, env| state.act(action, env));
}

/// The wall as the model builds it now (tracked).
fn wall_view(app: AppState) -> agents::Wall {
    let env = app.env();
    app.agents.with(|state| agents::wall(state, env))
}

/// The open session as the model builds it now (tracked).
fn session_view(app: AppState) -> agents::Session {
    let env = app.env();
    app.agents.with(|state| agents::session(state, env))
}
