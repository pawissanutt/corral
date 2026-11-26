//! The Gate overlay: a task's gate attempt as corrald observes it, over the page. Its head says
//! where the attempt stands (and offline, that its liveness is unknown), its facts, the check's
//! result and a way to open the retained check log; the log follows below, one bounded window at
//! a time. The newest window is read again every few seconds while the attempt is live; scrolling
//! up from the end pins the window the owner is reading (`GateUi::held`) so the text does not
//! slide under them, and scrolling back to the end, or Newest, follows again. Earlier output
//! reads the window before the shown one. test ids `gate` (`gate-done`), `gate-facts`,
//! `gate-log`, `btn-gate-cite`, `btn-gate-older`, `btn-gate-newest`, `btn-gate-retry`.

use corral_protocol::detail::{DetailBody, DetailGet, DetailRef};
use corral_types::GateAttemptPage;
use nsh_core::prelude::*;

use crate::model::daemon::{Feed, Link};
use crate::model::gate::{self, GateOpen, GateUi, GateView, due, target};
use crate::model::text::span;
use crate::model::{Effect, Form};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::notice::notice;
use crate::ui::kit::overlay::{self, Modal};
use crate::ui::kit::text::dim;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, Token, color};

use super::agents::transcript::wrapped_log_lines;

#[cfg(test)]
#[path = "gate_tests.rs"]
mod tests;

/// How close to the end still counts as at the end, in points.
const END_SLACK: f32 = 24.0;
/// The log's own height, in points; in a terminal, in rows.
const LOG_HEIGHT: f32 = 300.0;
const LOG_HEIGHT_PHONE: f32 = 240.0;
const LOG_ROWS: f32 = 14.0;
/// The label column of the facts.
const LABEL_WIDTH: f32 = 128.0;

/// The Gate overlay of `open`, shown while `app.gate_open` holds it. Mount it under a `dynamic`
/// on `app.gate_open`: the view's state lives with that mount, and the reads and the watch of
/// the attempt's log end with it.
pub fn overlay(app: AppState, open: GateOpen) -> AnyElement {
    let ui = signal(GateUi::default());
    keep(app, open.clone(), ui);
    let shown = open.clone();
    overlay::modal(
        app,
        move || app.gate_open.with(|now| now.as_ref() == Some(&shown)),
        move || app.gate_open.set(None),
        move || modal(app, open.clone(), ui),
    )
}

/// The newest window the feed holds of `open`'s log.
fn newest_page(feed: &Feed, open: &GateOpen) -> Option<GateAttemptPage> {
    match feed
        .detail(&target(open.task.as_str(), &open.attempt, None))?
        .body()?
    {
        DetailBody::GateAttempt(page) => Some(page.clone()),
        _ => None,
    }
}

/// Reads `target` again: conditional on what this device holds of it, so an unchanged log costs
/// an unchanged reply.
fn read(app: AppState, target: DetailRef) {
    let etag = app.feed.with_untracked(|feed| {
        feed.detail(&target)
            .and_then(|cache| cache.pages.first().map(|page| page.etag.clone()))
    });
    app.detail(DetailGet {
        target,
        etag,
        page: None,
    });
}

/// What the view keeps while it is mounted: the newest window pinned against eviction, read at
/// once on opening (a finished or interrupted attempt too, which `due` would never ask for) and
/// again when the link returns, then every few seconds while the attempt is live. The pin goes
/// with the mount.
fn keep(app: AppState, open: GateOpen, ui: Signal<GateUi>) {
    let newest = target(open.task.as_str(), &open.attempt, None);
    let owner = app.scope();
    owner.watch_detail(newest.clone(), true);
    let released = newest.clone();
    cleanup(move || owner.watch_detail(released.clone(), false));
    let last = signal(0u64);
    // On opening, and each time the link comes up: the newest window, and the older one shown.
    let up = computed(move || app.feed.with(|feed| feed.link == Link::Up));
    {
        let open = open.clone();
        effect(move || {
            if !up.get() {
                return;
            }
            untracked(|| {
                last.set(app.feed.with_untracked(|feed| feed.now));
                read(app, newest.clone());
                if let Some(before) = ui.with_untracked(|ui| ui.before) {
                    read(app, target(open.task.as_str(), &open.attempt, Some(before)));
                }
            });
        });
    }
    // While live: the newest window again as `due` says.
    effect(move || {
        let now = app
            .feed
            .with(|feed| due(feed, &open, last.get_untracked()).then_some(feed.now));
        if let Some(now) = now {
            untracked(|| {
                last.set(now);
                read(app, target(open.task.as_str(), &open.attempt, None));
            });
        }
    });
}

