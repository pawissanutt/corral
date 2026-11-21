//! The configuration form's fields: each a label beside its control (over it on a phone), with
//! the field's note, what it inherits while unset, and the reasons a refused save gave for it
//! under the control. Every value is read live from the draft, so typing redraws the words and
//! never the field being typed in. IDs: `field-config-<key>` for a field (`[<n>]` for a line,
//! `.<name>` for a variable), `seg-config-<key>-<on|off|inherit>` for a switch,
//! `btn-config-<key>-<action>` for a list's controls.

use nsh_core::element::{color as for_appearance, input};
use nsh_core::prelude::*;

use super::parts::{button_row, cell, mono_cell};
use crate::model::Form;
use crate::model::settings::document::{ListEdit, Path, Seg};
use crate::model::settings::editor::{Edit, Typed};
use crate::model::settings::form::{self, Field, Kind, Live};
use crate::model::settings::inherit;
use crate::state::AppState;
use crate::ui::composer::copyable_input;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::suggest::suggestions;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// Applies one edit to the draft.
pub fn edit(app: AppState, edit: Edit) {
    app.act(app.settings, move |state, _| state.edit_config(edit));
}

/// A label beside its control: a 150 pt column on the desktop, 18 Cells in a terminal; the
/// label over the control on a phone.
pub fn row(app: AppState, label: impl Into<SharedString>, control: AnyElement) -> AnyElement {
    let label = cell(app, label.into(), Token::Ink2, true);
    match app.form {
        Form::Phone => view()
            .min_w(px(0.0))
            .gap(px(6.0))
            .child(label)
            .child(control)
            .into_element(),
        Form::Desktop => view()
            .flex_row()
            .items_start()
            .gap(px(12.0))
            .min_w(px(0.0))
            .child(view().w(px(150.0)).flex_shrink_0().pt(px(7.0)).child(label))
            .child(view().flex_1().min_w(px(0.0)).child(control))
            .into_element(),
        Form::Terminal => view()
            .flex_row()
            .items_start()
            .min_w(px(0.0))
            .child(view().w(term::cells(18.0)).flex_shrink_0().child(label))
            .child(view().flex_1().min_w(px(0.0)).child(control))
            .into_element(),
    }
}

/// A one-line text entry in the form's look. The field the last edit added takes the focus as
/// it shows, so Add leaves the keyboard on what it made.
pub fn entry(
    app: AppState,
    id: String,
    label: impl Into<SharedString>,
    value: impl Fn() -> String + 'static,
    on_change: impl Fn(String) + 'static,
) -> AnyElement {
    entry_with(app, id, label, value, on_change, None, CopyMode::Ordinary)
}

#[derive(Clone, Copy)]
enum CopyMode {
    Ordinary,
    /// Environment values can hold credentials; leave copying entirely to the platform.
    NativeOnly,
}

/// [`entry`] with an optional focus handle and copy policy.
fn entry_with(
    app: AppState,
    id: String,
    label: impl Into<SharedString>,
    value: impl Fn() -> String + 'static,
    on_change: impl Fn(String) + 'static,
    handle: Option<ElementRef>,
    copy: CopyMode,
) -> AnyElement {
    let form = app.form;
    let label: SharedString = label.into();
    let added = app.settings.with_untracked(|state| {
        state
            .config
            .added
            .as_ref()
            .is_some_and(|added| self::id(added) == id)
    });
    let handle = handle.unwrap_or_else(element_ref);
    let jump_id = id.clone();
    on_jump(app, move |path| self::id(path) == jump_id, || {}, handle);
    let field = match copy {
        CopyMode::Ordinary => copyable_input(app, Some(handle), value),
        CopyMode::NativeOnly => input().element_ref(&handle).value(value),
    };
    let field = field
        .test_id(&id)
        .label(label.clone())
        .placeholder(label.to_string())
        .auto_focus(added)
        .on_change(move |text: &str| on_change(text.to_owned()))
        .auto_correct(false)
        .auto_capitalize(AutoCapitalize::None)
        .min_w(px(0.0))
        .flex_1()
        .text_color(color(Token::Ink))
        .placeholder_color(move || for_appearance(color(Token::Ink4)))
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .bg(AdaptiveColor::TRANSPARENT)
        .border_1()
        .border_color(color(Token::Rule2))
        .focus(|s| s.border_color(color(Token::Focus)));
    match form {
        Form::Terminal => field.h(term::rows(1.0)).px(term::cells(1.0)).into_element(),
        Form::Desktop => field
            .font_family(FONT_MONO)
            .text_size(px(12.5))
            .h(px(30.0))
            .px(px(9.0))
            .rounded(px(6.0))
            .into_element(),
        Form::Phone => field
            .font_family(FONT_TEXT)
            .text_size(px(16.0))
            .min_h(px(44.0))
            .px(px(12.0))
            .rounded(px(10.0))
            .into_element(),
    }
}

