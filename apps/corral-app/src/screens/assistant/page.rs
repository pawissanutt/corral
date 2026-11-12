//! The thread page (`Page::Thread`): its head, the transcript with the composer docked under
//! it, the session's facts beside them; the same as a pushed phone page; and a pending assistant
//! prompt as Inbox's detail pane shows it.

use nsh_core::prelude::*;

use crate::model::assistant::{
    LABELS, Prompt, PromptCard, ThreadHeader, prompt_pane, thread_header,
};
use crate::model::effect::Page;
use crate::model::{Destination, Form};
use crate::screens::PhonePage;
use crate::state::AppState;
use crate::ui::ask;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::page::{Width, body, columns, context, pushed_head, qhead};
use crate::ui::kit::text::{h3, label_value, rich_small};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::cards;
use super::parts::{model_button, note_size, ring, words};
use super::transcript;

/// The facts column beside the transcript (`grid-template-columns: minmax(0,1fr) 300px`).
const FACTS_WIDTH: f32 = 300.0;
/// The transcript rows' inset on the page (`.tm` padding 8 24); the phone's 14.
const PAGE_INSET: f32 = 24.0;
const PHONE_INSET: f32 = 14.0;
/// The prompt pane's measure (`max-width: 760px`).
const PROMPT_MEASURE: f32 = 760.0;

/// The thread open in `app.assistant` on desktop and in a terminal: the head (back to Agents,
/// title, the quick thread's state, where it runs), the transcript with its composer docked under
/// it, and the session's facts (with the Ideas filed from the quick thread) beside them. A
/// terminal narrower than its wide grid keeps the transcript alone.
pub fn thread(app: AppState) -> AnyElement {
    let id = computed(move || app.assistant.with(|state| state.thread));
    dynamic(move || Some(page(app, id.get())))
}

fn page(app: AppState, id: &'static str) -> AnyElement {
    let inset = if app.form == Form::Terminal {
        0.0
    } else {
        PAGE_INSET
    };
    let conversation = view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(transcript::list(app, id, inset))
        .child(ask::docked(app, id))
        .into_element();
    let facts = context(app, "thread-facts", dynamic(move || facts(app, id)));
    view()
        .size_full()
        .flex_col()
        .bg(color(Token::Bg))
        .child(dynamic(move || Some(head(app, id))))
        .child(columns(
            app,
            vec![
                (Width::Fill, conversation),
                (Width::Fixed(FACTS_WIDTH), facts),
            ],
            0,
        ))
        .into_element()
}

fn header(app: AppState, id: &'static str) -> Option<ThreadHeader> {
    let env = app.env();
    app.assistant.with(|state| thread_header(state, id, env))
}

/// The page head (`.pagehead`): a quiet back button, the title, the quick thread's state chip and
/// where it runs. A terminal uses the kit's pushed head with the chip at its end.
fn head(app: AppState, id: &'static str) -> AnyElement {
    let header = header(app, id);
    let title = header
        .as_ref()
        .map_or_else(|| id.to_owned(), |header| header.title.clone());
    let back = header
        .as_ref()
        .map_or(Destination::Agents.label(), |header| header.back_label);
    let state = header
        .as_ref()
        .and_then(|header| header.state.as_ref())
        .map(|state| chip(app, state.label.clone(), state.hue));
    if app.form == Form::Terminal {
        return pushed_head(app, title, back, move || app.back(), state);
    }
    view()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(12.0))
        .h(px(46.0))
        .px(px(18.0))
        .border_b(px(1.0))
        .border_color(color(Token::Rule))
        .child(button_with(
            app,
            back,
            ButtonKind::Quiet,
            ButtonOptions {
                icon: Some(Icon::Back),
                ..ButtonOptions::default()
            },
            "page-back",
            move || app.back(),
        ))
        .child(
            text(title)
                .test_id("page-title")
                .role(AccessibilityRole::Header)
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .truncate()
                .font_family(FONT_TEXT)
                .text_size(px(15.0))
                .font_weight(FontWeight::BOLD)
                .letter_spacing(px(-0.08))
                .text_color(color(Token::Ink)),
        )
        .children(state)
        .when_some(header.map(|header| header.subtitle), |row, sub| {
            row.child(
                words(app, sub, 12.0, Token::Ink3)
                    .number_of_lines(1u32)
                    .flex_shrink(1.0)
                    .min_w(px(0.0)),
            )
        })
        .into_element()
}

