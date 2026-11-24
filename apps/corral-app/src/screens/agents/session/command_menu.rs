//! What opens over the composer: the model or effort picker (`ui::ask::models`), and the `/`
//! menu while the draft is a `/` command (`model::session::SlashMenu`): each row its command
//! first, as the terminal UI's completion shows it, then what it does, where it comes from (an
//! extension, a prompt template, a skill) and, muted, why it cannot run now. ↑ ↓ move the
//! highlight, Enter (in the composer) runs the highlighted row and pressing a row does the same;
//! an unavailable row runs nothing. Esc closes the menu and keeps the text. A `/text` that matches
//! nothing is sent to the host as typed. Test ids `session-commands`, `cmd-<slug>`.

use nsh_core::keymap::When;
use nsh_core::prelude::*;

use crate::model::hue::Hue;
use crate::model::session::{Action, CommandAction, SessionPage, SlashMenu, SlashRow};
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::ask::models::session_picker;
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::super::run;

/// The `/` menu of the page as the model builds it (tracked).
pub(super) fn matches(page: Memo<SessionPage>) -> Memo<Option<SlashMenu>> {
    computed(move || page.with(|p| p.slash.clone()))
}

/// What Enter in the composer runs instead of sending while the menu shows rows: the
/// highlighted row's actions, or nothing when that row cannot run now.
pub(super) fn chosen(menu: Memo<Option<SlashMenu>>) -> Memo<Option<Vec<Action>>> {
    computed(move || {
        menu.with(|menu| {
            let menu = menu.as_ref()?;
            let row = menu.rows.get(menu.at?)?;
            Some(if row.unavailable.is_none() {
                row.run.clone()
            } else {
                Vec::new()
            })
        })
    })
}

/// The picker, then the `/` menu, over the composer.
pub(super) fn menu(
    app: AppState,
    page: Memo<SessionPage>,
    menu: Memo<Option<SlashMenu>>,
    ask: bool,
) -> AnyElement {
    let picker = computed(move || page.with(|p| p.picker.clone()));
    let list = dynamic(move || {
        let mut menu = menu.get()?;
        let session = session_of(&menu)?;
        if let Some(why) = app.session_input_block(&session, ask) {
            for row in &mut menu.rows {
                row.unavailable = Some(why.into());
            }
        }
        Some(untracked(|| slash_list(app, &menu, session, ask)))
    });
    view()
        .flex_none()
        .child(session_picker(app, picker))
        .child(list)
        .into_element()
}

fn sized(form: Form, words: Text, mono: bool) -> Text {
    match form {
        Form::Terminal => words,
        _ => words
            .font_family(if mono { FONT_MONO } else { FONT_TEXT })
            .text_size(px(if form == Form::Phone { 14.0 } else { 12.5 })),
    }
}

