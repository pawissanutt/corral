//! The session page: its head with the run's Session menu (and, where corrald serves
//! transcripts, the Transcript | Terminal switch), the strip with the run's state (`controls`),
//! then the terminal (`.termbar`, `.bigterm`) with the pane's keys bar and type field under it
//! (`keys`), or the transcript with its reply box beside the run (`.side-col`); on the phone the
//! segmented switch (`.pseg`) and the strip over the view, the keys or the reply docked. A run
//! without a recorded pane or process shows no terminal. A run's terminal is its pane's bounded
//! scrollback and screen (or its process log's tail), streamed while it shows and scrolling
//! inside its surface. A session running child agents lists them and their messages under the
//! run (`transcript::fleet`), and on the phone in a fold under the strip.

use corral_protocol::app::SESSION_V1;
use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::agents::{Action, FactValue, Run, Session, View};
use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::model::text::span;
use crate::model::{Form, Id};
use crate::screens::{PhoneBar, PhonePage, assistant};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with, seg};
use crate::ui::kit::marks::rail;
use crate::ui::kit::notice::notice;
use crate::ui::kit::page::{Width, columns, context, pushed_head};
use crate::ui::kit::table::{kv, steps};
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::console;
use super::transcript::{fleet, line, transcript};
use super::{Dispatch, controls, keys, reply, run, surface};

/// The run column's width (`minmax(0, 1fr) 280px`).
const RUN_WIDTH: f32 = 280.0;

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

/// One draft per open session page, kept while the view switches.
fn draft_for(app: AppState, session: Memo<Session>, id: Id) -> Signal<String> {
    let delivered = computed(move || session.with(|s| s.reply.delivered.clone()));
    draft(app, delivered, id, run)
}

/// A reply draft for session `id`. A delivered reply (`delivered` names the latest) clears it
/// once and refetches the transcript, which now holds the reply; a refused or unknown one keeps
/// the words so nothing typed is lost.
pub(super) fn draft(
    app: AppState,
    delivered: Memo<Option<String>>,
    id: Id,
    send: Dispatch,
) -> Signal<String> {
    let draft = signal(String::new());
    let seen = stored(None::<Option<String>>);
    effect(move || {
        let now = delivered.get();
        if let Some(before) = seen.replace(Some(now.clone()))
            && before != now
            && now.is_some()
        {
            draft.set(String::new());
            // Only a corrald that serves transcripts has one to read the reply in.
            if untracked(|| app.capable(SESSION_V1)) {
                let target = DetailRef::Session { id: id.to_string() };
                untracked(|| send(app, Action::Refresh(target)));
            }
        }
    });
    draft
}

/// While the session's terminal shows, holds corrald's stream of its pane open: each
/// replacement screen arrives on its own, so nothing here reads the capture on a clock. It is
/// released when the view switches back to the transcript, the session loses its pane, or the
/// page closes, always through the source the page mounted on.
fn stream_screen(app: AppState, session: Memo<Session>, id: Id) {
    let scoped = app.scope();
    let shown =
        computed(move || session.with(|s| s.view == View::Terminal && s.terminal.is_some()));
    let open = stored(false);
    let pane = id.to_string();
    effect(move || {
        let now = shown.get();
        if open.replace(now) != now {
            untracked(|| scoped.terminal(pane.clone(), now));
        }
    });
    // The owner may already be letting go of its values when this runs.
    cleanup(move || {
        if open.try_with(std::mem::take).unwrap_or(false) {
            scoped.terminal(id.to_string(), false);
        }
    });
}

