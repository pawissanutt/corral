//! The thread page: its head, the transcript with the composer docked under it, the session's
//! facts beside them; the same as a pushed phone page; and a pending assistant prompt as Inbox's
//! detail pane shows it, all from the served thread.

use std::rc::Rc;

use nsh_core::prelude::*;

use crate::model::assistant::{
    Action, Button, Fact, LABELS, LivePrompt, PromptCard, PromptPane, QUICK, ThreadHeader,
    ThreadRef, live_prompt_pane, live_thread_header, quick_id,
};
use crate::model::{Form, Id};
use crate::screens::{PhoneBar, PhonePage};
use crate::state::AppState;
use crate::ui::ask;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::dot;
use crate::ui::kit::overlay::{self, Confirm, Modal};
use crate::ui::kit::page::{Width, body, columns, context, pushed_head, qhead};
use crate::ui::kit::text::{h3, label_value, rich_small};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::parts::{act, model_button, note_size, ring, spacer, words};
use super::{cards, open_scope, side_facts, transcript, watch_ops};

/// The facts column beside the transcript (`grid-template-columns: minmax(0,1fr) 300px`).
const FACTS_WIDTH: f32 = 300.0;
/// The transcript rows' inset on the page (`.tm` padding 8 24); the phone's 14.
const PAGE_INSET: f32 = 24.0;
const PHONE_INSET: f32 = 14.0;
/// The prompt pane's measure (`max-width: 760px`).
const PROMPT_MEASURE: f32 = 760.0;

/// The served thread `id` ("quick" or A-n) on desktop and in a terminal, subscribed while it is
/// shown: the head (back to Agents, title, the thread's state, where it runs), the transcript
/// with its composer docked under it, and the session's facts (with the Ideas filed from it)
/// beside them. A terminal narrower than its wide grid keeps the transcript alone.
pub fn live_thread(app: AppState, id: Id) -> AnyElement {
    open_scope(app, id.clone(), false);
    watch_ops(app);
    page(app, ThreadRef::Live(id))
}

fn page(app: AppState, source: ThreadRef) -> AnyElement {
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
        .child(transcript::list(app, source.clone(), inset))
        .child(ask::docked(app, source.clone()))
        .into_element();
    let facts = {
        let source = source.clone();
        context(app, "thread-facts", dynamic(move || facts(app, &source)))
    };
    let end = end_dialog(app, &source);
    // The host is a column: growing along it fills the frame, so the transcript's scroll takes
    // the height and the composer sits at the bottom. A percentage height does not resolve
    // against the host's flexed size and collapsed the page to its content.
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .bg(color(Token::Bg))
        .child(dynamic(move || Some(head(app, &source))))
        .child(columns(
            app,
            vec![
                (Width::Fill, conversation),
                (Width::Fixed(FACTS_WIDTH), facts),
            ],
            0,
        ))
        .children(end)
        .into_element()
}

/// The thread's head and facts, tracked.
fn header(app: AppState, source: &ThreadRef) -> ThreadHeader {
    let ThreadRef::Live(scope) = source;
    app.feed.with(|feed| {
        app.assistant
            .with(|state| live_thread_header(state, feed, scope.as_str()))
    })
}