/// The open menu: its rows, the count beyond them, the read's note. ↑ ↓ move the highlight
/// while it shows.
fn slash_list(app: AppState, menu: &SlashMenu, session: Id, ask: bool) -> AnyElement {
    let form = app.form;
    if let Some(at) = menu.at {
        let len = menu.rows.len();
        for (key, step) in [("up", -1_isize), ("down", 1)] {
            let session = session.clone();
            on_key_with(
                key,
                KeyOptions {
                    when: When::Always,
                    ..KeyOptions::default()
                },
                move |_| {
                    let next = at.saturating_add_signed(step).min(len.saturating_sub(1));
                    run(
                        app,
                        Action::Command(CommandAction::MenuAt {
                            session: session.clone(),
                            at: next,
                        }),
                    );
                },
            );
        }
    }
    let items: Vec<AnyElement> = menu
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| item(app, row, menu.at == Some(index), session.clone(), ask))
        .collect();
    let pad = |words: Text| {
        let line = view().child(words);
        if form == Form::Terminal {
            line.into_element()
        } else {
            line.px(px(8.0)).py(px(3.0)).into_element()
        }
    };
    let more = (menu.more > 0).then(|| {
        pad(sized(
            form,
            text(format!("{} more · keep typing to narrow", menu.more))
                .text_color(color(Token::Ink3)),
            false,
        ))
    });
    let note = menu.note.clone().map(|(hue, words)| {
        pad(sized(
            form,
            text(words).text_color(if hue == Hue::Neutral {
                color(Token::Ink3)
            } else {
                hue_ink(hue)
            }),
            false,
        ))
    });
    let list = view()
        .test_id("session-commands")
        .role(AccessibilityRole::List)
        .label("Commands")
        .children(items)
        .children(more)
        .children(note);
    match form {
        Form::Terminal => view()
            .flex_none()
            .child(term::hrule())
            .child(list.px(term::cells(1.0)))
            .into_element(),
        Form::Desktop | Form::Phone => view()
            .flex_none()
            .px(px(14.0))
            .pt(px(6.0))
            .child(
                list.py(px(4.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(color(Token::Rule2))
                    .bg(color(Token::Surface)),
            )
            .into_element(),
    }
}

/// The session the menu's rows act on, read from its Esc action.
fn session_of(menu: &SlashMenu) -> Option<Id> {
    match &menu.close {
        Action::Command(CommandAction::CloseMenu(session)) => Some(session.clone()),
        _ => None,
    }
}

/// One row: `⏎` before the highlighted one, its command and argument, what it does, where it
/// comes from, and why it cannot run (muted) where it cannot.
fn item(app: AppState, row: &SlashRow, highlighted: bool, session: Id, ask: bool) -> AnyElement {
    let form = app.form;
    let enabled = row.unavailable.is_none();
    let lead = sized(
        form,
        text(if highlighted { "⏎" } else { " " }).text_color(color(Token::Ink3)),
        true,
    )
    .flex_shrink_0()
    .w(if form == Form::Terminal {
        term::cells(2.0)
    } else {
        px(16.0)
    });
    let command = sized(
        form,
        text(row.command.clone())
            .number_of_lines(1u32)
            .text_color(color(if enabled { Token::Ink } else { Token::Ink3 }))
            .when(form == Form::Terminal && enabled, |t| {
                t.font_weight(FontWeight::BOLD)
            }),
        true,
    )
    .flex_shrink_0();
    let args = row
        .args
        .clone()
        .map(|args| sized(form, text(args).text_color(color(Token::Ink3)), true).flex_shrink_0());
    let label = sized(
        form,
        text(row.label.clone())
            .number_of_lines(1u32)
            .truncate()
            .text_color(color(if enabled { Token::Ink2 } else { Token::Ink3 })),
        false,
    )
    .flex_shrink(1.0)
    .min_w(px(0.0));
    let why = row.unavailable.clone().map(|why| {
        sized(
            form,
            text(format!("unavailable: {why}"))
                .number_of_lines(1u32)
                .truncate()
                .text_color(color(Token::Ink3)),
            false,
        )
        .flex_shrink(2.0)
        .min_w(px(0.0))
    });
    let source = row.source.map(|source| {
        sized(form, text(source).text_color(color(Token::Ink3)), false).flex_shrink_0()
    });
    let actions = row.run.clone();
    let mut accessible = format!("{} {}", row.command, row.label);
    if let Some(why) = &row.unavailable {
        accessible.push_str(&format!(", unavailable: {why}"));
    }
    view()
        .test_id(&row.id)
        .role(AccessibilityRole::Button)
        .label(accessible)
        .accessibility_state(AccessibilityState {
            disabled: !enabled,
            selected: highlighted,
            ..AccessibilityState::default()
        })
        .flex_row()
        .items_center()
        .gap(if form == Form::Terminal {
            term::cells(1.0)
        } else {
            px(8.0)
        })
        .when(form == Form::Terminal && highlighted, |line| {
            line.bg(color(Token::Surface2))
        })
        .when(form != Form::Terminal, |line| {
            line.px(px(8.0))
                .py(px(if form == Form::Phone { 9.0 } else { 4.0 }))
                .when(form == Form::Phone, |line| line.min_h(px(44.0)))
                .rounded(px(6.0))
                .when(highlighted, |line| line.bg(color(Token::Sel)))
                .hover(|s| s.bg(color(Token::Hover)))
        })
        .when(enabled, |line| {
            line.cursor_pointer().on_press(move || {
                app.session_slash(&session, actions.clone(), ask);
            })
        })
        .child(lead)
        .child(command)
        .children(args)
        .child(label)
        .children(why)
        .child(view().flex_1().min_w(px(0.0)))
        .children(source)
        .into_element()
}