fn modal(app: AppState, open: GateOpen, ui: Signal<GateUi>) -> Modal {
    let shown = computed({
        let open = open.clone();
        move || {
            app.feed
                .with(|feed| ui.with(|ui| gate::view(feed, &open, ui)))
        }
    });
    let (title, sub) = shown.with_untracked(|v| (v.title.clone(), v.sub.clone()));
    let scroller = element_ref();
    follow(shown, ui, scroller);
    let head = dynamic(move || {
        let v = shown.get();
        Some(untracked(|| head(app, &v)))
    });
    let body = view()
        .test_id("gate-body")
        .gap(px(if app.form == Form::Terminal {
            0.0
        } else {
            10.0
        }))
        .child(head)
        .child(paging_before(app, open.clone(), shown, ui))
        .child(log(app, open, shown, ui, scroller))
        .child(paging_after(app, shown, ui))
        .into_element();
    Modal {
        test_id: "gate".into(),
        title,
        sub: Some(sub),
        body,
        foot: None,
        done: Some("Close".into()),
        confirm: None,
    }
}

/// While following (no older window, nothing held), each change of the output keeps the log at
/// its newest line.
fn follow(shown: Memo<GateView>, ui: Signal<GateUi>, scroller: ElementRef) {
    let tail = computed(move || shown.with(|v| (v.end, v.lines.len())));
    let following = computed(move || ui.with(is_following));
    effect(move || {
        let _ = tail.get();
        if following.get() {
            queue_end(ui, scroller);
        }
    });
}

fn is_following(ui: &GateUi) -> bool {
    ui.before.is_none() && ui.held.is_none()
}

fn queue_end(ui: Signal<GateUi>, scroller: ElementRef) {
    // A wheel or Earlier press may arrive between scheduling and executing the scroll.
    after_answer(move || {
        if ui.with_untracked(is_following) {
            scroller.scroll_to_end(false);
        }
    });
}

/// The head: where the attempt stands, its facts, the check's result with the way to open its
/// log, and whether the log follows.
fn head(app: AppState, v: &GateView) -> AnyElement {
    let form = app.form;
    let (hue, said) = v.standing.clone();
    let facts = view()
        .test_id("gate-facts")
        .gap(px(if form == Form::Terminal { 0.0 } else { 3.0 }))
        .children(
            v.facts
                .iter()
                .map(|(label, value)| fact(app, label, value.clone())),
        );
    let result = v
        .result
        .clone()
        .map(|(hue, said)| notice(app, hue, &vec![span(said)], None));
    let cite = v.artifact.clone().map(|reference| {
        view()
            .flex_row()
            .child(button(
                app,
                "Open check log",
                ButtonKind::Ghost,
                "btn-gate-cite",
                move || {
                    app.gate_open.set(None);
                    app.run(vec![Effect::OpenArtifact {
                        reference: reference.clone(),
                        section: None,
                    }]);
                },
            ))
            .into_element()
    });
    let follow = v
        .follow
        .clone()
        .map(|(hue, said)| notice(app, hue, &vec![span(said)], None));
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
        .child(notice(app, hue, &vec![span(said)], None))
        .child(facts)
        .children(result)
        .children(cite)
        .children(follow)
        .children(v.span.clone().map(|span| dim(app, span).into_element()))
        .into_element()
}

/// One fact: its label, then its value in the log's type.
fn fact(app: AppState, label: &'static str, value: String) -> AnyElement {
    let compact = app.form == Form::Phone;
    let value = text(value)
        .min_w(px(0.0))
        .font_family(FONT_MONO)
        .text_size(px(12.0))
        .text_color(color(Token::Ink));
    let label = view()
        .flex_none()
        .w(px(if compact { 104.0 } else { LABEL_WIDTH }))
        .child(dim(app, label.to_owned()));
    view()
        .flex_row()
        .gap(px(10.0))
        .child(label)
        .child(view().flex_1().min_w(px(0.0)).child(value))
        .into_element()
}

