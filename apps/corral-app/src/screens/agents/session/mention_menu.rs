//! The `@` file mention popup over the composer (`model::session::MentionMenu`), drawn as the `/`
//! menu is: the folder listed, then its matched entries, each folder with its `/`, the
//! highlighted one led by `⏎`, the count beyond them and the read's note. While the composer's
//! field has the keys, ↑ ↓ move the highlight, Enter (in the composer) and Tab choose it (a
//! folder descends, a file goes into the draft) and Esc closes the popup and keeps the text;
//! pressing a row chooses it. Test ids `session-mentions`, `mention-<n>`.

use nsh_core::keymap::When;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{Action, MentionMenu, MentionRow, SessionComposer};
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

fn sized(form: Form, words: Text, mono: bool) -> Text {
    match form {
        Form::Terminal => words,
        _ => words
            .font_family(if mono { FONT_MONO } else { FONT_TEXT })
            .text_size(px(if form == Form::Phone { 14.0 } else { 12.5 })),
    }
}

/// The popup of `composer` while its draft ends in an `@` mention; `typing` says its `field`
/// has the keyboard focus, so its keys are the popup's; `ask` is Ask's composer.
pub(crate) fn mention_menu(
    app: AppState,
    composer: Memo<Option<SessionComposer>>,
    field: ElementRef,
    typing: Memo<bool>,
    ask: bool,
) -> AnyElement {
    let menu = computed(move || composer.with(|c| c.as_ref().and_then(|c| c.mention.clone())));
    // Not while a side pane it is drawn in is set aside, nor while the field has given the
    // keyboard to another control: the page keeps its keys then.
    effect(move || {
        if !app.binds_keys() || !typing.get() {
            return;
        }
        let Some(shown) = menu.get() else {
            return;
        };
        untracked(|| bind(app, shown, ask));
    });
    dynamic(move || {
        let menu = menu.get()?;
        Some(untracked(|| list(app, &menu, field, ask)))
    })
    .into_element()
}

fn bind(app: AppState, menu: MentionMenu, ask: bool) {
    let always = KeyOptions {
        when: When::Always,
        ..KeyOptions::default()
    };
    for (key, step) in [("up", -1_isize), ("down", 1)] {
        if let Some(action) = menu.step(step) {
            on_key_with(key, always, move |_| {
                app.session_input(action.clone(), ask);
            });
        }
    }
    if let Some(chosen) = menu.chosen().cloned() {
        on_key_with("tab", always, move |_| {
            app.session_input(chosen.clone(), ask);
        });
    }
    let close = menu.close.clone();
    on_key_with("escape", always, move |_| {
        app.session_input(close.clone(), ask);
    });
}

fn list(app: AppState, menu: &MentionMenu, field: ElementRef, ask: bool) -> AnyElement {
    let form = app.form;
    let pad = |words: Text| {
        let line = view().child(words);
        if form == Form::Terminal {
            line.into_element()
        } else {
            line.px(px(8.0)).py(px(3.0)).into_element()
        }
    };
    let muted = |words: String| sized(form, text(words).text_color(color(Token::Ink3)), false);
    let head = pad(muted(if menu.folder.is_empty() {
        "Mention a file · the worktree's root".to_owned()
    } else {
        format!("Mention a file · {}", menu.folder)
    }));
    let above = (menu.first > 0).then(|| pad(muted(format!("{} above · ↑ for more", menu.first))));
    let items: Vec<AnyElement> = menu
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| item(app, row, menu.at == Some(index), field, ask))
        .collect();
    let more = (menu.more > 0).then(|| {
        pad(muted(format!(
            "{} more · ↓ or keep typing to narrow",
            menu.more
        )))
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
        .test_id("session-mentions")
        .role(AccessibilityRole::List)
        .label("File mentions")
        .child(head)
        .children(above)
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

/// One entry: `⏎` before the highlighted one, then its name (a folder with its `/`). Pressing
/// it chooses it and gives the keyboard back to the field.
fn item(
    app: AppState,
    row: &MentionRow,
    highlighted: bool,
    field: ElementRef,
    ask: bool,
) -> AnyElement {
    let form = app.form;
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
    let name = sized(
        form,
        text(row.name.clone())
            .number_of_lines(1u32)
            .truncate()
            .text_color(color(if row.directory {
                Token::Ink2
            } else {
                Token::Ink
            }))
            .when(form == Form::Terminal && highlighted, |t| {
                t.font_weight(FontWeight::BOLD)
            }),
        true,
    )
    .flex_shrink(1.0)
    .min_w(px(0.0));
    let pick: Action = row.pick.clone();
    view()
        .test_id(&row.id)
        .role(AccessibilityRole::Button)
        .label(row.name.clone())
        .accessibility_state(AccessibilityState {
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
        .cursor_pointer()
        .on_press(move || {
            app.session_input(pick.clone(), ask);
            field.focus();
        })
        .child(lead)
        .child(name)
        .into_element()
}