/// Desktop and terminal: the head, the action bar, then any notice about the session, then the
/// open view. The terminal is built once while it shows and follows its parts, so a screen
/// arriving neither moves the owner's scroll nor takes the type field's focus; the transcript,
/// the reply and the run each follow their own part of the session.
pub fn page(app: AppState, session: Memo<Session>, id: Id) -> AnyElement {
    stream_screen(app, session, id.clone());
    let draft = draft_for(app, session, id);
    // The head follows the open session, its switch and its menu, not its view: the switch and
    // the menu keep their focus.
    let key = computed(move || session.with(|s| (s.id.clone(), s.switch, s.controls.menu.clone())));
    let top = dynamic(move || {
        let (_, switch_shown, menu) = key.get();
        untracked(move || {
            let control = switch_shown.then(|| switch(app, session));
            let menu = menu.map(|id| controls::session_menu(app, id));
            session.with(|s| head(app, s, control, menu))
        })
    });
    let shape = shape_of(session);
    let content = dynamic(move || {
        let (terminal, note) = shape.get();
        let surface = if terminal {
            untracked(|| surface::terminal_view(app, session))
        } else {
            untracked(|| transcript_view(app, session, draft))
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
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex_col()
        .when(app.form != Form::Terminal, |page| page.bg(color(Token::Bg)))
        .child(top)
        .child(controls::bar(app, session))
        .child(view().flex_1().min_h(px(0.0)).flex_col().child(content))
        .into_element()
}

/// The page head. Desktop (`.pagehead`): back to Agents, the id in quiet mono and the title in
/// bold, the run and agent, then the `` ` `` key and the switch when the session has both views,
/// and the Session menu at the trailing edge. Terminal: the kit's pushed head with the same at
/// its trailing edge.
fn head(
    app: AppState,
    session: &Session,
    switch: Option<AnyElement>,
    menu: Option<AnyElement>,
) -> AnyElement {
    let back = session.back.clone();
    if app.form == Form::Terminal {
        let has_switch = switch.is_some();
        let trailing = view()
            .flex_row()
            .items_center()
            .when(has_switch, |row| {
                row.child(term::legend(session.switch_key, session.switch_hint))
                    .child(term::space(2.0))
            })
            .children(switch)
            .when(has_switch, |row| row.child(term::space(2.0)))
            .children(menu)
            .into_element();
        return pushed_head(
            app,
            format!("{} {}", session.id, session.title),
            session.desktop_back_label,
            move || run(app, back.clone()),
            Some(trailing),
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
        .children(menu)
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
/// wide grid keeps the transcript alone. The transcript keeps to its newest rows as they arrive,
/// unless the owner scrolled up.
fn transcript_view(app: AppState, session: Memo<Session>, draft: Signal<String>) -> AnyElement {
    let page = computed(move || session.with(|s| s.transcript.clone()));
    let details = computed(move || session.with(|s| s.run_details.clone()));
    let children = computed(move || session.with(|s| s.fleet.clone()));
    let shown = dynamic(move || page.with(|page| transcript(app, page)));
    let surface = console::follow_end(
        scroll()
            .test_id("session-transcript")
            .flex_1()
            .min_h(px(0.0)),
        move || page.with(|_| ()),
    );
    let surface = if app.form == Form::Terminal {
        surface.child(view().px(term::cells(1.0)).child(shown))
    } else {
        surface.child(shown)
    };
    let main = view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(surface)
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
                    view()
                        .child(dynamic(move || details.with(|run| run_details(app, run))))
                        .child(dynamic(move || {
                            children.with(|children| match children {
                                Some(children) => fleet::column(app, children),
                                None => view().into_element(),
                            })
                        }))
                        .into_element(),
                ),
            ),
        ],
        0,
    )
}

/// The run (`.side-col`): its facts (the task, its rail, agent, runtime, machine, start, usage),
/// then its steps or stages, those still to come quiet. A terminal's run column is under 30
/// Cells, too narrow for a label column beside a value: there each label stands over its value,
/// and an id leading a value (task, profile, runtime, machine) stays one line cut to fit.
fn run_details(app: AppState, details: &Run) -> AnyElement {
    let form = app.form;
    let value_size = if form == Form::Phone { 14.0 } else { 12.5 };
    let value = |fact_value: &FactValue| match fact_value {
        FactValue::Text(words) if form == Form::Terminal => assistant::identified(app, words),
        FactValue::Text(words) => line(app, words, value_size, Token::Ink),
        FactValue::Rail(stages) => rail(app, *stages),
    };
    let facts = if form == Form::Terminal {
        view()
            .children(
                details
                    .facts
                    .iter()
                    .map(|fact| assistant::stacked(app, fact.label, value(&fact.value)))
                    .collect::<Vec<_>>(),
            )
            .into_element()
    } else {
        kv(
            app,
            details
                .facts
                .iter()
                .map(|fact| (fact.label.to_owned(), value(&fact.value)))
                .collect(),
        )
    };
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
        .child(facts)
        .when(has_steps, |run| {
            run.when(form == Form::Terminal, |run| run.child(term::blank()))
                .child(h3(app, details.steps_title, None))
                .child(steps(app, items))
        })
        .into_element()
}

/// The phone body, which scrolls itself: the switch stays over the view, the transcript scrolls
/// in its own region, and the terminal keeps to its newest rows.
fn live_phone_body(app: AppState, session: Memo<Session>, switcher: AnyElement) -> AnyElement {
    let shape = shape_of(session);
    view()
        .flex_1()
        .min_h(px(0.0))
        .child(view().flex_none().child(switcher))
        .child(controls::bar(app, session))
        .child(fleet::phone(app, session))
        .child(dynamic(move || {
            let (terminal, note) = shape.get();
            untracked(move || {
                let surface = if terminal {
                    surface::phone_terminal(app, session)
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

/// The transcript on the phone, in its own scroll that a changed row does not rebuild, so the
/// place read is kept; it keeps to the newest rows unless the owner scrolled up.
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

/// The phone's session page: the switch over the transcript or the terminal (where corrald
/// serves both), the trailing control switching the view. The terminal docks the pane's keys
/// in the bar, the transcript its reply, and the page scrolls itself so the terminal can keep
/// to its newest rows.
pub fn phone(app: AppState, session: Memo<Session>, id: Id) -> PhonePage {
    stream_screen(app, session, id.clone());
    let draft = draft_for(app, session, id);
    let (title, toggle) =
        untracked(move || session.with(|s| (s.phone_title.clone(), s.toggle.clone())));
    let has_terminal = computed(move || session.with(|s| s.switch));
    let terminal = move || session.with(|s| s.view == View::Terminal && s.terminal.is_some());
    let docks_keys = computed(terminal);
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
    let body = view()
        .flex_1()
        .min_h(px(0.0))
        .child(live_phone_body(app, session, switcher))
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
    // The docked words are redrawn only when the view switches, not with each new screen, so a
    // field being typed in keeps its focus.
    let docked = dynamic(move || {
        if docks_keys.get() {
            untracked(|| keys::region(app, session))
        } else {
            untracked(|| reply::region(app, session, draft))
        }
    });
    PhonePage {
        title,
        body,
        trailing: Some(trailing),
        bar: Some(PhoneBar::always(docked)),
        ask: false,
        scroll: false,
    }
}
