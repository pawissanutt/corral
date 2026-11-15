//! The Agents destination: the wall of agents and one agent's session, its transcript beside
//! the run, or its terminal. Everything shown comes from `model::agents`, built from the feed's
//! rows and details; this module lays it out per form and sends every press to the model's
//! reducer.

mod console;
mod controls;
mod grid;
mod keys;
mod reply;
mod session;
mod transcript;
mod wall;

use corral_protocol::detail::{DetailRef, STREAM_TERMINAL_V1};
use nsh_core::prelude::*;

use crate::model::Id;
use crate::model::agents::{self, Action, View};
use crate::model::daemon::Link;
use crate::screens::{PhonePage, assistant};
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

/// A session page on desktop and terminal, by its session id: the header with back to Agents,
/// the Transcript | Terminal switch (the `` ` `` key toggles it too), then the transcript with
/// its reply box beside the run, or the terminal surface, from the feed's row, transcript page
/// and its pane's screen.
pub fn live_session(app: AppState, id: Id) -> AnyElement {
    let view = computed({
        let id = id.clone();
        move || daemon_session(app, &id)
    });
    follow(app, id.clone());
    transcript::watch(app, id.clone());
    chat(app, id.clone());
    session::page(app, view, id)
}

/// A session page on the phone, by its session id.
pub fn live_session_phone(app: AppState, id: Id) -> PhonePage {
    let view = computed({
        let id = id.clone();
        move || daemon_session(app, &id)
    });
    follow(app, id.clone());
    transcript::watch(app, id.clone());
    chat(app, id.clone());
    session::phone(app, view, id)
}

/// While an assistant's session page is open, subscribes the thread its composer chats in (the
/// reference-counted thread scope; released when the page closes or the scope changes, through
/// the source it mounted on) and settles the composer's session operations, as the thread page
/// does.
fn chat(app: AppState, id: Id) {
    assistant::watch_ops(app);
    let scoped = untracked(|| app.scope());
    let scope = computed(move || {
        app.feed.with(|feed| {
            feed.agents
                .get(id.as_str())
                .filter(|row| row.role == "assistant")
                .map(|row| agents::chat_scope(feed, row))
        })
    });
    effect(move || {
        if let Some(scope) = scope.get() {
            untracked(|| assistant::open_scope(scoped, scope, false));
        }
    });
}

/// Runs one of the model's actions through the feed-backed reducer (with the feed and the root
/// selection), which applies its effects.
fn run(app: AppState, action: Action) {
    app.reduce(app.agents, move |state, feed, selection, env| {
        state.act_live(action, feed, selection, env)
    });
}

/// The wall as the model builds it now (tracked).
fn wall_view(app: AppState) -> agents::Wall {
    let env = app.env();
    app.feed
        .with(|feed| app.agents.with(|state| agents::live_wall(state, feed, env)))
}

/// A session as the model builds it now (tracked), with the reply to its latest
/// `assistant.session` control operation.
fn daemon_session(app: AppState, id: &Id) -> agents::Session {
    let env = app.env();
    let via = app.via();
    let op = app
        .agents
        .with(|state| agents::op_key(state, id))
        .and_then(|key| app.with_operation(&key, |op| op.map(agents::op_status)));
    app.feed.with(|feed| {
        app.agents
            .with(|state| agents::live_session(state, id, feed, env, &via, op.as_ref()))
    })
}

/// Keeps the open session's details current: its transcript page whenever its row changes (not
/// while the owner reads an earlier page), and its pane's screen while the terminal shows: read
/// when first shown, then revalidated once a tick of the feed's clock while the link is up. A
/// failed read is not repeated every tick; it is tried again once the row changes or the link
/// comes back, or by the owner's Refresh. A corrald that streams terminals sends the screen
/// itself, its first frame complete, to the session page's stream; reading it here as well
/// would race that stream, so then nothing is read. The session page itself refreshes the
/// transcript once a reply is delivered. Reads go to the source the page mounted on.
fn follow(app: AppState, id: Id) {
    let scoped = untracked(|| app.scope());
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
                None => untracked(|| scoped.load(transcript.clone())),
                Some(before) if before != now && !earlier => {
                    untracked(|| scoped.refresh(transcript.clone()));
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
    let streamed = computed(move || app.feed.with(|feed| feed.capable(STREAM_TERMINAL_V1)));
    let shown = computed(move || {
        has_pane.get()
            && !streamed.get()
            && app.agents.with(|state| state.live_view == View::Terminal)
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
    let tried_rev = stored(None::<Option<u64>>);
    effect(move || {
        if !shown.get() {
            return;
        }
        if tick.get().is_none() {
            tried_rev.replace(None);
            return;
        }
        untracked(|| {
            let row = rev.get();
            let changed = tried_rev.replace(Some(row)) != Some(row);
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
