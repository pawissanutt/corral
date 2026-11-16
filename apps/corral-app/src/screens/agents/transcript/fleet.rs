//! A watched session's child agents and the messages between them (`model::agents::Fleet`),
//! drawn as a published build's marked entries (`screens::builds::line_view`), read-only.
//! Desktop and terminal: under the run's facts and stages in the run column, so the transcript
//! keeps its place. Phone: one folded line over the transcript (`btn-fleet`) that opens both
//! lists in a bounded scroll on the sunken ground. A session without a fleet draws nothing.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::agents::{Fleet, FleetSection, Session};
use crate::screens::builds::line_view;
use crate::state::AppState;
use crate::ui::kit::page::context;
use crate::ui::kit::table::steps;
use crate::ui::kit::text::{dim, h3};
use crate::ui::term;
use crate::ui::tokens::{Token, color};

use super::super::run;
use super::parts::{disclosure, small};

/// The phone's opened lists stop here, so the transcript under them keeps most of the page.
const PHONE_OPEN_HEIGHT: f32 = 320.0;

/// The fleet in the run column: a note while it is stale, then the children and the messages,
/// each under its caption with its count.
pub fn column(app: AppState, fleet: &Fleet) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    view()
        .test_id("session-fleet")
        .children(fleet.stale.as_ref().map(|words| {
            view()
                .when(terminal, |note| note.child(term::blank()))
                .when(!terminal, |note| note.pt(px(14.0)))
                .child(dim(app, words.clone()))
                .into_element()
        }))
        .children(
            [&fleet.children, &fleet.messages]
                .into_iter()
                .flatten()
                .map(|part| section(app, part))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// One list under its caption: each entry a mark and its line, carrying the entry's test id.
fn section(app: AppState, part: &FleetSection) -> AnyElement {
    let entries = part
        .entries
        .iter()
        .map(|entry| {
            (
                entry.line.mark,
                entry.line.hue,
                view()
                    .test_id(&entry.test_id)
                    .min_w(px(0.0))
                    .child(line_view(app, &entry.line))
                    .into_element(),
            )
        })
        .collect();
    view()
        .when(app.form == Form::Terminal, |part| part.child(term::blank()))
        .child(h3(app, part.heading, part.note.clone()))
        .child(steps(app, entries))
        .into_element()
}

/// The phone's fold over the transcript: nothing for a session without a fleet; otherwise the
/// line naming the children and saying how many work and how many messages there are, and,
/// while it is open, both lists under it. The line and the lists follow the fleet apart, so a
/// new child neither moves the lists' scroll nor takes the line's focus.
pub fn phone(app: AppState, session: Memo<Session>) -> AnyElement {
    let fleet = computed(move || session.with(|s| s.fleet.clone()));
    let shown = computed(move || fleet.with(Option::is_some));
    dynamic(move || {
        if !shown.get() {
            return view().into_element();
        }
        untracked(move || fold(app, fleet))
    })
}

fn fold(app: AppState, fleet: Memo<Option<Fleet>>) -> AnyElement {
    let open = computed(move || fleet.with(|f| f.as_ref().is_some_and(|f| f.open)));
    let words = computed(move || {
        fleet.with(|f| {
            f.as_ref()
                .map(|f| (f.fold_title, f.summary.clone()))
                .unwrap_or_default()
        })
    });
    let line = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .min_h(px(44.0))
        .px(px(16.0))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(move || words.with(|(title, summary)| format!("{title}: {summary}")))
        .accessibility_state(move || AccessibilityState {
            expanded: Some(open.get()),
            ..AccessibilityState::default()
        })
        .test_id("btn-fleet")
        .on_press(move || {
            if let Some(toggle) = fleet.with_untracked(|f| f.as_ref().map(|f| f.toggle.clone())) {
                run(app, toggle);
            }
        })
        .active(|s| s.bg(color(Token::Sel)))
        .focus_visible(|s| {
            s.outline(px(2.0))
                .outline_color(color(Token::Focus))
                .outline_offset(px(-2.0))
        })
        .child(dynamic(move || disclosure(app, open.get())))
        .child(dynamic(move || {
            words.with(|(title, summary)| {
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex_row()
                    .gap(px(6.0))
                    .child(
                        small(app, *title, Token::Ink)
                            .font_weight(FontWeight::SEMIBOLD)
                            .flex_shrink_0(),
                    )
                    .child(
                        small(app, summary.clone(), Token::Ink3)
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate(),
                    )
                    .into_element()
            })
        }));
    let lists = view()
        .style_when(move || !open.get(), |s| s.hidden())
        .accessibility_elements_hidden(move || !open.get())
        .child(
            scroll()
                .max_h(px(PHONE_OPEN_HEIGHT))
                .child(dynamic(move || {
                    fleet.with(|f| match f {
                        Some(f) if f.open => context(app, "session-fleet-open", column(app, f)),
                        _ => view().into_element(),
                    })
                })),
        );
    view()
        .flex_none()
        .border_b(px(1.0))
        .border_color(color(Token::Rule))
        .child(line)
        .child(lists)
        .into_element()
}
