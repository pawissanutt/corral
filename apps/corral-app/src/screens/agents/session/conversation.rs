//! An agent's conversation in the Agents tab's place (`session::Conversation`, tab label
//! `Agents › a2`): a head (‹ Agents, the agent's mark, id, state and title, its profile, model,
//! time, cost and context share, who receives what the composer sends, and its controls), the
//! confirmation or model choices it asks for, then its transcript drawn by the one transcript
//! renderer, following its newest rows like Chat, and how the agent stands after its last
//! message.
//!
//! The transcript keeps the room: on desktop the whole head stays put above it; in a terminal
//! the head is one line with the facts and controls behind More; on the phone the head is that
//! same compact line at the top of the transcript's own scroll, so the conversation reads in
//! all the room the chat has, and only a confirmation or the model choices stay put above it
//! while they wait.
//!
//! Nothing here is built again because the clock moved: the head is mounted from what its
//! buttons are made of, its facts line (running time, cost, context share) follows in place,
//! and the transcript is drawn again only when the transcript changes. test ids
//! `session-conversation`, `btn-agents-back`, `btn-child-more`, `btn-child-end`.

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::model::session::{
    Action, AgentsTab, Conversation, EndLine, FleetButton, Opened, Transcript,
};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::term;
use crate::ui::tokens::{Token, color};

use super::super::super::run;
use super::super::super::transcript;
use super::super::hue_words;
use super::agents_detail::{asking, controls, head, timed, words};
use super::{Tab, cell};

fn of(tab: &AgentsTab) -> Option<&Conversation> {
    match tab.opened.as_ref()? {
        Opened::Conversation(conversation) => Some(conversation),
        Opened::Job(_) => None,
    }
}

/// What the head's controls are built from: everything but its facts line and the transcript.
#[derive(Clone, PartialEq)]
struct Head {
    child: String,
    mark: Mark,
    hue: Hue,
    state: &'static str,
    title: String,
    receiver: Option<String>,
    buttons: Vec<FleetButton>,
    back: Action,
}

impl Head {
    fn of(conversation: &Conversation) -> Self {
        Self {
            child: conversation.child.clone(),
            mark: conversation.mark,
            hue: conversation.hue,
            state: conversation.state,
            title: conversation.title.clone(),
            receiver: conversation.receiver.clone(),
            buttons: conversation.buttons.clone(),
            back: conversation.back.clone(),
        }
    }
}

/// Each conversation is mounted by its full scope. Capture the target itself: NSH
/// disposes owner-local reactive storage before calling the owner's cleanups.
fn pin_child(scope: String, watch: impl Fn(DetailRef, bool) + 'static) {
    let target = DetailRef::Session { id: scope };
    watch(target.clone(), true);
    cleanup(move || watch(target, false));
}

/// The conversation's head, what it asks, and its following transcript.
pub(super) fn page(
    app: AppState,
    tab: Tab,
    scope: String,
    jump: impl Fn() -> u32 + 'static,
) -> AnyElement {
    let form = app.form;
    let phone = form == Form::Phone;
    // A parent subscription does not pin a child's transcript pages or item assemblies.
    let source = untracked(|| app.scope());
    pin_child(scope.clone(), move |target, open| {
        source.watch_detail(target, open)
    });
    let more = signal(false);
    let (top_scope, phone_scope) = (scope.clone(), scope.clone());
    // The side pane already shows this conversation: it gives way here rather than twice.
    let scoped = crate::model::Id::from(scope.as_str());
    let beside = {
        let scoped = scoped.clone();
        // Never inside the pane itself.
        let in_pane = app.dock_panel().is_some();
        computed(move || !in_pane && app.pane_conversation().as_ref() == Some(&scoped))
    };
    let shape = tab.part(|t| of(t).map(Head::of));
    let facts = tab.part(|t| of(t).map(|c| c.facts.clone()));
    let asks = tab.part(|t| t.confirm.is_some() || t.picker.is_some());
    let asked =
        dynamic(move || (asks.get() == Some(true)).then(|| untracked(|| waiting(app, tab))));
    let top = dynamic(move || {
        if phone {
            return None;
        }
        let open = more.get();
        let shown = shape.get().flatten()?;
        Some(untracked(|| {
            bar(app, tab, &shown, &top_scope, facts, open, more)
        }))
    });
    let phone_head = phone.then(|| {
        dynamic(move || {
            let open = more.get();
            let shown = shape.get().flatten()?;
            Some(untracked(|| {
                head_column(app, tab, &shown, &phone_scope, facts, open, more)
                    .pb(px(8.0))
                    .border_b(px(1.0))
                    .border_color(color(Token::Rule))
                    .into_element()
            }))
        })
    });
    let said = tab.part(|t| of(t).map(|c| (c.transcript.clone(), c.end.clone())));
    let body = dynamic(move || {
        let (transcript, end) = said.get().flatten()?;
        Some(untracked(|| content(app, &transcript, end)))
    });
    // Keep the measured transcript wrapper directly in the scroll-content column:
    // its layout then includes the top padding and the phone header (including More).
    let column = match form {
        Form::Terminal => view().px(term::cells(1.0)),
        Form::Desktop => view().px(px(18.0)).py(px(10.0)),
        Form::Phone => view().px(px(12.0)).py(px(8.0)),
    };
    let body = column
        .gap(px(if phone { 10.0 } else { 0.0 }))
        .children(phone_head)
        .child(transcript::content_origin(scope, view().child(body)));
    let surface = transcript::follow(
        app,
        scroll()
            .test_id("session-conversation")
            .flex_1()
            .min_h(px(0.0))
            .child(body),
        move || said.with(|said| said.as_ref()?.as_ref().map(|(t, _)| t.following())),
        move || {
            said.with(|said| {
                said.as_ref()?
                    .as_ref()
                    .map(|(t, _)| transcript::Earlier::of(t))
            })
        },
        move |key| said.with(|said| said.as_ref()?.as_ref()?.0.rows_below(key)),
        jump,
    );
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(top)
        .child(asked)
        .child(when(
            move || beside.get(),
            move || crate::ui::ask::beside_note(app, scoped.clone()),
        ))
        .child(
            view()
                .flex_1()
                .min_h(px(0.0))
                .flex_col()
                .style_when(move || beside.get(), |s| s.hidden())
                .child(surface),
        )
        .into_element()
}

