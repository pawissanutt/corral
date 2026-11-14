//! The session page: its header and Transcript | Terminal switch, the transcript with its reply
//! box beside the run (`.side-col`), or the terminal (`.termbar`, `.bigterm`); on the phone the
//! segmented switch (`.pseg`), the docked reply or the terminal's keys. A session without a
//! terminal (a daemon-hosted assistant has no pane) shows no switch and no terminal.

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::agents::{Action, FactValue, Feed, Run, Session, View};
use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::model::text::span;
use crate::model::{Form, Id};
use crate::screens::PhonePage;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with, seg};
use crate::ui::kit::marks::rail;
use crate::ui::kit::notice::notice;
use crate::ui::kit::page::{Width, bar, body, columns, context, pushed_head};
use crate::ui::kit::table::{kv, steps};
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::console::{self, BIG, PHONE};
use super::run;
use super::transcript::{line, phone_reply, reply, transcript};

/// The run column's width (`minmax(0, 1fr) 280px`).
const RUN_WIDTH: f32 = 280.0;

/// The Transcript | Terminal switch, reading the open view.
fn switch(app: AppState, session: Memo<Session>) -> AnyElement {
    let (labels, actions) =
        untracked(move || session.with(|s| (s.switch_labels, s.switch_actions.clone())));
    seg(
        app,
        &[(View::Transcript, labels[0]), (View::Terminal, labels[1])],
        move || session.with(|s| s.view),
        move |view| {
            let action = if view == View::Transcript {
                actions[0].clone()
            } else {
                actions[1].clone()
            };
            run(app, action);
        },
        "seg-session",
    )
}

/// One draft per open session page, kept while the view switches. A delivered reply clears it
/// once and refetches the transcript, which now holds the reply; a refused or unknown one keeps
/// the words so nothing typed is lost.
fn draft_for(app: AppState, session: Memo<Session>, id: Option<Id>) -> Signal<String> {
    let draft = signal(String::new());
    let delivered = computed(move || session.with(|s| s.reply.delivered.clone()));
    let seen = stored(None::<Option<String>>);
    effect(move || {
        let now = delivered.get();
        if let Some(before) = seen.replace(Some(now.clone()))
            && before != now
            && now.is_some()
        {
            draft.set(String::new());
            if let Some(id) = &id {
                let target = DetailRef::Session { id: id.to_string() };
                untracked(|| run(app, Action::Refresh(target)));
            }
        }
    });
    draft
}

/// Desktop and terminal: the head, then any notice about the session, then the open view.
pub fn page(app: AppState, session: Memo<Session>, id: Option<Id>) -> AnyElement {
    let draft = draft_for(app, session, id);
    // The head follows the open session and whether it has a terminal, not its view: the
    // switch keeps its focus.
    let key = computed(move || session.with(|s| (s.id.clone(), s.terminal.is_some())));
    let top = dynamic(move || {
        let (_, terminal) = key.get();
        untracked(move || {
            let control = terminal.then(|| switch(app, session));
            session.with(|s| head(app, s, control))
        })
    });
    let content = dynamic(move || {
        session.with(|s| {
            let surface = match (s.view, &s.terminal) {
                (View::Terminal, Some(_)) => terminal_view(app, s),
                _ => transcript_view(app, s, draft),
            };
            match &s.notice {
                Some((hue, words)) => view()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex_col()
                    .child(
                        view()
                            .flex_none()
                            .when(app.form != Form::Terminal, |n| n.px(px(18.0)).pt(px(10.0)))
                            .child(notice(app, *hue, &vec![span(words.clone())], None)),
                    )
                    .child(surface)
                    .into_element(),
                None => surface,
            }
        })
    });
    view()
        .size_full()
        .flex_col()
        .when(app.form != Form::Terminal, |page| page.bg(color(Token::Bg)))
        .child(top)
        .child(view().flex_1().min_h(px(0.0)).flex_col().child(content))
        .into_element()
}