/// The page head (`.pagehead`): a quiet back button, the title, the thread's state chip, where
/// it runs and, at the trailing edge, End session while the served session can be ended. A
/// terminal uses the kit's pushed head with the chip and End at its end.
fn head(app: AppState, source: &ThreadRef) -> AnyElement {
    let header = header(app, source);
    let state = header
        .state
        .as_ref()
        .map(|state| chip(app, state.label.clone(), state.hue));
    let end = header
        .end
        .clone()
        .map(|end| end_button(app, end, ButtonKind::Ghost));
    let title = header.title;
    let back = header.back_label;
    if app.form == Form::Terminal {
        let trailing = match (state, end) {
            (None, None) => None,
            (state, end) => Some(
                view()
                    .flex_row()
                    .items_center()
                    .children(state)
                    .when_some(end, |row, end| row.child(term::space(2.0)).child(end))
                    .into_element(),
            ),
        };
        return pushed_head(app, title, back, move || app.back(), trailing);
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
        .child(
            words(app, header.subtitle, 12.0, Token::Ink3)
                .number_of_lines(1u32)
                .flex_shrink(1.0)
                .min_w(px(0.0)),
        )
        .when_some(end, |row, end| row.child(spacer()).child(end))
        .into_element()
}

/// End session as `kind` (outlined beside the desktop title, words alone in a phone's head).
/// The model offers it only on a served session (or the quick thread's current generation)
/// corrald has not ended.
fn end_button(app: AppState, mut end: Button, kind: ButtonKind) -> AnyElement {
    if app.form == Form::Phone {
        // The pushed head's trailing slot sits beside the state chip; the title names the session.
        end.label = "End";
    }
    model_button(app, &end, kind)
}

/// The fresh confirmation End asks for on this device, over the page while `confirm_end`
/// names this served session or the quick thread. Confirming sends the canonical
/// `assistant.session` End; Cancel, Escape and the scrim leave the session running.
fn end_dialog(app: AppState, source: &ThreadRef) -> Option<AnyElement> {
    let ThreadRef::Live(scope) = source;
    let open = scope.clone();
    let session = scope.clone();
    Some(overlay::modal(
        app,
        move || {
            app.assistant
                .with(|state| state.live.confirm_end.as_deref() == Some(open.as_str()))
        },
        move || act(app, Action::EndSession(None)),
        move || end_modal(app, session.as_str()),
    ))
}

/// What ending a session does, in corrald's terms (`corral assistant end`), then Cancel and End.
fn end_modal(app: AppState, session: &str) -> Modal {
    let form = app.form;
    let ending = session.to_owned();
    let (title, sub) = if session == QUICK {
        let current = app.feed.with(quick_id);
        (
            "End quick thread".to_owned(),
            format!(
                "Ends {}; your next message starts a new quick thread.",
                current.as_deref().unwrap_or("its current session")
            ),
        )
    } else {
        (
            format!("End {session}"),
            "Ends this session for good; it cannot be resumed.".to_owned(),
        )
    };
    Modal {
        test_id: "end-session".into(),
        title,
        sub: Some(sub),
        body: words(
            app,
            "corrald asks its host to stop, cancels what it was waiting to do and closes the \
             session. Its transcript, summary and scratch records stay readable.",
            note_size(form),
            Token::Ink2,
        )
        .into_element(),
        done: None,
        confirm: Some(Confirm {
            label: if form == Form::Phone {
                "End session".into()
            } else {
                format!("End {session}")
            },
            cancel: "Cancel".into(),
            word: "end".into(),
            check: None,
            on_confirm: Rc::new(move || act(app, Action::ConfirmEnd(ending.clone()))),
        }),
    }
}

/// The session's facts (`Session`: id, state, where, profile, cost and what is known besides),
/// how long its host runs, the served session's canonical actions, and the Ideas filed from
/// this thread.
fn facts(app: AppState, source: &ThreadRef) -> Vec<AnyElement> {
    let header = header(app, source);
    let form = app.form;
    let mut out = vec![
        h3(app, LABELS.session, None),
        view()
            .gap(px(if form == Form::Terminal { 0.0 } else { 5.0 }))
            .children(header.facts.iter().map(|fact| fact_row(app, fact)))
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
                        .into_iter()
                        .map(|row| filed_row(app, row.id, row.title, row.action)),
                )
                .into_element(),
        );
    }
    out
}

/// One session fact: desktop and phone the label column beside the value; a terminal the label
/// over the value, its id cut to one line (`side_facts`).
fn fact_row(app: AppState, fact: &Fact) -> AnyElement {
    if app.form == Form::Terminal {
        return side_facts::stacked(app, fact.label, side_facts::identified(app, &fact.value));
    }
    label_value(app, fact.label, rich_small(app, &fact.value))
}