/// Reveals and focuses the control that owns an explicit jump, even when it is already mounted.
pub(super) fn on_jump(
    app: AppState,
    matches: impl Fn(&Path) -> bool + 'static,
    reveal: impl Fn() + 'static,
    handle: ElementRef,
) {
    let jump = computed(move || {
        app.settings
            .with(|state| state.config.focus_path.as_ref().is_some_and(&matches))
    });
    effect(move || {
        if jump.get() {
            reveal();
            after_answer(move || {
                if jump.get_untracked() {
                    handle.focus();
                    app.act(app.settings, |state, _| {
                        state.config.focus_path = None;
                        Vec::new()
                    });
                }
            });
        }
    });
}

/// The field's note, what it inherits while the draft leaves it unset, and the refusal's reasons
/// for it, read live.
fn under(app: AppState, field: &Field, live: Memo<Live>) -> AnyElement {
    let path = field.path.clone();
    let note = field.note;
    let typed = matches!(field.kind, Kind::Typed(_));
    let lines = computed(move || {
        let unset = live.with(|live| live.draft.get(&path).is_none())
            && !(typed
                && app.settings.with(|state| {
                    state
                        .config
                        .raw
                        .get(&path)
                        .is_some_and(|raw| !raw.trim().is_empty())
                }));
        let inherited = unset.then(|| {
            app.feed
                .with(|feed| live.with(|live| inherit::unset(&path, live, feed)))
        });
        let (reasons, invalid) = app.settings.with(|state| {
            (
                state
                    .config
                    .reasons_at(&path)
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
                state.config.invalid.contains(&path),
            )
        });
        (inherited, reasons, invalid)
    });
    dynamic(move || {
        let (inherited, reasons, invalid) = lines.get();
        untracked(move || {
            let mut parts: Vec<AnyElement> = Vec::new();
            if let Some(note) = note {
                parts.push(cell(app, note, Token::Ink3, false).into_element());
            }
            if let Some(inherited) = inherited {
                parts.push(cell(app, inherited, Token::Ink3, false).into_element());
            }
            if invalid {
                parts.push(cell(app, form::WHOLE_NUMBER, Token::Fail, false).into_element());
            }
            parts.extend(
                reasons
                    .into_iter()
                    .map(|reason| cell(app, reason, Token::Fail, false).into_element()),
            );
            view()
                .min_w(px(0.0))
                .gap(px(2.0))
                .children(parts)
                .into_element()
        })
    })
    .into_element()
}

fn id(path: &Path) -> String {
    format!("field-config-{path}")
}

/// The path of line `index` of the list at `path`: `projects.site.gate[1]`.
fn line_path(path: &Path, index: usize) -> Path {
    let mut line = path.clone();
    line.0.push(Seg::Index(index));
    line
}

/// One field: its control by kind, then what [`under`] says.
pub fn field(app: AppState, field: &Field, live: Memo<Live>) -> AnyElement {
    let control = match &field.kind {
        Kind::Text => text_field(app, field, live),
        Kind::Typed(typed) => typed_field(app, field, *typed, live),
        Kind::Switch { inherit } => switch(app, &field.path, *inherit, live),
        Kind::Lines { count, add } => lines(app, &field.path, field.label, *count, add, live),
        Kind::Pairs { names } => pairs(app, &field.path, names, live),
    };
    row(
        app,
        field.label,
        view()
            .min_w(px(0.0))
            .gap(px(if app.form == Form::Terminal { 0.0 } else { 4.0 }))
            .child(control)
            .child(under(app, field, live))
            .into_element(),
    )
}