/// The page head. Desktop (`.pagehead`): back to Agents, the id in quiet mono and the title in
/// bold, the run and agent, then the `` ` `` key and the switch at the trailing edge when the
/// session has a terminal. Terminal: the kit's pushed head with the key's legend and the switch.
fn head(app: AppState, session: &Session, switch: Option<AnyElement>) -> AnyElement {
    let back = session.back.clone();
    if app.form == Form::Terminal {
        return pushed_head(
            app,
            format!("{} {}", session.id, session.title),
            session.desktop_back_label,
            move || run(app, back.clone()),
            switch.map(|switch| {
                view()
                    .flex_row()
                    .items_center()
                    .child(term::legend(session.switch_key, session.switch_hint))
                    .child(term::space(2.0))
                    .child(switch)
                    .into_element()
            }),
        );
    }
    let has_switch = switch.is_some();
    view()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(12.0))
        .h(px(46.0))
        .px(px(18.0))
        .border_b_1()
        .border_b_color(color(Token::Rule))
        .child(button_with(
            app,
            session.desktop_back_label,
            ButtonKind::Quiet,
            ButtonOptions {
                icon: Some(Icon::Back),
                ..ButtonOptions::default()
            },
            "page-back",
            move || run(app, back.clone()),
        ))
        .child(
            text("")
                .test_id("page-title")
                .role(AccessibilityRole::Header)
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .truncate()
                .font_family(FONT_TEXT)
                .text_size(px(15.0))
                .letter_spacing(px(-0.08))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink))
                .child(
                    text(session.id.clone())
                        .font_family(FONT_MONO)
                        .text_size(px(14.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color(Token::Ink3)),
                )
                .child(" ")
                .child(text(session.title.clone())),
        )
        .child(
            text(session.sub.clone())
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .truncate()
                .font_family(FONT_TEXT)
                .text_size(px(12.0))
                .text_color(color(Token::Ink3)),
        )
        .child(view().flex_1().min_w(px(0.0)))
        .when(has_switch, |row| {
            row.child(
                view()
                    .flex_row()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(5.0))
                    .child(key_cap(session.switch_key))
                    .child(
                        text(session.switch_hint)
                            .font_family(FONT_TEXT)
                            .text_size(px(12.0))
                            .text_color(color(Token::Ink3)),
                    ),
            )
        })
        .children(switch)
        .into_element()
}

