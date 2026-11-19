//! The Host events sheet (`session::HostEvents`), opened from `⋯ › Host events`: a read-only
//! pager of the host's event log, newest first, each line its time, its kind and what the host
//! recorded, in the log lines the session log pager draws. How many there are and how current
//! they are lead; why the host cannot be read, or what the empty log means, stands in for lines;
//! Show earlier adds the page before; Reread reads the log again. test ids `host-events`
//! (`host-events-done`), `btn-events-earlier`, `btn-reread-events`.

use nsh_core::prelude::*;

use crate::model::hue::Hue;
use crate::model::session::{Action, EventsAction, HostEvents, SidebarAction, host_events};
use crate::model::text::span;
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::notice::notice;
use crate::ui::kit::overlay::{self, Modal};
use crate::ui::kit::text::dim;

use super::super::run;
use super::super::transcript::log_lines;
use super::sidebar::with_cx;

/// The sheet of session `id`, open while its model says so.
pub(super) fn sheet(app: AppState, id: Id) -> AnyElement {
    let opened = id.clone();
    let closed = id.clone();
    overlay::modal(
        app,
        move || {
            app.session
                .with(|state| state.ui(opened.as_str()).sidebar.events.open)
        },
        move || {
            run(
                app,
                Action::Sidebar(SidebarAction::Events(EventsAction::Close(closed.clone()))),
            )
        },
        move || modal(app, id.clone()),
    )
}

fn modal(app: AppState, id: Id) -> Modal {
    let shown = computed(move || with_cx(app, &id, host_events));
    let body = dynamic(move || shown.get().map(|events| untracked(|| body(app, &events))));
    let reread = computed(move || shown.with(|s| s.as_ref().and_then(|s| s.reread.clone())));
    let foot = dynamic(move || {
        reread.get().map(|action| {
            untracked(|| {
                button(
                    app,
                    "Reread",
                    ButtonKind::Ghost,
                    "btn-reread-events",
                    move || run(app, action.clone()),
                )
            })
        })
    });
    Modal {
        test_id: "host-events".into(),
        title: "Host events".into(),
        sub: None,
        body,
        foot: Some(view().flex_row().child(foot).into_element()),
        done: Some("Close".into()),
        confirm: None,
    }
}

fn body(app: AppState, events: &HostEvents) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let unavailable = events.unavailable.as_ref().map(|why| {
        notice(
            app,
            Hue::Held,
            &vec![span(format!("unavailable: {why}"))],
            None,
        )
    });
    let empty = events
        .empty
        .clone()
        .map(|words| dim(app, words).into_element());
    let earlier = events.earlier.clone().map(|(label, action)| {
        view()
            .flex_row()
            .child(button(
                app,
                label,
                ButtonKind::Quiet,
                "btn-events-earlier",
                move || run(app, action.clone()),
            ))
            .into_element()
    });
    view()
        .test_id("host-events-lines")
        .gap(px(if terminal { 0.0 } else { 8.0 }))
        .child(dim(app, events.sub.clone()))
        .children(unavailable)
        .children(empty)
        .child(log_lines(app, &events.lines))
        .children(earlier)
        .into_element()
}
