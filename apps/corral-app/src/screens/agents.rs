//! The Agents destination: the wall of agents and one agent's session, its transcript beside
//! the run, or its terminal. Everything shown comes from `model::agents`, built from the fixture
//! preview or from corrald's rows and details; this module lays it out per form and sends every
//! press to the model's reducer.

mod console;
mod session;
mod transcript;
mod wall;

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::Id;
use crate::model::agents::{self, Action, View};
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

/// The fixture session page (`Page::Session`) on desktop and terminal: the header with back to
/// Agents, the Transcript | Terminal switch (the `` ` `` key toggles it too), then the
/// transcript with its reply box beside the run, or the terminal surface.
pub fn session(app: AppState) -> AnyElement {
    session::page(app, computed(move || fixture_session(app)), None)
}

/// The fixture session page on the phone.
pub fn session_phone(app: AppState) -> PhonePage {
    session::phone(app, computed(move || fixture_session(app)), None)
}

/// A daemon session page on desktop and terminal, by its session id: the same page as the
/// fixture's, from corrald's row, transcript page and recorded capture.
pub fn live_session(app: AppState, id: Id) -> AnyElement {
    let view = computed({
        let id = id.clone();
        move || daemon_session(app, &id)
    });
    follow(app, id.clone());
    session::page(app, view, Some(id))
}

/// A daemon session page on the phone, by its session id.
pub fn live_session_phone(app: AppState, id: Id) -> PhonePage {
    let view = computed({
        let id = id.clone();
        move || daemon_session(app, &id)
    });
    follow(app, id.clone());
    session::phone(app, view, Some(id))
}

/// Runs one of the model's actions through the app state, which applies its effects: the
/// fixture reducer in the preview, the daemon reducer (with the feed and the root selection)
/// otherwise.
fn run(app: AppState, action: Action) {
    if untracked(|| app.daemon()) {
        app.reduce(app.agents, move |state, feed, selection, env| {
            state.act_live(action, feed, selection, env)
        });
    } else {
        app.act(app.agents, move |state, env| state.act(action, env));
    }
}

/// The wall as the model builds it now (tracked).
fn wall_view(app: AppState) -> agents::Wall {
    let env = app.env();
    if app.daemon() {
        app.feed
            .with(|feed| app.agents.with(|state| agents::live_wall(state, feed, env)))
    } else {
        app.agents.with(|state| agents::wall(state, env))
    }
}

/// The open fixture session as the model builds it now (tracked).
fn fixture_session(app: AppState) -> agents::Session {
    let env = app.env();
    app.agents.with(|state| agents::session(state, env))
}

/// A daemon session as the model builds it now (tracked).
fn daemon_session(app: AppState, id: &Id) -> agents::Session {
    let env = app.env();
    let via = app.via();
    app.feed.with(|feed| {
        app.agents
            .with(|state| agents::live_session(state, id, feed, env, &via))
    })
}

/// Keeps the open session's details current: its transcript page whenever its row changes (not
/// while the owner reads an earlier page) and its capture when the terminal is first shown.
/// The session page itself refreshes the transcript once a reply is delivered.
fn follow(app: AppState, id: Id) {
    let transcript = DetailRef::Session { id: id.to_string() };
    let capture = DetailRef::Terminal { id: id.to_string() };
    let rev = computed({
        let id = id.clone();
        move || {
            app.feed
                .with(|feed| feed.agents.get(id.as_str()).map(|row| row.rev))
        }
    });
    let seen_rev = stored(None::<Option<u64>>);
    {
        let id = id.clone();
        effect(move || {
            let now = rev.get();
            let earlier = app
                .agents
                .with_untracked(|state| state.earlier.contains(id.as_str()));
            match seen_rev.replace(Some(now)) {
                None => untracked(|| app.load(transcript.clone())),
                Some(before) if before != now && !earlier => {
                    untracked(|| app.refresh(transcript.clone()));
                }
                Some(_) => {}
            }
        });
    }
    let has_pane = computed(move || {
        app.feed.with(|feed| {
            feed.agents
                .get(id.as_str())
                .is_some_and(|row| row.pane.is_some())
        })
    });
    effect(move || {
        if has_pane.get() && app.agents.with(|state| state.view == View::Terminal) {
            untracked(|| app.load(capture.clone()));
        }
    });
}