/// A filed row of the facts column (`.irow`): the mono id and the title cut to fit.
fn filed_row(
    app: AppState,
    id: String,
    title: String,
    action: crate::model::assistant::Action,
) -> AnyElement {
    let form = app.form;
    let id_text = text(id.clone())
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
        .on_press(move || act(app, action.clone()))
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

/// The served thread `id` as a pushed phone page, subscribed while it is shown.
pub fn live_thread_phone(app: AppState, id: Id) -> PhonePage {
    open_scope(app, id.clone(), false);
    watch_ops(app);
    phone_page(app, ThreadRef::Live(id))
}

/// The served thread's phone title, following the thread: the quick thread's current session
/// changes when it rolls to the next generation (A-9 → A-10) while its page stays pushed.
pub fn live_thread_title(app: AppState, id: Id) -> Memo<String> {
    let source = ThreadRef::Live(id);
    computed(move || header(app, &source).phone_title)
}

/// The thread as a pushed phone page: the compact title, the thread's state and
/// End (while the served session can be ended) at the trailing edge, the transcript scrolling
/// itself, the composer docked in place of the tab bar and no floating Ask button.
fn phone_page(app: AppState, source: ThreadRef) -> PhonePage {
    let title = untracked(|| header(app, &source)).phone_title;
    let trailing = {
        let source = source.clone();
        dynamic(move || {
            let current = header(app, &source);
            let end = current
                .end
                .map(|end| end_button(app, end, ButtonKind::Quiet));
            // A 390 pt header has no room for the session's title and a worded chip: the state is
            // its colour mark here, named for screen readers; the words are in the transcript.
            let state = current.state.map(|state| {
                view()
                    .label(state.label)
                    .px(px(4.0))
                    .child(dot(app, state.hue))
                    .into_element()
            });
            (state.is_some() || end.is_some()).then(|| {
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(2.0))
                    .pr(px(if end.is_some() { 0.0 } else { 6.0 }))
                    .children(state)
                    .children(end)
                    .into_element()
            })
        })
    };
    let body = view()
        .flex_1()
        .flex_col()
        .min_h(px(0.0))
        .child(transcript::list(app, source.clone(), PHONE_INSET))
        .children(end_dialog(app, &source))
        .into_element();
    PhonePage {
        title,
        body,
        trailing: Some(trailing),
        bar: Some(PhoneBar::always(ask::docked(app, source))),
        ask: false,
        scroll: false,
    }
}

/// Prompt `key` of thread `thread` as Inbox's detail pane shows it, with its real answers; the
/// thread is subscribed while the pane is shown.
pub fn live_prompt(app: AppState, thread: Id, key: Id) -> AnyElement {
    open_scope(app, thread.clone(), false);
    dynamic(move || {
        let pane = app.feed.with(|feed| {
            app.assistant
                .with(|state| live_prompt_pane(state, feed, thread.as_str(), key.as_str()))
        });
        Some(match pane {
            LivePrompt::Pane(pane) => prompt_view(app, &pane),
            LivePrompt::Loading => notice(app, "Loading the prompt from its thread…".into()),
            LivePrompt::Missing(why) => notice(app, why),
        })
    })
}

fn notice(app: AppState, said: String) -> AnyElement {
    let form = app.form;
    let line = words(app, said, note_size(form), Token::Ink3);
    match form {
        Form::Phone => view().px(px(18.0)).py(px(14.0)).child(line).into_element(),
        Form::Desktop | Form::Terminal => body(app, "prompt-pane", line.into_element()),
    }
}

/// The needs-you chip and where it came from, the title, the card shared with the thread, that
/// one answer wins, and the way to the thread. Desktop and terminal: it fills its container and
/// scrolls its body itself. Phone: the same content unscrolled, for a page body.
fn prompt_view(app: AppState, pane: &PromptPane) -> AnyElement {
    let form = app.form;
    let meta = vec![
        chip(app, pane.state.label.clone(), pane.state.hue),
        words(app, pane.origin.clone(), note_size(form), Token::Ink3)
            .flex_shrink(1.0)
            .min_w(px(0.0))
            .into_element(),
    ];
    let card = match &pane.card {
        PromptCard::Permission(card) => cards::permission(app, card),
        PromptCard::Confirmation(card) => cards::action(app, card),
    };
    let content = view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
        .when(form == Form::Desktop, |c| c.max_w(px(PROMPT_MEASURE)))
        .child(card)
        .when(form == Form::Terminal, |c| c.child(term::blank()))
        .child(words(app, pane.note, note_size(form), Token::Ink3))
        .child(
            view()
                .flex_row()
                .child(model_button(app, &pane.open, ButtonKind::Quiet)),
        )
        .into_element();
    let head = qhead(app, meta, pane.title.clone(), None, Vec::new());
    match form {
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
    }
}