/// The session's facts (`Session`: id, state, where, profile, cost, scratch), how long its host
/// runs, and the Ideas filed from this thread.
fn facts(app: AppState, id: &'static str) -> Vec<AnyElement> {
    let Some(header) = header(app, id) else {
        return Vec::new();
    };
    let form = app.form;
    let mut out = vec![
        h3(app, LABELS.session, None),
        view()
            .gap(px(if form == Form::Terminal { 0.0 } else { 5.0 }))
            .children(
                header
                    .facts
                    .iter()
                    .map(|fact| label_value(app, fact.label, rich_small(app, &fact.value))),
            )
            .into_element(),
    ];
    let note = words(app, header.lifetime_note, note_size(form), Token::Ink3);
    out.push(if form == Form::Terminal {
        view().child(term::blank()).child(note).into_element()
    } else {
        view().mt(px(8.0)).child(note).into_element()
    });
    if !header.filed.is_empty() {
        if form == Form::Terminal {
            out.push(term::blank());
        }
        out.push(h3(app, LABELS.filed_from_thread, None));
        out.push(
            view()
                .gap(px(2.0))
                .children(
                    header
                        .filed
                        .iter()
                        .map(|row| filed_row(app, row.id, row.title)),
                )
                .into_element(),
        );
    }
    out
}

/// A filed row of the facts column (`.irow`): the mono id and the title cut to fit.
fn filed_row(app: AppState, id: &'static str, title: &'static str) -> AnyElement {
    let form = app.form;
    let id_text = text(id)
        .number_of_lines(1u32)
        .text_color(color(Token::Ink3));
    let id_text = if form == Form::Terminal {
        id_text
    } else {
        id_text.font_family(FONT_MONO).text_size(px(11.5))
    };
    let row = view()
        .flex_row()
        .items_center()
        .role(AccessibilityRole::Button)
        .label(format!("{id} {title}"))
        .test_id(&format!("task-{id}"))
        .cursor_pointer()
        .on_press(move || app.open(Page::Task(id)))
        .child(
            view()
                .w(if form == Form::Terminal {
                    term::cells(6.0)
                } else {
                    px(46.0)
                })
                .flex_shrink_0()
                .child(id_text),
        )
        .child(
            words(app, title, 12.5, Token::Ink)
                .number_of_lines(1u32)
                .truncate()
                .flex_1()
                .min_w(px(0.0)),
        );
    if form == Form::Terminal {
        return row.into_element();
    }
    row.gap(px(8.0))
        .px(px(4.0))
        .py(px(6.0))
        .rounded(px(6.0))
        .hover(|s| s.bg(color(Token::Hover)))
        .focus_visible(ring)
        .into_element()
}

/// The thread as a pushed phone page: the compact title, back to Agents, the quick thread's state
/// at the trailing edge, the transcript scrolling itself, the composer docked in place of the tab
/// bar and no floating Ask button.
pub fn thread_phone(app: AppState) -> PhonePage {
    let id = app.assistant.with_untracked(|state| state.thread);
    let env = app.env();
    let info = app
        .assistant
        .with_untracked(|state| thread_header(state, id, env));
    let title = info
        .as_ref()
        .map_or_else(|| id.to_owned(), |info| info.phone_title.to_owned());
    let back = info
        .as_ref()
        .map_or(Destination::Agents.label(), |info| info.back_label);
    let trailing = info.as_ref().and_then(|info| info.state.as_ref()).map(|_| {
        dynamic(move || {
            header(app, id)
                .and_then(|current| current.state)
                .map(|state| {
                    view()
                        .pr(px(6.0))
                        .child(chip(app, state.label, state.hue))
                        .into_element()
                })
        })
    });
    PhonePage {
        title,
        back,
        body: transcript::list(app, id, PHONE_INSET),
        trailing,
        bar: Some(ask::docked(app, id)),
        ask: false,
        scroll: false,
    }
}

/// One pending assistant prompt as Inbox's detail pane shows it: the needs-you chip and where it
/// came from, the title, the card shared with the quick thread, that one answer wins, and the way
/// to the quick thread. Desktop and terminal: it fills its container and scrolls its body itself.
/// Phone: the same content unscrolled, for a page body.
pub fn prompt(app: AppState, which: Prompt) -> AnyElement {
    dynamic(move || {
        let pane = prompt_pane(which, app.env());
        let form = app.form;
        let meta = vec![
            chip(app, pane.state.label.clone(), pane.state.hue),
            words(app, pane.origin, note_size(form), Token::Ink3)
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .into_element(),
        ];
        let card = match &pane.card {
            PromptCard::Permission(card) => cards::permission(app, card),
            PromptCard::Confirmation(card) => cards::action(app, card),
        };
        let open = pane.open.clone();
        let content = view()
            .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
            .when(form == Form::Desktop, |c| c.max_w(px(PROMPT_MEASURE)))
            .child(card)
            .when(form == Form::Terminal, |c| c.child(term::blank()))
            .child(words(app, pane.note, note_size(form), Token::Ink3))
            .child(
                view()
                    .flex_row()
                    .child(model_button(app, &open, ButtonKind::Quiet)),
            )
            .into_element();
        let head = qhead(app, meta, pane.title, None, Vec::new());
        Some(match form {
            Form::Phone => view()
                .child(head)
                .child(view().px(px(18.0)).py(px(14.0)).child(content))
                .into_element(),
            Form::Desktop | Form::Terminal => view()
                .flex_1()
                .flex_col()
                .min_h(px(0.0))
                .child(head)
                .child(body(app, "prompt-pane", content))
                .into_element(),
        })
    })
}