/// The head: the way back and who the agent is, then (on desktop always, elsewhere behind More)
/// its facts line, who receives what is sent and its controls. The facts line follows the
/// clock and the agent's cost in place.
fn head_column(
    app: AppState,
    tab: Tab,
    shown: &Head,
    scope: &str,
    facts: Memo<Option<Option<(String, String)>>>,
    open: bool,
    more: Signal<bool>,
) -> View {
    let form = app.form;
    let back = shown.back.clone();
    let back = button(
        app,
        "‹ Agents",
        ButtonKind::Quiet,
        "btn-agents-back",
        move || run(app, back.clone()),
    );
    let compact = form != Form::Desktop;
    let toggle = compact.then(|| {
        button(
            app,
            if open { "Less" } else { "⋯ More" },
            ButtonKind::Quiet,
            "btn-child-more",
            move || more.update(|open| *open = !*open),
        )
    });
    let who = view()
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 16.0 } else { 10.0 }))
        .child(back)
        .child(
            head(
                app,
                (shown.mark, shown.hue),
                &shown.child,
                shown.state,
                &shown.title,
            )
            .flex_1()
            .min_w(px(0.0)),
        )
        // The conversation on a side pane tab of its own (desktop).
        .children(crate::ui::ask::open_beside(
            app,
            &crate::model::Live::Session(crate::model::Id::from(scope)),
        ))
        .children(toggle);
    let expanded = !compact || open;
    let child = shown.child.clone();
    let facts = expanded.then(|| {
        cell(
            form,
            move || {
                let time = tab.time(&child);
                facts.with(|facts| {
                    facts
                        .as_ref()
                        .and_then(|facts| facts.as_ref())
                        .map(|(lead, tail)| timed(lead, time, tail))
                        .unwrap_or_default()
                })
            },
            Token::Ink3,
            true,
        )
    });
    let receiver = shown
        .receiver
        .clone()
        .filter(|_| expanded)
        .map(|said| words(form, format!("→ {said}"), color(Token::Ink2)).into_element());
    let buttons = expanded.then(|| controls(app, &shown.buttons));
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(who)
        .children(facts)
        .children(receiver)
        .children(buttons)
}

/// The head where it stays put: desktop and terminal above the transcript.
fn bar(
    app: AppState,
    tab: Tab,
    shown: &Head,
    scope: &str,
    facts: Memo<Option<Option<(String, String)>>>,
    open: bool,
    more: Signal<bool>,
) -> AnyElement {
    let column = head_column(app, tab, shown, scope, facts, open, more);
    match app.form {
        Form::Terminal => view()
            .flex_none()
            .child(column.px(term::cells(1.0)))
            .child(term::hrule())
            .into_element(),
        _ => column
            .flex_none()
            .px(px(18.0))
            .py(px(10.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .bg(color(Token::BgSunk))
            .into_element(),
    }
}

/// What waits on the owner above the transcript while it waits: a confirmation or the model
/// choices.
fn waiting(app: AppState, tab: Tab) -> AnyElement {
    let asked = view().flex_none().child(asking(app, tab));
    match app.form {
        Form::Terminal => asked
            .px(term::cells(1.0))
            .child(term::hrule())
            .into_element(),
        Form::Desktop => asked.px(px(18.0)).py(px(8.0)).into_element(),
        Form::Phone => asked
            .px(px(12.0))
            .py(px(6.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .into_element(),
    }
}

/// The transcript, then how the agent stands after it (a question presses to its card).
fn content(app: AppState, said: &Transcript, end: Option<EndLine>) -> AnyElement {
    let form = app.form;
    let end = end.map(|end| {
        let line = words(form, end.words.clone(), hue_words(form, end.hue))
            .when(end.hue == Hue::You, |t| t.font_weight(FontWeight::SEMIBOLD));
        let line = view().test_id("btn-child-end").child(line);
        match end.action {
            Some(action) => line
                .role(AccessibilityRole::Button)
                .label(format!("{}; press to answer", end.words))
                .cursor_pointer()
                .on_press(move || run(app, action.clone()))
                .into_element(),
            None => line.into_element(),
        }
    });
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
        .child(transcript::transcript(app, said))
        .children(end)
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_core::element::{Runtime, create_root};
    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn disposing_child_releases_the_mounted_scope() {
        let runtime = Runtime::default();
        let _entered = runtime.enter();
        let scope = "B-parent/child/reader";
        let events = Rc::new(RefCell::new(Vec::new()));
        let (owner, ()) = create_root(|| {
            let events = events.clone();
            pin_child(scope.into(), move |target, open| {
                events.borrow_mut().push((target, open));
            });
        });
        owner.dispose();
        let child = DetailRef::Session { id: scope.into() };
        assert_eq!(*events.borrow(), [(child.clone(), true), (child, false)]);
    }
}
