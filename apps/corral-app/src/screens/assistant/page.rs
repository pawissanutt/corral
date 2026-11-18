//! The thread page: its head with the session's controls, the transcript with the composer
//! docked under it, the session's facts beside them; and the same as a pushed phone page, all
//! from the served thread.

use std::rc::Rc;

use nsh_core::prelude::*;

use crate::model::assistant::{
    Action, Button, Fact, LABELS, QUICK, ThreadHeader, ThreadRef, live_thread_header, quick_id,
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
use crate::ui::kit::page::{Width, columns, context, pushed_head};
use crate::ui::kit::text::{h3, label_value, rich_small};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::parts::{act, model_button, note_size, spacer, words};
use super::{open_scope, side_facts, transcript, watch_ops};

/// The facts column beside the transcript (`grid-template-columns: minmax(0,1fr) 300px`).
const FACTS_WIDTH: f32 = 300.0;
/// The transcript rows' inset on the page (`.tm` padding 8 24); the phone's 14.
const PAGE_INSET: f32 = 24.0;
const PHONE_INSET: f32 = 14.0;

/// The served thread `id` ("quick" or A-n) on desktop and in a terminal, subscribed while it is
/// shown: the head (back to Agents, title, the thread's state, where it runs), the transcript
/// with its composer docked under it, and the session's facts beside them. A terminal narrower
/// than its wide grid keeps the transcript alone.
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
        .child({
            // An ended session takes no more messages: no composer under its transcript.
            let ThreadRef::Live(scope) = source.clone();
            let ended = computed(move || {
                app.feed
                    .with(|feed| crate::model::assistant::ended(feed, scope.as_str()))
            });
            let source = source.clone();
            dynamic(move || (!ended.get()).then(|| untracked(|| ask::docked(app, source.clone()))))
        })
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
        .child(head(app, &source))
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
/// it runs and, at the trailing edge, the Session menu and End session while the served session
/// can be ended. A terminal uses the kit's pushed head with the chip, the menu and End at its
/// end. The head follows the thread's facts part by part: the Session menu is drawn anew only
/// for another session, so the state a control changes under the open menu leaves its trigger
/// mounted for the keyboard focus the closing menu puts back on it.
fn head(app: AppState, source: &ThreadRef) -> AnyElement {
    let shown = {
        let source = source.clone();
        computed(move || header(app, &source))
    };
    let terminal = app.form == Form::Terminal;
    let gap = move |row: View| {
        if terminal {
            row.child(term::space(2.0))
        } else {
            row
        }
    };
    let session = computed(move || shown.with(|header| header.menu.clone()));
    let menu = dynamic(move || {
        session.get().map(|session| {
            gap(view().flex_row().items_center())
                .child(crate::screens::agents::session_menu(app, session))
        })
    });
    let end = dynamic(move || {
        shown.with(|header| header.end.clone()).map(|end| {
            gap(view().flex_row().items_center()).child(end_button(app, end, ButtonKind::Ghost))
        })
    });
    let state = move || {
        dynamic(move || {
            shown.with(|header| {
                header
                    .state
                    .as_ref()
                    .map(|state| chip(app, state.label.clone(), state.hue))
            })
        })
    };
    if terminal {
        let trailing = view()
            .flex_row()
            .items_center()
            .child(state())
            .child(menu)
            .child(end)
            .into_element();
        return pushed_head(
            app,
            move || SharedString::from(shown.with(|header| header.title.clone())),
            move || SharedString::from(shown.with(|header| header.back_label)),
            move || app.back(),
            Some(trailing),
        );
    }
    let back = untracked(|| shown.with(|header| header.back_label));
    let title = dynamic(move || {
        text(shown.with(|header| header.title.clone()))
            .test_id("page-title")
            .role(AccessibilityRole::Header)
            .flex_shrink(1.0)
            .min_w(px(0.0))
            .truncate()
            .font_family(FONT_TEXT)
            .text_size(px(15.0))
            .font_weight(FontWeight::BOLD)
            .letter_spacing(px(-0.08))
            .text_color(color(Token::Ink))
    });
    let subtitle = dynamic(move || {
        words(
            app,
            shown.with(|header| header.subtitle.clone()),
            12.0,
            Token::Ink3,
        )
        .number_of_lines(1u32)
        .flex_shrink(1.0)
        .min_w(px(0.0))
    });
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
        .child(title)
        .child(state())
        .child(subtitle)
        .child(spacer())
        .child(menu)
        .child(end)
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
        foot: None,
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

/// The session's facts (`Session`: id, state, where, profile, cost and what is known besides)
/// and how long its host runs.
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
