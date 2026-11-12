//! The session page: its header and Transcript | Terminal switch, the transcript with its reply
//! box beside the run (`.side-col`), or the terminal (`.termbar`, `.bigterm`); on the phone the
//! segmented switch (`.pseg`), the docked reply or the terminal's keys.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::agents::{Action, FactValue, Feed, Run, Session, View};
use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::screens::PhonePage;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with, seg};
use crate::ui::kit::marks::rail;
use crate::ui::kit::page::{Width, bar, body, columns, context, pushed_head};
use crate::ui::kit::table::{kv, steps};
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::console::{self, BIG, PHONE};
use super::transcript::{line, phone_reply, reply, transcript};
use super::{run, session_view};

/// The run column's width (`minmax(0, 1fr) 280px`).
const RUN_WIDTH: f32 = 280.0;

/// The Transcript | Terminal switch, reading the open view.
fn switch(app: AppState, session: Memo<Session>) -> AnyElement {
    let (labels, actions) =
        untracked(move || session.with(|s| (s.switch_labels, s.switch_actions)));
    seg(
        app,
        &[(View::Transcript, labels[0]), (View::Terminal, labels[1])],
        move || session.with(|s| s.view),
        move |view| {
            let action = if view == View::Transcript {
                actions[0]
            } else {
                actions[1]
            };
            run(app, action);
        },
        "seg-session",
    )
}

/// Desktop and terminal: the head, then the open view.
pub fn page(app: AppState) -> AnyElement {
    let session = computed(move || session_view(app));
    let draft = signal(String::new());
    // The head follows the open session but not its view: the switch keeps its focus.
    let id = computed(move || session.with(|s| s.id));
    let top = dynamic(move || {
        let _ = id.get();
        untracked(move || {
            let control = switch(app, session);
            session.with(|s| head(app, s, control))
        })
    });
    let content = dynamic(move || {
        session.with(|s| match s.view {
            View::Transcript => transcript_view(app, s, draft),
            View::Terminal => terminal_view(app, s),
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
/// bold, the run and agent, then the `` ` `` key and the switch at the trailing edge. Terminal:
/// the kit's pushed head with the key's legend and the switch.
fn head(app: AppState, session: &Session, switch: AnyElement) -> AnyElement {
    let back = session.back;
    if app.form == Form::Terminal {
        return pushed_head(
            app,
            format!("{} {}", session.id, session.title),
            session.desktop_back_label,
            move || run(app, back),
            Some(
                view()
                    .flex_row()
                    .items_center()
                    .child(term::legend(session.switch_key, session.switch_hint))
                    .child(term::space(2.0))
                    .child(switch)
                    .into_element(),
            ),
        );
    }
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
            move || run(app, back),
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
                    text(session.id)
                        .font_family(FONT_MONO)
                        .text_size(px(14.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(color(Token::Ink3)),
                )
                .child(" ")
                .child(text(session.title)),
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
        .child(
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
        .child(switch)
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

/// The run (`.side-col`): its facts (the task, its rail, agent, runtime, machine, start), then
/// its steps, those still to come quiet.
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
        .map(|&(state, words)| {
            let ink = if state == Mark::Next {
                Token::Ink3
            } else {
                Token::Ink
            };
            let words = text(words).text_color(color(ink));
            let words = if form == Form::Terminal {
                words
            } else {
                words
                    .font_family(FONT_TEXT)
                    .text_size(px(value_size))
                    .line_height(px(19.0))
            };
            (state, Hue::Neutral, words.into_element())
        })
        .collect::<Vec<_>>();
    view()
        .child(h3(app, details.title, None))
        .child(kv(app, facts))
        .when(form == Form::Terminal, |run| run.child(term::blank()))
        .child(h3(app, details.steps_title, None))
        .child(steps(app, items))
        .into_element()
}

/// The terminal view. Desktop: the toolbar over the dark mono surface, scrolling, the cursor
/// after the last line. Terminal: the toolbar's words quiet on one row, a rule, the lines.
fn terminal_view(app: AppState, session: &Session) -> AnyElement {
    let screen = &session.terminal;
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
                        term::quiet(screen.status)
                            .flex_shrink(1.0)
                            .min_w(px(0.0))
                            .text_color(color(match screen.feed {
                                Feed::Live => Token::Ink3,
                                Feed::Snapshot => Token::Held,
                            })),
                    )
                    .child(view().flex_1().min_w(px(0.0)))
                    .child(term::quiet(screen.input).flex_shrink(1.0).min_w(px(0.0))),
            )
            .child(term::hrule())
            .child(body(
                app,
                "session-terminal",
                console::cell_lines(app, &screen.lines),
            ))
            .into_element();
    }
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(console::toolbar(screen))
        .child(
            scroll()
                .test_id("session-terminal")
                .flex_1()
                .min_h(px(0.0))
                .bg(console::ground())
                .child(view().px(px(18.0)).py(px(14.0)).child(console::lines(
                    &screen.lines,
                    BIG,
                    screen.cursor,
                ))),
        )
        .into_element()
}

/// The phone's session page: the switch over the transcript or the terminal, the reply docked
/// as the bar (the terminal's keys while it shows), the trailing control switching the view.
pub fn phone(app: AppState) -> PhonePage {
    let session = computed(move || session_view(app));
    let draft = signal(String::new());
    let (title, back, toggle) =
        untracked(move || session.with(|s| (s.phone_title.clone(), s.back_label, s.toggle)));
    let terminal = move || session.with(|s| s.view == View::Terminal);
    let body = view()
        .child(
            view()
                .px(px(16.0))
                .pt(px(4.0))
                .pb(px(8.0))
                .child(switch(app, session)),
        )
        .child(dynamic(move || {
            session.with(|s| match s.view {
                View::Transcript => transcript(app, &s.transcript),
                View::Terminal => view()
                    .min_h(px(520.0))
                    .p(px(12.0))
                    .bg(console::ground())
                    .child(console::lines(&s.terminal.lines, PHONE, s.terminal.cursor))
                    .into_element(),
            })
        }))
        .into_element();
    let trailing = view()
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
        .on_press(move || run(app, toggle))
        .child(dynamic(move || {
            icon(
                if terminal() { Icon::Ask } else { Icon::Term },
                20.0,
                color(Token::Ink2),
            )
        }))
        .into_element();
    let docked = dynamic(move || {
        session.with(|s| {
            if s.view == View::Terminal {
                let keys = s
                    .terminal
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
                bar(app, keys, None)
            } else {
                bar(app, vec![phone_reply(app, &s.reply, draft)], None)
            }
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