/// The switch key's cap (`.kbd`). `atoms::key_hint` sets its key in 10.5 pt ink-3, where a lone
/// backtick is a speck in an empty box; this cap sets one punctuation key large enough to read.
fn key_cap(key: &'static str) -> AnyElement {
    view()
        .flex_row()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .h(px(18.0))
        .min_w(px(18.0))
        .px(px(4.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(color(Token::Rule2))
        .bg(color(Token::BgSunk))
        .accessibility_elements_hidden(true)
        .child(
            text(key)
                .number_of_lines(1u32)
                .font_family(FONT_MONO)
                .text_size(px(15.0))
                .line_height(px(16.0))
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .into_element()
}

/// The transcript and its reply box beside the run. A terminal narrower than its wide grid
/// keeps the transcript alone.
fn transcript_view(app: AppState, session: &Session, draft: Signal<String>) -> AnyElement {
    let main = view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(match app.form {
            Form::Terminal => body(
                app,
                "session-transcript",
                transcript(app, &session.transcript),
            ),
            Form::Desktop | Form::Phone => scroll()
                .test_id("session-transcript")
                .flex_1()
                .min_h(px(0.0))
                .child(transcript(app, &session.transcript))
                .into_element(),
        })
        .child(reply(app, &session.reply, draft))
        .into_element();
    columns(
        app,
        vec![
            (Width::Fill, main),
            (
                Width::Fixed(RUN_WIDTH),
                context(app, "session-run", run_details(app, &session.run_details)),
            ),
        ],
        0,
    )
}

/// The run (`.side-col`): its facts (the task, its rail, agent, runtime, machine, start, usage),
/// then its steps or stages, those still to come quiet.
fn run_details(app: AppState, details: &Run) -> AnyElement {
    let form = app.form;
    let value_size = if form == Form::Phone { 14.0 } else { 12.5 };
    let facts = details
        .facts
        .iter()
        .map(|fact| {
            let value = match &fact.value {
                FactValue::Text(words) => line(app, words, value_size, Token::Ink),
                FactValue::Rail(stages) => rail(app, *stages),
            };
            (fact.label.to_owned(), value)
        })
        .collect::<Vec<_>>();
    let items = details
        .steps
        .iter()
        .map(|(state, words)| {
            let ink = if *state == Mark::Next {
                Token::Ink3
            } else {
                Token::Ink
            };
            let words = text(words.clone()).text_color(color(ink));
            let words = if form == Form::Terminal {
                words
            } else {
                words
                    .font_family(FONT_TEXT)
                    .text_size(px(value_size))
                    .line_height(px(19.0))
            };
            (*state, Hue::Neutral, words.into_element())
        })
        .collect::<Vec<_>>();
    let has_steps = !items.is_empty();
    view()
        .child(h3(app, details.title, None))
        .child(kv(app, facts))
        .when(has_steps, |run| {
            run.when(form == Form::Terminal, |run| run.child(term::blank()))
                .child(h3(app, details.steps_title, None))
                .child(steps(app, items))
        })
        .into_element()
}

/// What the terminal surface shows: its lines, or why there are none.
fn surface_lines(app: AppState, session: &Session, set: console::Set) -> AnyElement {
    let Some(screen) = &session.terminal else {
        return view().into_element();
    };
    match &screen.empty {
        Some(words) if screen.lines.is_empty() => {
            if app.form == Form::Terminal {
                term::quiet(words.clone()).into_element()
            } else {
                console::empty(words.clone(), set)
            }
        }
        _ if app.form == Form::Terminal => console::cell_lines(app, &screen.lines),
        _ => console::lines(&screen.lines, set, screen.cursor),
    }
}

/// The terminal view. Desktop: the toolbar over the dark mono surface, scrolling, the cursor
/// after the last line where the agent takes keys. Terminal: the toolbar's words quiet on one
/// row with the refresh control, a rule, the lines.
fn terminal_view(app: AppState, session: &Session) -> AnyElement {
    let Some(screen) = &session.terminal else {
        return view().into_element();
    };
    let refresh = screen.refresh.clone();
    if app.form == Form::Terminal {
        return view()
            .flex_1()
            .min_h(px(0.0))
            .child(
                view()
                    .flex_row()
                    .h(term::rows(1.0))
                    .px(term::cells(1.0))
                    .child(term::quiet(screen.heading.clone()))
                    .child(term::space(2.0))
                    .child(
                        term::quiet(screen.status.clone())
                            .flex_shrink(1.0)
                            .min_w(px(0.0))
                            .text_color(color(match screen.feed {
                                Feed::Live => Token::Ink3,
                                Feed::Snapshot => Token::Held,
                            })),
                    )
                    .child(view().flex_1().min_w(px(0.0)))
                    .child(
                        term::quiet(screen.input.clone())
                            .flex_shrink(1.0)
                            .min_w(px(0.0)),
                    )
                    .when_some(refresh, |row, action| {
                        row.child(term::space(2.0)).child(button_with(
                            app,
                            "Refresh",
                            ButtonKind::Ghost,
                            ButtonOptions::default(),
                            "btn-refresh-capture",
                            move || run(app, action.clone()),
                        ))
                    }),
            )
            .child(term::hrule())
            .child(body(
                app,
                "session-terminal",
                surface_lines(app, session, BIG),
            ))
            .into_element();
    }
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(console::toolbar(
            screen,
            refresh.map(|action| console::refresh(move || run(app, action.clone()))),
        ))
        .child(
            scroll()
                .test_id("session-terminal")
                .flex_1()
                .min_h(px(0.0))
                .bg(console::ground())
                .child(
                    view()
                        .px(px(18.0))
                        .py(px(14.0))
                        .child(surface_lines(app, session, BIG)),
                ),
        )
        .into_element()
}

/// The phone's session page: the switch over the transcript or the terminal (when there is
/// one), the reply docked as the bar (the terminal's keys, or its refresh, while it shows), the
/// trailing control switching the view.
pub fn phone(app: AppState, session: Memo<Session>, id: Option<Id>) -> PhonePage {
    let draft = draft_for(app, session, id);
    let (title, back, toggle) = untracked(move || {
        session.with(|s| (s.phone_title.clone(), s.back_label, s.toggle.clone()))
    });
    let has_terminal = computed(move || session.with(|s| s.terminal.is_some()));
    let terminal = move || session.with(|s| s.view == View::Terminal && s.terminal.is_some());
    let body = view()
        .child(dynamic(move || {
            if has_terminal.get() {
                untracked(move || {
                    view()
                        .px(px(16.0))
                        .pt(px(4.0))
                        .pb(px(8.0))
                        .child(switch(app, session))
                        .into_element()
                })
            } else {
                view().into_element()
            }
        }))
        .child(dynamic(move || {
            session.with(|s| {
                let surface = match (s.view, &s.terminal) {
                    (View::Terminal, Some(_)) => view()
                        .min_h(px(520.0))
                        .p(px(12.0))
                        .bg(console::ground())
                        .child(surface_lines(app, s, PHONE))
                        .into_element(),
                    _ => transcript(app, &s.transcript),
                };
                match &s.notice {
                    Some((hue, words)) => view()
                        .child(view().px(px(16.0)).pb(px(8.0)).child(notice(
                            app,
                            *hue,
                            &vec![span(words.clone())],
                            None,
                        )))
                        .child(surface)
                        .into_element(),
                    None => surface,
                }
            })
        }))
        .into_element();
    let trailing = dynamic(move || {
        if !has_terminal.get() {
            return view().into_element();
        }
        let toggle = toggle.clone();
        view()
            .w(px(44.0))
            .h(px(44.0))
            .items_center()
            .justify_center()
            .rounded(px(10.0))
            .cursor_pointer()
            .role(AccessibilityRole::Button)
            .label("Switch view")
            .test_id("btn-switch-view")
            .active(|s| s.bg(color(Token::Hover)))
            .focus_visible(|s| {
                s.outline(px(2.0))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(-2.0))
            })
            .on_press(move || run(app, toggle.clone()))
            .child(dynamic(move || {
                icon(
                    if terminal() { Icon::Ask } else { Icon::Term },
                    20.0,
                    color(Token::Ink2),
                )
            }))
            .into_element()
    });
    let docked = dynamic(move || {
        session.with(|s| match (&s.terminal, s.view) {
            (Some(screen), View::Terminal) => {
                let mut controls = screen
                    .phone_keys
                    .iter()
                    .enumerate()
                    .map(|(at, &key)| {
                        button_with(
                            app,
                            key,
                            ButtonKind::Ghost,
                            ButtonOptions {
                                wide: true,
                                ..ButtonOptions::default()
                            },
                            format!("btn-key-{at}"),
                            move || run(app, Action::Key(key)),
                        )
                    })
                    .collect::<Vec<_>>();
                if let Some(action) = screen.refresh.clone() {
                    controls.push(button_with(
                        app,
                        "Refresh capture",
                        ButtonKind::Ghost,
                        ButtonOptions {
                            wide: true,
                            ..ButtonOptions::default()
                        },
                        "btn-refresh-capture",
                        move || run(app, action.clone()),
                    ));
                }
                bar(app, controls, None)
            }
            _ => bar(app, vec![phone_reply(app, &s.reply, draft)], None),
        })
    });
    PhonePage {
        title,
        back,
        body,
        trailing: Some(trailing),
        bar: Some(docked),
        ask: false,
        scroll: true,
    }
}
