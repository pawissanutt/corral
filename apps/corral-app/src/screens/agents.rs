//! The Agents destination: the roster of live runs and the quick thread with the selected one's
//! panel, and one run's full session page: its terminal with the pane's keys, or, from a corrald
//! that serves them, its transcript beside the run. Everything shown comes from `model::agents`,
//! built from the feed's rows and details; this module lays it out per form and sends every
//! press to the model's reducer. Each run's row, panel and page carry its Session menu.

mod console;
mod controls;
mod grid;
mod keys;
mod panel;
mod panel_summary;
mod reply;
mod roster;
mod roster_rows;
mod segs;
mod session;
mod transcript;

use corral_protocol::app::SESSION_V1;
use corral_protocol::detail::{DetailRef, STREAM_TERMINAL_V1};
use corral_protocol::rows::AgentRow;
use nsh_core::prelude::*;

use crate::model::Id;
use crate::model::agents::{self, Action, View};
use crate::model::daemon::Link;
use crate::model::projects::FilterScreen;
use crate::screens::PhonePage;
use crate::state::AppState;

/// How a press reaches the agents model: through the full session page's route selection
/// ([`run`]) or through the roster's own selection ([`run_roster`]).
type Dispatch = fn(AppState, Action);

/// The Agents page (`pgAgents`). Desktop: the page head with its counts and role tabs, the
/// project scope, the roster one line per session with the selected session's panel beside or
/// over it, then the assistant sessions. Phone: the same head and scopes over two-line rows that
/// push the session page. Terminal: the roster as a table of Cells with the panel beside it on a
/// wide grid, in its place otherwise.
pub fn screen(app: AppState) -> AnyElement {
    roster::screen(app)
}

/// A session page on desktop and terminal, by its session id: the header with back to Agents,
/// its Session menu and, where corrald serves transcripts, the Transcript | Terminal switch (the
/// `` ` `` key toggles it too), the strip with the run's state, then the terminal with the
/// pane's keys, or the transcript with its reply box beside the run.
pub fn live_session(app: AppState, id: Id) -> AnyElement {
    let view = computed({
        let id = id.clone();
        move || daemon_session(app, &id)
    });
    follow(app, id.clone());
    if untracked(|| app.capable(SESSION_V1)) {
        transcript::watch(app, id.clone());
    }
    session::page(app, view, id)
}

/// A session page on the phone, by its session id.
pub fn live_session_phone(app: AppState, id: Id) -> PhonePage {
    let view = computed({
        let id = id.clone();
        move || daemon_session(app, &id)
    });
    follow(app, id.clone());
    if untracked(|| app.capable(SESSION_V1)) {
        transcript::watch(app, id.clone());
    }
    session::phone(app, view, id)
}

/// Runs one of the model's actions through the feed-backed reducer (with the feed and the root
/// selection), which applies its effects.
fn run(app: AppState, action: Action) {
    app.reduce(app.agents, move |state, feed, selection, env| {
        state.act_live(action, feed, selection, env)
    });
}

/// Runs one of the model's actions against the roster's selection: the panel's reply reaches
/// the selected session without the route changing.
fn run_roster(app: AppState, action: Action) {
    app.reduce(app.agents, move |state, feed, _, env| {
        state.act_roster(action, feed, env)
    });
}

/// The roster as the model builds it now in the Agents scope (tracked).
fn roster_view(app: AppState) -> agents::Roster {
    let env = app.env();
    let filter = app.project_filter(FilterScreen::Agents);
    app.feed.with(|feed| {
        app.agents
            .with(|state| agents::roster(state, feed, env, &filter))
    })
}

/// The selected session's panel as the model builds it now (tracked); none while no session in
/// scope is selected.
fn panel_view(app: AppState) -> Option<agents::RosterPanel> {
    let env = app.env();
    let via = app.via();
    let filter = app.project_filter(FilterScreen::Agents);
    app.feed.with(|feed| {
        app.agents
            .with(|state| agents::roster_panel(state, feed, env, &filter, &via))
    })
}

/// A session as the model builds it now (tracked).
fn daemon_session(app: AppState, id: &Id) -> agents::Session {
    let env = app.env();
    let via = app.via();
    app.feed.with(|feed| {
        app.agents
            .with(|state| agents::live_session(state, id, feed, env, &via))
    })
}

/// The run's row as served, compared by equality: rows carry no revision.
fn row_of(app: AppState, id: Id) -> Memo<Option<AgentRow>> {
    computed(move || app.feed.with(|feed| feed.agents.get(id.as_str()).cloned()))
}

/// Keeps the open session's details current: its transcript page, from a corrald that serves
/// transcripts, whenever its row changes (not while the owner reads an earlier page), and its
/// pane's screen while the terminal shows: read when first shown, then revalidated once a tick
/// of the feed's clock while the link is up. A failed read is not repeated every tick; it is
/// tried again once the row changes or the link comes back, or by the owner's Refresh. A
/// corrald that streams terminals sends the screen itself, its first frame complete, to the
/// session page's stream; reading it here as well would race that stream, so then nothing is
/// read. The session page itself refreshes the transcript once a reply is delivered. Reads go
/// to the source the page mounted on.
fn follow(app: AppState, id: Id) {
    let scoped = untracked(|| app.scope());
    let transcript = DetailRef::Session { id: id.to_string() };
    let capture = DetailRef::Terminal { id: id.to_string() };
    let row = row_of(app, id.clone());
    let transcripts = computed(move || app.capable(SESSION_V1));
    let seen_row = stored(None::<Option<AgentRow>>);
    {
        let id = id.clone();
        effect(move || {
            let now = row.get();
            if !transcripts.get() {
                return;
            }
            let earlier = app
                .agents
                .with_untracked(|state| state.earlier.contains(id.as_str()));
            match seen_row.replace(Some(now.clone())) {
                None => untracked(|| scoped.load(transcript.clone())),
                Some(before) if before != now && !earlier => {
                    untracked(|| scoped.refresh(transcript.clone()));
                }
                Some(_) => {}
            }
        });
    }
    let has_screen =
        computed(move || row.with(|row| row.as_ref().is_some_and(|row| row.run.session.is_some())));
    let streamed = computed(move || app.feed.with(|feed| feed.capable(STREAM_TERMINAL_V1)));
    let shown = computed(move || {
        has_screen.get()
            && !streamed.get()
            && (!transcripts.get() || app.agents.with(|state| state.live_view == View::Terminal))
    });
    {
        let capture = capture.clone();
        effect(move || {
            if shown.get() {
                untracked(|| scoped.load(capture.clone()));
            }
        });
    }
    // The client driver advances `now` once a second; reading it through this memo runs the
    // revalidation once a tick rather than on every feed write, and not at all while down.
    let tick = computed(move || {
        app.feed
            .with(|feed| (feed.link == Link::Up).then_some(feed.now))
    });
    let tried_row = stored(None::<Option<AgentRow>>);
    effect(move || {
        if !shown.get() {
            return;
        }
        if tick.get().is_none() {
            tried_row.replace(None);
            return;
        }
        untracked(|| {
            let now = row.get();
            let changed = tried_row.replace(Some(now.clone())) != Some(now);
            let failed = app.feed.with(|feed| {
                feed.detail(&capture)
                    .is_some_and(|cache| cache.error.is_some())
            });
            if changed || !failed {
                scoped.refresh(capture.clone());
            }
        });
    });
}
