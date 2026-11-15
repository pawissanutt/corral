//! The session page: its header and Transcript | Terminal switch, the action bar
//! (`controls`), then the transcript with its reply box beside the run (`.side-col`), or the
//! terminal (`.termbar`, `.bigterm`); on the phone the segmented switch (`.pseg`) and the bar
//! over the view, the docked reply or the terminal's keys. A session without a terminal (a
//! daemon-hosted assistant has no pane) shows no switch and no terminal. A daemon session's
//! terminal is its pane's screen, kept current, with the reply under it (docked on the phone)
//! so the owner answers without leaving it; an assistant's reply is the shared composer on its
//! thread (`reply::region`).

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::agents::{Action, FactValue, Feed, Line, Run, Session, Terminal, View, screen};
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

use super::console::{self, BIG, PHONE, PHONE_SCREEN, SCREEN};
use super::transcript::{line, transcript};
use super::{controls, grid, keys, reply, run};

/// The run column's width (`minmax(0, 1fr) 280px`).
const RUN_WIDTH: f32 = 280.0;

/// What the terminal surface draws below its toolbar; a new screen redraws only this.
#[derive(Clone, PartialEq)]
struct Rows {
    lines: Vec<Line>,
    empty: Option<String>,
    cursor: bool,
    verbatim: bool,
    styled: Vec<screen::Row>,
}

impl Rows {
    fn of(screen: &Terminal) -> Self {
        Self {
            lines: screen.lines.clone(),
            empty: screen.empty.clone(),
            cursor: screen.cursor,
            verbatim: screen.verbatim,
            styled: screen.styled.clone(),
        }
    }
}

/// The toolbar's words and control, apart from the rows: it is redrawn only when the read state
/// changes, so its Refresh keeps the focus while the screen updates.
#[derive(Clone, PartialEq)]
struct Termbar {
    feed: Feed,
    heading: String,
    status: String,
    input: String,
    refresh: Option<Action>,
}

fn rows_of(session: Memo<Session>) -> Memo<Option<Rows>> {
    computed(move || session.with(|s| s.terminal.as_ref().map(Rows::of)))
}

fn termbar_of(session: Memo<Session>) -> Memo<Option<Termbar>> {
    computed(move || {
        session.with(|s| {
            s.terminal.as_ref().map(|screen| Termbar {
                feed: screen.feed,
                heading: screen.heading.clone(),
                status: screen.status.clone(),
                input: screen.input.clone(),
                refresh: screen.refresh.clone(),
            })
        })
    })
}

/// Whether the terminal shows, and the notice over the page: what decides the page's layout.
fn shape_of(session: Memo<Session>) -> Memo<(bool, Option<(Hue, String)>)> {
    computed(move || {
        session.with(|s| {
            (
                s.view == View::Terminal && s.terminal.is_some(),
                s.notice.clone(),
            )
        })
    })
}

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