fn text_field(app: AppState, field: &Field, live: Memo<Live>) -> AnyElement {
    let path = field.path.clone();
    let read = path.clone();
    let handle = element_ref();
    let entry = entry_with(
        app,
        id(&path),
        field.label,
        move || live.with(|live| live.draft.text(&read)),
        move |text| edit(app, Edit::Text(path.clone(), text)),
        Some(handle),
        CopyMode::Ordinary,
    );
    if field.offers.is_empty() {
        return entry;
    }
    let offers = field.offers.clone();
    let typed = field.path.clone();
    let offered = computed(move || {
        live.with(|live| crate::model::projects::suggest(&offers, &live.draft.text(&typed)))
    });
    let pick_path = field.path.clone();
    // The pressed value leaves the list, so once it has redrawn the keyboard returns to the
    // field (as the operation form's choices do).
    let picks = signal(0u64);
    effect(move || {
        if picks.get() > 0 {
            after_answer(move || handle.focus());
        }
    });
    view()
        .min_w(px(0.0))
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(entry)
        .child(suggestions(
            app,
            field.caption,
            format!("config-{}", field.path),
            offered,
            move |value| {
                edit(app, Edit::Text(pick_path.clone(), value));
                picks.update(|picks| *picks += 1);
            },
        ))
        .into_element()
}

fn typed_field(app: AppState, field: &Field, typed: Typed, live: Memo<Live>) -> AnyElement {
    let path = field.path.clone();
    let read = path.clone();
    entry(
        app,
        id(&path),
        field.label,
        move || {
            app.settings
                .with(|state| state.config.raw.get(&read).cloned())
                .unwrap_or_else(|| live.with(|live| live.draft.text(&read)))
        },
        move |text| edit(app, Edit::Typed(path.clone(), typed, text)),
    )
}

/// On and off, and Inherit where the key may be left to the workspace; the chosen one filled.
fn switch(app: AppState, path: &Path, inherit: bool, live: Memo<Live>) -> AnyElement {
    let read = path.clone();
    let current = computed(move || live.with(|live| live.draft.flag(&read)));
    let mut options = vec![(Some(true), "on"), (Some(false), "off")];
    if inherit {
        options.push((None, "inherit"));
    }
    let buttons = options
        .into_iter()
        .map(|(value, word)| {
            let path = path.clone();
            let id = format!("seg-config-{path}-{word}");
            dynamic(move || {
                let chosen = current.get() == value
                    // A workspace switch left unset reads as off, as the engine defaults it.
                    || (!inherit && value == Some(false) && current.get().is_none());
                let path = path.clone();
                untracked(|| {
                    button(
                        app,
                        word,
                        if chosen {
                            ButtonKind::Primary
                        } else {
                            ButtonKind::Ghost
                        },
                        id.clone(),
                        move || edit(app, Edit::Switch(path.clone(), value)),
                    )
                })
            })
            .into_element()
        })
        .collect();
    view()
        .role(AccessibilityRole::Radiogroup)
        .child(button_row(app, buttons))
        .into_element()
}

/// An ordered list: one entry per line with Up, Down and Remove, then Add.
fn lines(
    app: AppState,
    path: &Path,
    label: &'static str,
    count: usize,
    add: &'static str,
    live: Memo<Live>,
) -> AnyElement {
    let mut rows: Vec<AnyElement> = (0..count)
        .map(|index| {
            let read = path.clone();
            let write = path.clone();
            let line = entry(
                app,
                id(&line_path(path, index)),
                format!("{label} {}", index + 1),
                move || {
                    live.with(|live| {
                        live.draft
                            .lines(&read)
                            .get(index)
                            .cloned()
                            .unwrap_or_default()
                    })
                },
                move |text| edit(app, Edit::List(write.clone(), ListEdit::Set(index, text))),
            );
            let control = |word: &'static str, action: &'static str, change: ListEdit| {
                let path = path.clone();
                button(
                    app,
                    word,
                    ButtonKind::Quiet,
                    format!("btn-config-{path}-{index}-{action}"),
                    move || edit(app, Edit::List(path.clone(), change.clone())),
                )
            };
            view()
                .flex_row()
                .items_center()
                .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
                .min_w(px(0.0))
                .child(line)
                .child(button_row(
                    app,
                    vec![
                        control("Up", "up", ListEdit::Move(index, true)),
                        control("Down", "down", ListEdit::Move(index, false)),
                        control("Remove", "remove", ListEdit::Remove(index)),
                    ],
                ))
                .into_element()
        })
        .collect();
    let add_path = path.clone();
    rows.push(button_row(
        app,
        vec![button(
            app,
            add,
            ButtonKind::Ghost,
            format!("btn-config-{path}-add"),
            move || edit(app, Edit::List(add_path.clone(), ListEdit::Add)),
        )],
    ));
    view()
        .min_w(px(0.0))
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
        .children(rows)
        .into_element()
}

