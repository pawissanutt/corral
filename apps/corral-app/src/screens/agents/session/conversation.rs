//! An agent's conversation in the Agents tab's place (`session::Conversation`, tab label
//! `Agents › a2`): a head that stays put (‹ Agents, the agent's mark, id, state and title, its
//! profile, model, time, cost and context share, who receives what the composer sends, the
//! confirmation or model choices it asks for, and its controls), then its transcript drawn by
//! the one transcript renderer, following its newest rows like Chat, and how the agent stands
//! after its last message. test ids `session-conversation`, `btn-agents-back`,
//! `btn-child-end`.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{AgentsTab, Conversation, Opened};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::term;
use crate::ui::tokens::{Token, color};

use super::super::super::run;
use super::super::super::transcript;
use super::super::hue_words;
use super::agents_detail::{asked, controls, head, words};
use super::cell;

fn opened(tab: &Option<AgentsTab>) -> Option<&Conversation> {
    match tab.as_ref()?.opened.as_ref()? {
        Opened::Conversation(conversation) => Some(conversation),
        Opened::Job(_) => None,
    }
}

/// The conversation's head and its following transcript.
pub(super) fn page(app: AppState, tab: Memo<Option<AgentsTab>>) -> AnyElement {
    let form = app.form;
    let top = dynamic(move || {
        tab.with(|tab| {
            let asking = tab.as_ref().and_then(|tab| untracked(|| asked(app, tab)));
            opened(tab).map(|conversation| untracked(|| bar(app, conversation, asking)))
        })
    });
    let body = dynamic(move || {
        tab.with(|tab| opened(tab).map(|conversation| untracked(|| content(app, conversation))))
    });
    let body = match form {
        Form::Terminal => view().px(term::cells(1.0)).child(body).into_element(),
        Form::Desktop => view().px(px(18.0)).py(px(10.0)).child(body).into_element(),
        Form::Phone => view().px(px(12.0)).py(px(8.0)).child(body).into_element(),
    };
    let surface = transcript::follow(
        app,
        scroll()
            .test_id("session-conversation")
            .flex_1()
            .min_h(px(0.0))
            .child(body),
        move || tab.with(|tab| opened(tab).map_or(0, |c| c.transcript.rows())),
        || 0,
    );
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(top)
        .child(surface)
        .into_element()
}

/// The head: the way back, who the agent is and how it stands, then what it asks of the owner
/// and its controls.
fn bar(app: AppState, conversation: &Conversation, asked: Option<AnyElement>) -> AnyElement {
    let form = app.form;
    let back = conversation.back.clone();
    let back = button(
        app,
        "‹ Agents",
        ButtonKind::Quiet,
        "btn-agents-back",
        move || run(app, back.clone()),
    );
    let who = view()
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 16.0 } else { 10.0 }))
        .child(back)
        .child(
            head(
                app,
                (conversation.mark, conversation.hue),
                &conversation.child,
                conversation.state,
                &conversation.title,
            )
            .flex_1()
            .min_w(px(0.0)),
        );
    let receiver = conversation
        .receiver
        .clone()
        .map(|said| words(form, format!("→ {said}"), color(Token::Ink2)).into_element());
    let column = view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(who)
        .child(cell(form, conversation.facts.clone(), Token::Ink3, true))
        .children(receiver)
        .children(asked)
        .child(controls(app, &conversation.buttons));
    match form {
        Form::Terminal => view()
            .flex_none()
            .child(column.px(term::cells(1.0)))
            .child(term::hrule())
            .into_element(),
        Form::Desktop => column
            .flex_none()
            .px(px(18.0))
            .py(px(10.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .bg(color(Token::BgSunk))
            .into_element(),
        Form::Phone => column
            .flex_none()
            .px(px(12.0))
            .py(px(8.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .bg(color(Token::BgSunk))
            .into_element(),
    }
}

/// The transcript, then how the agent stands after it (a question presses to its card).
fn content(app: AppState, conversation: &Conversation) -> AnyElement {
    let form = app.form;
    let end = conversation.end.clone().map(|end| {
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
        .child(transcript::transcript(app, &conversation.transcript))
        .children(end)
        .into_element()
}