/// The way to earlier output, and how the read stands with its Retry, drawn again only when
/// those change.
fn paging_before(
    app: AppState,
    open: GateOpen,
    shown: Memo<GateView>,
    ui: Signal<GateUi>,
) -> AnyElement {
    let paging = computed(move || {
        shown.with(|v| (v.older, v.read.clone(), v.retry.clone(), v.empty.clone()))
    });
    dynamic(move || {
        let (older, read_state, retry, empty) = paging.get();
        Some(untracked(|| {
            let older = older.map(|before| {
                let open = open.clone();
                view()
                    .flex_row()
                    .child(button(
                        app,
                        "Earlier output",
                        ButtonKind::Quiet,
                        "btn-gate-older",
                        move || {
                            ui.update(|ui| ui.before = Some(before));
                            read(app, target(open.task.as_str(), &open.attempt, Some(before)));
                        },
                    ))
                    .into_element()
            });
            let read_state = read_state.map(|(hue, said)| {
                let retry = retry.map(|again| {
                    button(
                        app,
                        "Retry",
                        ButtonKind::Quiet,
                        "btn-gate-retry",
                        move || read(app, again.clone()),
                    )
                });
                notice(app, hue, &vec![span(said)], retry)
            });
            let empty = empty.map(|said| dim(app, said).into_element());
            view()
                .gap(px(8.0))
                .children(older)
                .children(read_state)
                .children(empty)
                .into_element()
        }))
    })
    .into_element()
}

/// The log: its lines in a scroll of their own, so the overlay's other parts stay in view.
/// Scrolling up from the end pins the newest window as it stands; coming back to the end (or
/// Newest) lets it follow again. An older window does not follow whatever the scroll does.
fn log(
    app: AppState,
    open: GateOpen,
    shown: Memo<GateView>,
    ui: Signal<GateUi>,
    scroller: ElementRef,
) -> AnyElement {
    let form = app.form;
    let lines = computed(move || shown.with(|v| v.lines.clone()));
    let content = dynamic(move || {
        let lines = lines.get();
        Some(untracked(|| {
            view()
                .test_id("gate-log-content")
                .child(wrapped_log_lines(app, &lines))
                .into_element()
        }))
    });
    let surface = scroll()
        .test_id("gate-log")
        .element_ref(&scroller)
        .on_scroll(move |event: &ScrollEvent| {
            let below = event.content_size.height.0 - event.offset.y.0 - event.size.height.0;
            let at_end = below <= END_SLACK;
            let (older, held) = ui.with_untracked(|ui| (ui.before.is_some(), ui.held.is_some()));
            if older {
                return;
            }
            match (at_end, held) {
                (false, false) => {
                    if let Some(page) = app.feed.with_untracked(|feed| newest_page(feed, &open)) {
                        ui.update(|ui| ui.held = Some(page));
                    }
                }
                (true, true) => ui.update(|ui| ui.held = None),
                _ => {}
            }
        });
    match form {
        Form::Terminal => surface
            .h(term::rows(LOG_ROWS))
            .child(view().px(term::cells(1.0)).child(content))
            .into_element(),
        Form::Desktop => surface
            .h(px(LOG_HEIGHT))
            .child(
                view()
                    .px(px(10.0))
                    .py(px(8.0))
                    .bg(color(Token::BgSunk))
                    .child(content),
            )
            .into_element(),
        Form::Phone => surface
            .h(px(LOG_HEIGHT_PHONE))
            .child(view().px(px(8.0)).py(px(6.0)).child(content))
            .into_element(),
    }
}

/// Newest: from an older or pinned window back to the live end.
fn paging_after(app: AppState, shown: Memo<GateView>, ui: Signal<GateUi>) -> AnyElement {
    let newest = computed(move || shown.with(|v| v.newest));
    dynamic(move || {
        newest.get().then(|| {
            untracked(|| {
                view()
                    .flex_row()
                    .child(button(
                        app,
                        "Newest output",
                        ButtonKind::Ghost,
                        "btn-gate-newest",
                        move || ui.set(GateUi::default()),
                    ))
                    .into_element()
            })
        })
    })
    .into_element()
}