/// Names with values (an environment): each name with its value and Remove, then a new name
/// and value with Add.
fn pairs(app: AppState, path: &Path, names: &[String], live: Memo<Live>) -> AnyElement {
    let mut rows: Vec<AnyElement> = names
        .iter()
        .map(|name| {
            let (read, write, gone) = (path.clone(), path.clone(), path.clone());
            let (shown, typed, removed) = (name.clone(), name.clone(), name.clone());
            let value = entry_with(
                app,
                format!("{}.{name}", id(path)),
                name.clone(),
                move || live.with(|live| live.draft.pair(&read, &shown)),
                move |text| edit(app, Edit::Pair(write.clone(), typed.clone(), Some(text))),
                None,
                CopyMode::NativeOnly,
            );
            view()
                .flex_row()
                .items_center()
                .gap(px(if app.form == Form::Terminal { 0.0 } else { 8.0 }))
                .min_w(px(0.0))
                .child(
                    view()
                        .w(match app.form {
                            Form::Terminal => term::cells(16.0),
                            Form::Desktop => px(150.0),
                            Form::Phone => px(110.0),
                        })
                        .flex_shrink_0()
                        .child(mono_cell(app, name.clone(), Token::Ink)),
                )
                .child(value)
                .child(button(
                    app,
                    "Remove",
                    ButtonKind::Quiet,
                    format!("btn-config-{path}.{name}-remove"),
                    move || edit(app, Edit::Pair(gone.clone(), removed.clone(), None)),
                ))
                .into_element()
        })
        .collect();
    rows.push(new_pair(app, path));
    view()
        .min_w(px(0.0))
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
        .children(rows)
        .into_element()
}

/// The new name and value with Add. Removing the last pair hands the keyboard focus to the new
/// name (the editor's `added` names the pairs' own path then).
fn new_pair(app: AppState, path: &Path) -> AnyElement {
    let name = signal(String::new());
    let value = signal(String::new());
    let add_path = path.clone();
    let handle = element_ref();
    if app
        .settings
        .with_untracked(|state| state.config.added.as_ref() == Some(path))
    {
        after_answer(move || handle.focus());
    }
    let add = dynamic(move || {
        let disabled = name.with(|name| name.trim().is_empty());
        let path = add_path.clone();
        untracked(move || {
            button_with(
                app,
                "Add variable",
                ButtonKind::Ghost,
                ButtonOptions {
                    disabled,
                    ..ButtonOptions::default()
                },
                format!("btn-config-{path}-add"),
                move || {
                    let (named, valued) = (name.get_untracked(), value.get_untracked());
                    edit(
                        app,
                        Edit::Pair(path.clone(), named.trim().to_owned(), Some(valued)),
                    );
                    name.set(String::new());
                    value.set(String::new());
                },
            )
        })
    });
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(if app.form == Form::Terminal {
            16.0
        } else {
            8.0
        }))
        .min_w(px(0.0))
        .child(entry_with(
            app,
            format!("{}-new-name", id(path)),
            "New variable",
            move || name.get(),
            move |text| name.set(text),
            Some(handle),
            CopyMode::Ordinary,
        ))
        .child(entry_with(
            app,
            format!("{}-new-value", id(path)),
            "Value",
            move || value.get(),
            move |text| value.set(text),
            None,
            CopyMode::NativeOnly,
        ))
        .child(add)
        .into_element()
}