/// Desktop and terminal: the head, the action bar, then any notice about the session, then the
/// open view. The terminal is built once while it shows and follows its parts, so a screen read
/// every second neither moves the owner's scroll nor takes the reply field's focus; the
/// transcript, the reply and the run each follow their own part of the session.
pub fn page(app: AppState, session: Memo<Session>, id: Option<Id>) -> AnyElement {
    let daemon = id.is_some();
    let draft = draft_for(app, session, id);
    controls::bind_escape(app, session);
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
    let shape = shape_of(session);
    let content = dynamic(move || {
        let (terminal, note) = shape.get();
        let surface = if terminal {
            untracked(|| terminal_view(app, session, draft, daemon))
        } else {
            untracked(|| transcript_view(app, session, draft, daemon))
        };
        match note {
            Some((hue, words)) => view()
                .flex_1()
                .min_h(px(0.0))
                .flex_col()
                .child(
                    view()
                        .flex_none()
                        .when(app.form != Form::Terminal, |n| n.px(px(18.0)).pt(px(10.0)))
                        .child(notice(app, hue, &vec![span(words)], None)),
                )
                .child(surface)
                .into_element(),
            None => surface,
        }
    });
    view()
        .size_full()
        .flex_col()
        .when(app.form != Form::Terminal, |page| page.bg(color(Token::Bg)))
        .child(top)
        .child(controls::bar(app, session))
        .child(view().flex_1().min_h(px(0.0)).flex_col().child(content))
        .child(controls::ask_dialog(app, session))
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

/// The transcript and the words region under it beside the run. A terminal narrower than its
/// wide grid keeps the transcript alone. A daemon session's transcript keeps to its newest
/// rows as they arrive, unless the owner scrolled up.
fn transcript_view(
    app: AppState,
    session: Memo<Session>,
    draft: Signal<String>,
    daemon: bool,
) -> AnyElement {
    let page = computed(move || session.with(|s| s.transcript.clone()));
    let details = computed(move || session.with(|s| s.run_details.clone()));
    let shown = dynamic(move || page.with(|page| transcript(app, page)));
    let surface = scroll()
        .test_id("session-transcript")
        .flex_1()
        .min_h(px(0.0));
    let main = view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(match (app.form, daemon) {
            (Form::Terminal, false) => body(app, "session-transcript", shown),
            (Form::Terminal, true) => console::follow_end(surface, move || page.with(|_| ()))
                .child(view().px(term::cells(1.0)).child(shown))
                .into_element(),
            (Form::Desktop | Form::Phone, false) => surface.child(shown).into_element(),
            (Form::Desktop | Form::Phone, true) => {
                console::follow_end(surface, move || page.with(|_| ()))
                    .child(shown)
                    .into_element()
            }
        })
        .child(reply::region(app, session, draft))
        .into_element();
    columns(
        app,
        vec![
            (Width::Fill, main),
            (
                Width::Fixed(RUN_WIDTH),
                context(
                    app,
                    "session-run",
                    dynamic(move || details.with(|run| run_details(app, run))),
                ),
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

/// What the terminal surface shows inside its scroll, `sides` and `ends` in from its edges (a
/// Cell aside in a real terminal): a pane's screen row for row, the classed lines, or why there
/// are none.
fn rows_view(
    app: AppState,
    rows: Option<&Rows>,
    set: console::Set,
    (sides, ends): (f32, f32),
) -> AnyElement {
    let Some(rows) = rows else {
        return view().into_element();
    };
    let empty = rows.empty.clone().filter(|_| rows.lines.is_empty());
    if app.form == Form::Terminal {
        return match empty {
            Some(words) => view()
                .px(term::cells(1.0))
                .child(term::quiet(words))
                .into_element(),
            None if rows.verbatim && !rows.styled.is_empty() => grid::cell_screen(&rows.styled),
            None if rows.verbatim => console::cell_screen(&rows.lines),
            None => view()
                .px(term::cells(1.0))
                .child(console::cell_lines(app, &rows.lines))
                .into_element(),
        };
    }
    let padded = |content: AnyElement| {
        view()
            .px(px(sides))
            .py(px(ends))
            .child(content)
            .into_element()
    };
    match empty {
        Some(words) => padded(console::empty(words, set)),
        None if rows.verbatim => {
            let screen = if app.form == Form::Phone {
                PHONE_SCREEN
            } else {
                SCREEN
            };
            if rows.styled.is_empty() {
                console::screen_rows(&rows.lines, screen, (sides, ends))
            } else {
                grid::screen(&rows.styled, screen, (sides, ends))
            }
        }
        None => padded(console::lines(&rows.lines, set, rows.cursor)),
    }
}

/// The pressable Refresh for a toolbar, when the terminal can be revalidated.
fn refresh_control(app: AppState, bar: &Termbar) -> Option<AnyElement> {
    bar.refresh
        .clone()
        .map(|action| console::refresh(move || run(app, action.clone())))
}

/// The real terminal's toolbar: its words quiet on one row, the read state held once the rows
/// are not current, and the refresh control.
fn cell_toolbar(app: AppState, bar: &Termbar) -> AnyElement {
    view()
        .flex_row()
        .h(term::rows(1.0))
        .px(term::cells(1.0))
        .child(term::quiet(bar.heading.clone()))
        .child(term::space(2.0))
        .child(
            term::quiet(bar.status.clone())
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .text_color(color(match bar.feed {
                    Feed::Live => Token::Ink3,
                    Feed::Snapshot => Token::Held,
                })),
        )
        .child(view().flex_1().min_w(px(0.0)))
        .child(
            term::quiet(bar.input.clone())
                .flex_shrink(1.0)
                .min_w(px(0.0)),
        )
        .when_some(bar.refresh.clone(), |row, action| {
            row.child(term::space(2.0)).child(button_with(
                app,
                "Refresh",
                ButtonKind::Ghost,
                ButtonOptions::default(),
                "btn-refresh-capture",
                move || run(app, action.clone()),
            ))
        })
        .into_element()
}

/// The terminal surface's scroll. A daemon session's opens at its newest rows and keeps to
/// them as the screen changes, unless the owner scrolled up; the fixture's stays as drawn.
fn surface_scroll(rows: Memo<Option<Rows>>, follow: bool) -> Scroll {
    let surface = scroll().test_id("session-terminal").flex_1().min_h(px(0.0));
    if follow {
        console::follow_end(surface, move || rows.with(|_| ()))
    } else {
        surface
    }
}

/// The terminal view. Desktop: the toolbar over the dark mono surface, scrolling, the cursor
/// after the last line where the agent takes keys, and for a daemon session the keys bar
/// (`keys`) and the reply box under it. Terminal: the toolbar's words quiet on one row with the
/// refresh control, a rule, the lines, then the keys bar and the reply.
fn terminal_view(
    app: AppState,
    session: Memo<Session>,
    draft: Signal<String>,
    daemon: bool,
) -> AnyElement {
    let bar = termbar_of(session);
    let rows = rows_of(session);
    let under = daemon.then(|| keys::under(app, session));
    let answer = daemon.then(|| reply::region(app, session, draft));
    let toolbar = dynamic(move || {
        bar.with(|bar| match bar {
            Some(bar) if app.form == Form::Terminal => cell_toolbar(app, bar),
            Some(bar) => console::toolbar(
                bar.feed,
                &bar.heading,
                &bar.status,
                &bar.input,
                refresh_control(app, bar),
            ),
            None => view().into_element(),
        })
    });
    if app.form == Form::Terminal {
        return view()
            .flex_1()
            .min_h(px(0.0))
            .child(toolbar)
            .child(term::hrule())
            .child(surface_scroll(rows, daemon).child(dynamic(move || {
                rows.with(|rows| rows_view(app, rows.as_ref(), BIG, (0.0, 0.0)))
            })))
            .children(under)
            .children(answer)
            .into_element();
    }
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(toolbar)
        .child(
            surface_scroll(rows, daemon)
                .bg(console::ground())
                .child(dynamic(move || {
                    rows.with(|rows| rows_view(app, rows.as_ref(), BIG, (18.0, 14.0)))
                })),
        )
        .children(under)
        .children(answer)
        .into_element()
}

/// A daemon session's phone body, which scrolls itself: the switch stays over the view, the
/// transcript scrolls in its own region, and the terminal keeps to its newest rows.
fn live_phone_body(app: AppState, session: Memo<Session>, switcher: AnyElement) -> AnyElement {
    let shape = shape_of(session);
    view()
        .flex_1()
        .min_h(px(0.0))
        .child(view().flex_none().child(switcher))
        .child(controls::bar(app, session))
        .child(dynamic(move || {
            let (terminal, note) = shape.get();
            untracked(move || {
                let surface = if terminal {
                    phone_terminal(app, session)
                } else {
                    phone_transcript(app, session)
                };
                view()
                    .flex_1()
                    .min_h(px(0.0))
                    .children(note.map(|(hue, words)| {
                        view().flex_none().px(px(16.0)).pb(px(8.0)).child(notice(
                            app,
                            hue,
                            &vec![span(words)],
                            None,
                        ))
                    }))
                    .child(surface)
                    .into_element()
            })
        }))
        .into_element()
}

/// A daemon session's transcript on the phone, in its own scroll that a changed row does not
/// rebuild, so the place read is kept; it keeps to the newest rows unless the owner scrolled up.
fn phone_transcript(app: AppState, session: Memo<Session>) -> AnyElement {
    let shown = computed(move || session.with(|s| s.transcript.clone()));
    let surface = scroll()
        .test_id("session-transcript")
        .flex_1()
        .min_h(px(0.0));
    console::follow_end(surface, move || shown.with(|_| ()))
        .child(dynamic(move || shown.with(|page| transcript(app, page))))
        .into_element()
}

/// A daemon session's terminal on the phone: the read state and Refresh over the dark surface,
/// the screen at its full width, panning sideways.
fn phone_terminal(app: AppState, session: Memo<Session>) -> AnyElement {
    let bar = termbar_of(session);
    let rows = rows_of(session);
    view()
        .flex_1()
        .min_h(px(0.0))
        .child(dynamic(move || {
            bar.with(|bar| match bar {
                Some(bar) => {
                    console::phone_toolbar(bar.feed, &bar.status, refresh_control(app, bar))
                }
                None => view().into_element(),
            })
        }))
        .child(
            surface_scroll(rows, true)
                .bg(console::ground())
                .child(dynamic(move || {
                    rows.with(|rows| rows_view(app, rows.as_ref(), PHONE, (12.0, 12.0)))
                })),
        )
        .child(keys::under(app, session))
        .into_element()
}

/// The phone's session page: the switch over the transcript or the terminal (when there is
/// one), the trailing control switching the view. The fixture docks the reply under the
/// transcript and the terminal's keys under the terminal; a daemon session docks the keys bar
/// under a terminal corrald types into and the reply otherwise, and its page scrolls itself so
/// the terminal can keep to its newest rows.
pub fn phone(app: AppState, session: Memo<Session>, id: Option<Id>) -> PhonePage {
    let daemon = id.is_some();
    let draft = draft_for(app, session, id);
    let (title, back, toggle) = untracked(move || {
        session.with(|s| (s.phone_title.clone(), s.back_label, s.toggle.clone()))
    });
    let has_terminal = computed(move || session.with(|s| s.terminal.is_some()));
    let terminal = move || session.with(|s| s.view == View::Terminal && s.terminal.is_some());
    let switcher = dynamic(move || {
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
    });
    let body = if daemon {
        view()
            .flex_1()
            .min_h(px(0.0))
            .child(live_phone_body(app, session, switcher))
            .child(controls::ask_dialog(app, session))
            .into_element()
    } else {
        view()
            .child(switcher)
            .child(controls::bar(app, session))
            .child(dynamic(move || {
                session.with(|s| {
                    let surface = match (s.view, &s.terminal) {
                        (View::Terminal, Some(screen)) => view()
                            .min_h(px(520.0))
                            .bg(console::ground())
                            .child(rows_view(app, Some(&Rows::of(screen)), PHONE, (12.0, 12.0)))
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
            .into_element()
    };
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
    // The reply is redrawn only when it changes, not with each new screen, so a field being
    // typed in keeps its focus.
    let docked = if daemon {
        keys::docked(app, session, move || reply::region(app, session, draft))
    } else {
        dynamic(move || {
            session.with(|s| match (&s.terminal, s.view) {
                (Some(screen), View::Terminal) => bar(
                    app,
                    screen
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
                        .collect(),
                    None,
                ),
                _ => bar(app, vec![reply::phone_reply(app, &s.reply, draft)], None),
            })
        })
    };
    PhonePage {
        title,
        back,
        body,
        trailing: Some(trailing),
        bar: Some(docked),
        ask: false,
        scroll: !daemon,
    }
}
