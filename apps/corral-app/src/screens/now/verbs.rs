//! The record verbs of the model's table (`work::record_verbs`) where a row or a page offers
//! them: a row's context menu (desktop and terminal: the secondary button, Shift+F10 or the
//! Menu key; phone: a held finger opens it as the action sheet), a control's press-opened menu,
//! the reasons under an action row's unavailable commands, and the work state's overlays the
//! shells host once beside the confirmation: the command menu the keyboard opens (`.`) and the
//! composer a text verb opens (a comment, a park reason). A run's Session menu opens its task's
//! Actions menu as a submenu (beside it, or in the sheet's place on the phone).

use nsh_base::{button as press_target, context_menu, menu, menu_item, submenu};
use nsh_core::element::{StyleRefinement, color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::commands::CONFIRM_CANCEL;
use crate::model::projects;
use crate::model::work::{self, Button, Press, Record, VerbAction};
use crate::model::{Form, Hue, Live};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::overlay::{Modal, counted, modal};
use crate::ui::kit::suggest::suggestions;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::parts::press;

/// The phone's least touch target.
const TARGET: f32 = 44.0;
/// The desktop menu's width range: room for "Remove from queue" and a short reason under it,
/// never a line as wide as the inspector.
const MENU_MIN: f32 = 220.0;
const MENU_MAX: f32 = 340.0;

/// The record a list row's press is about: a task's select or open. Local presses (a tab, a
/// group's rows) carry no commands, so they get no menu.
pub(crate) fn row_record(action: &Press) -> Option<Record> {
    match action {
        Press::Select(id) | Press::Open(Live::Task(id)) => Some(Record::Task(id.clone())),
        _ => None,
    }
}

/// `row` with the verb menu of `record`. The menu's items are made when it opens, from the feed
/// as it stands then, so a row costs nothing until the owner asks for its commands.
pub(crate) fn with_menu(app: AppState, record: Record, row: AnyElement) -> AnyElement {
    context_menu()
        .popup_style(popup(app.form))
        .backdrop_style(backdrop)
        .on_open_change(counted(app))
        .trigger(row)
        .child(dynamic(move || {
            let record = record.clone();
            untracked(move || items(app, &record))
        }))
        .into_element()
}

/// The same verb menu opened by pressing `trigger` (a page head's Actions control): a popup at
/// the trigger on desktop and terminal, the action sheet on the phone; Return, Space or Down on
/// the focused trigger open it with the first available verb focused.
pub(crate) fn verb_menu(app: AppState, record: Record, trigger: AnyElement) -> AnyElement {
    menu()
        .popup_style(popup(app.form))
        .backdrop_style(backdrop)
        .on_open_change(counted(app))
        .trigger(trigger)
        .child(dynamic(move || {
            let record = record.clone();
            untracked(move || items(app, &record))
        }))
        .into_element()
}

/// The menu's surface for the form: a raised card on desktop, the sheet's top on the phone, a
/// framed box in a terminal.
fn popup(form: Form) -> impl Fn(StyleRefinement) -> StyleRefinement + 'static {
    move |s| {
        let s = s
            .bg(color(Token::Surface))
            .border_1()
            .border_color(color(Token::Rule2));
        match form {
            Form::Desktop => s
                .min_w(px(MENU_MIN))
                .max_w(px(MENU_MAX))
                .py(px(4.0))
                .rounded(px(8.0))
                .shadow_lg()
                .shadow_color(color(Token::Shell)),
            Form::Phone => s.rounded_t(px(14.0)).pt(px(8.0)).pb(px(24.0)),
            Form::Terminal => s.min_w(term::cells(24.0)),
        }
    }
}

/// The phone sheet's scrim, as the modal sheet's.
fn backdrop(s: StyleRefinement) -> StyleRefinement {
    s.bg(color(Token::Shell).opacity(0.5))
}

/// Every engine operation of the record, in the table's order; each runs when chosen, or
/// opens the composer when it asks for input. Nothing is predicted or disabled: the engine
/// decides, and a refusal opens its sheet. A Session menu's Task entry is a submenu holding the
/// task's Actions menu; its Focus pane says why where it cannot work.
fn items(app: AppState, record: &Record) -> AnyElement {
    let verbs = app
        .feed
        .with_untracked(|feed| work::record_verbs(record, feed));
    view()
        .children(
            verbs
                .into_iter()
                .map(|verb| match &verb.press {
                    Press::Menu(inner) => task_menu(app, inner.clone(), verb.label, verb.slug),
                    _ => item(app, verb),
                })
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// The Task entry of a Session menu: the task's Actions menu, opened beside the entry on
/// desktop (its chevron; ` ›` in a terminal) and in the sheet's place on the phone, under a
/// Back row. test_id `btn-<slug>`.
fn task_menu(app: AppState, record: Record, label: String, slug: &'static str) -> AnyElement {
    let form = app.form;
    let words = text(label)
        .number_of_lines(1u32)
        .truncate()
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(Token::Ink));
    let trigger = match form {
        Form::Terminal => view()
            .flex_row()
            .items_center()
            .h(term::rows(1.0))
            .px(term::cells(1.0))
            .child(words),
        Form::Desktop => view()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .px(px(12.0))
            .py(px(6.0))
            .child(words.font_family(FONT_TEXT).text_size(px(12.5)))
            .child(icon(Icon::Chevron, 12.0, color(Token::Ink3))),
        Form::Phone => view()
            .flex_row()
            .items_center()
            .min_h(px(TARGET + 4.0))
            .px(px(18.0))
            .py(px(8.0))
            .child(words.font_family(FONT_TEXT).text_size(px(16.0)))
            .child(icon(Icon::Chevron, 16.0, color(Token::Ink3))),
    };
    let trigger = trigger
        .test_id(&format!("btn-{slug}"))
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .focus(|s| s.bg(color(Token::Hover)));
    let back = view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .min_h(px(TARGET))
        .px(px(14.0))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label("Back to the Session menu")
        .test_id("btn-menu-back")
        .child(icon(Icon::Back, 16.0, color(Token::Ink2)))
        .child(
            text("Session")
                .font_family(FONT_TEXT)
                .text_size(px(15.0))
                .text_color(color(Token::Ink2)),
        );
    submenu()
        .trigger(trigger)
        .back(back)
        .child(dynamic(move || {
            let record = record.clone();
            untracked(move || items(app, &record))
        }))
        .into_element()
}

fn item(app: AppState, verb: VerbAction) -> AnyElement {
    let form = app.form;
    let VerbAction {
        slug,
        label,
        enabled,
        reason,
        press: action,
        ..
    } = verb;
    let spoken = match (&reason, enabled) {
        (Some(reason), false) => format!("{label}, {reason}"),
        _ => label.clone(),
    };
    // An unavailable verb reads a step quieter, not faded: its reason must stay legible.
    let words = text(label)
        .number_of_lines(1u32)
        .truncate()
        .text_color(color(if enabled { Token::Ink } else { Token::Ink3 }));
    let why = reason.filter(|_| !enabled).map(|reason| {
        let why = text(reason)
            .number_of_lines(if form == Form::Terminal { 1u32 } else { 2u32 })
            .truncate()
            .text_color(color(Token::Ink3));
        match form {
            Form::Terminal => why.flex_1().min_w(px(0.0)).into_element(),
            Form::Desktop => why
                .font_family(FONT_TEXT)
                .text_size(px(11.5))
                .into_element(),
            Form::Phone => why
                .font_family(FONT_TEXT)
                .text_size(px(13.0))
                .into_element(),
        }
    });
    let content = match form {
        Form::Terminal => view()
            .flex_row()
            .items_center()
            .gap(term::cells(2.0))
            .h(term::rows(1.0))
            .px(term::cells(1.0))
            .child(words.flex_shrink_0())
            .children(why),
        Form::Desktop => view()
            .gap(px(1.0))
            .px(px(12.0))
            .py(px(6.0))
            .child(words.font_family(FONT_TEXT).text_size(px(12.5)))
            .children(why),
        Form::Phone => view()
            .justify_center()
            .gap(px(2.0))
            .min_h(px(TARGET + 4.0))
            .px(px(18.0))
            .py(px(8.0))
            .child(words.font_family(FONT_TEXT).text_size(px(16.0)))
            .children(why),
    };
    menu_item()
        .accessibility_label(spoken)
        .disabled(!enabled)
        .on_select(move || press(app, action.clone()))
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .focus(|s| s.bg(color(Token::Hover)))
        .child(content.test_id(&format!("btn-{slug}")))
        .into_element()
}

/// Why the commands of `buttons` that cannot run now cannot: the commands sharing a reason
/// named together, then the model's reason, one wrapping line per reason under the action row,
/// so a held task's many waiting commands read as one fact. Nothing when every command can run.
pub(crate) fn reasons(app: AppState, buttons: &[Button]) -> Option<AnyElement> {
    let form = app.form;
    let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
    for button in buttons.iter().filter(|button| !button.enabled) {
        let Some(reason) = &button.reason else {
            continue;
        };
        let label = button.label.to_string();
        match grouped.iter_mut().find(|(known, _)| known == reason) {
            Some((_, labels)) => labels.push(label),
            None => grouped.push((reason.clone(), vec![label])),
        }
    }
    let size = match form {
        Form::Terminal => None,
        Form::Desktop => Some(12.0),
        Form::Phone => Some(13.5),
    };
    let lines: Vec<AnyElement> = grouped
        .into_iter()
        .map(|(reason, labels)| {
            let names = text(labels.join(", "))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(Token::Ink2));
            let why = text(format!("  {reason}")).text_color(color(Token::Ink3));
            let line = text("").min_w(px(0.0)).children([names, why]);
            match size {
                Some(size) => line
                    .font_family(FONT_TEXT)
                    .text_size(px(size))
                    .line_height(px((size * 1.45).round()))
                    .into_element(),
                None => line.into_element(),
            }
        })
        .collect();
    (!lines.is_empty()).then(|| {
        view()
            .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
            .pt(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
            .children(lines)
            .into_element()
    })
}

/// Which of the work state's overlays shows: the record's command menu the keyboard opened
/// (`.`), or a text verb's composer, which a chosen verb opens in the menu's place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Overlay {
    Hidden,
    Menu,
    Composer,
}

fn overlay(state: &work::State) -> Overlay {
    if state.composer.is_some() {
        Overlay::Composer
    } else if state.menu.is_some() {
        Overlay::Menu
    } else {
        Overlay::Hidden
    }
}

/// The work state's overlays, hosted once by each shell beside the confirmation: the command
/// menu the keyboard opens on the selected record, and the composer of a text verb. Each is a
/// dialog on desktop, a sheet on the phone, the framed region over the page in a terminal. The
/// host follows the overlay last shown, so a closing sheet leaves with its own exit; a verb
/// chosen in the menu that needs words replaces it with its composer, and a Session menu's Task
/// entry replaces it with the task's menu.
pub fn verb_composer(app: AppState) -> AnyElement {
    let shown = computed(move || app.work.with(overlay));
    let host = signal(Overlay::Composer);
    let listed = signal(None::<Record>);
    effect(move || {
        let now = shown.get();
        if now != Overlay::Hidden && untracked(|| host.get()) != now {
            host.set(now);
        }
        // The menu's record while it shows; a closing menu keeps the last, so it leaves whole.
        if let Some(record) = app.work.with(|state| state.menu.clone())
            && untracked(|| listed.get()).as_ref() != Some(&record)
        {
            listed.set(Some(record));
        }
    });
    dynamic(move || {
        let kind = host.get();
        if kind == Overlay::Menu {
            listed.with(|_| ());
        }
        untracked(|| match kind {
            Overlay::Menu => modal(
                app,
                move || shown.get() == Overlay::Menu,
                move || {
                    if app.work.with_untracked(|state| state.menu.is_some()) {
                        press(app, Press::CloseMenu);
                    }
                },
                move || command_menu(app),
            ),
            Overlay::Composer | Overlay::Hidden => modal(
                app,
                move || shown.get() == Overlay::Composer,
                move || {
                    if app.work.with_untracked(|state| state.composer.is_some()) {
                        press(app, Press::CancelVerb);
                    }
                },
                move || composer(app),
            ),
        })
    })
}

/// The keyboard's command menu for the record the work state names: every operation in the
/// table's order, the first one focused, so Tab moves, Return runs, Escape closes.
/// test_id "verbs".
fn command_menu(app: AppState) -> Modal {
    let form = app.form;
    let record = app.work.with_untracked(|state| state.menu.clone());
    let verbs = app.feed.with_untracked(|feed| {
        record
            .as_ref()
            .map_or_else(Vec::new, |record| work::record_verbs(record, feed))
    });
    let first = element_ref();
    let mut focused = false;
    let rows: Vec<AnyElement> = verbs
        .into_iter()
        .map(|verb| {
            let focus = (verb.enabled && !focused).then(|| {
                focused = true;
                first
            });
            let label = match &verb.press {
                Press::Menu(_) => format!("{} ›", verb.label),
                _ => verb.label.clone(),
            };
            command_row(app, VerbAction { label, ..verb }, focus)
        })
        .collect();
    if focused {
        after_answer(move || first.focus());
    }
    Modal {
        test_id: "verbs".into(),
        title: record
            .as_ref()
            .map_or_else(String::new, |record| record.id().to_owned()),
        sub: None,
        body: view()
            .gap(px(if form == Form::Terminal { 0.0 } else { 2.0 }))
            .role(AccessibilityRole::List)
            .children(rows)
            .into_element(),
        done: None,
        confirm: None,
    }
}

/// One verb of the keyboard's menu: its label, and under it (beside it in a terminal) why it
/// cannot run now; choosing it runs it, or opens its composer.
fn command_row(app: AppState, verb: VerbAction, focus: Option<ElementRef>) -> AnyElement {
    let form = app.form;
    let VerbAction {
        slug,
        label,
        enabled,
        reason,
        press: action,
        ..
    } = verb;
    let spoken = match (&reason, enabled) {
        (Some(reason), false) => format!("{label}, {reason}"),
        _ => label.clone(),
    };
    let words = text(label)
        .number_of_lines(1u32)
        .truncate()
        .text_color(color(if enabled { Token::Ink } else { Token::Ink3 }));
    let why = reason.filter(|_| !enabled).map(|reason| {
        let why = text(reason).text_color(color(Token::Ink3));
        match form {
            Form::Terminal => why.flex_1().min_w(px(0.0)).number_of_lines(1u32).truncate(),
            Form::Desktop => why.font_family(FONT_TEXT).text_size(px(11.5)),
            Form::Phone => why.font_family(FONT_TEXT).text_size(px(13.0)),
        }
    });
    let row = press_target()
        .disabled(!enabled)
        .on_press(move |_: &()| press(app, action.clone()))
        .when_some(focus, |row, handle| row.element_ref(&handle))
        .accessibility_label(spoken)
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .focus_visible(|s| s.bg(color(Token::Hover)));
    let styled = match form {
        Form::Terminal => row
            .flex_row()
            .items_center()
            .gap(term::cells(2.0))
            .h(term::rows(1.0))
            .px(term::cells(1.0))
            .child(words.flex_shrink_0())
            .children(why),
        Form::Desktop => row
            .gap(px(1.0))
            .px(px(10.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .child(words.font_family(FONT_TEXT).text_size(px(13.0)))
            .children(why),
        Form::Phone => row
            .justify_center()
            .gap(px(2.0))
            .min_h(px(TARGET + 4.0))
            .py(px(8.0))
            .border_b_1()
            .border_b_color(color(Token::Rule))
            .child(words.font_family(FONT_TEXT).text_size(px(16.0)))
            .children(why),
    };
    view()
        .test_id(&format!("btn-{slug}"))
        .child(styled)
        .into_element()
}

fn composer(app: AppState) -> Modal {
    let form = app.form;
    let (title, fields, submit) = app.work.with_untracked(|state| {
        state.composer.as_ref().map_or_else(
            || (String::new(), Vec::new(), String::new()),
            |composer| {
                (
                    composer.title.clone(),
                    composer.fields.clone(),
                    composer.submit_label.clone(),
                )
            },
        )
    });
    let values = signal(
        fields
            .iter()
            .map(|field| field.value.clone())
            .collect::<Vec<_>>(),
    );
    let required: Vec<bool> = fields.iter().map(|field| field.required).collect();
    let ready = computed(move || {
        values.with(|values| {
            values
                .iter()
                .zip(&required)
                .all(|(value, required)| !required || !value.trim().is_empty())
        })
    });
    let send = move || {
        if ready.get_untracked() {
            press(app, Press::SubmitVerb(values.get_untracked()));
        }
    };
    let rows: Vec<AnyElement> = fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let caption = text(field.label)
                .number_of_lines(1u32)
                .text_color(color(Token::Ink2));
            let caption = match form {
                Form::Terminal => caption,
                Form::Desktop => caption
                    .font_family(FONT_TEXT)
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD),
                Form::Phone => caption
                    .font_family(FONT_TEXT)
                    .text_size(px(14.0))
                    .font_weight(FontWeight::SEMIBOLD),
            };
            if !field.choices.is_empty() {
                return choice_row(app, field, index, values, caption);
            }
            let entry = input()
                .test_id(&format!("field-verb-{index}"))
                .label(field.label)
                .placeholder(field.placeholder)
                .auto_focus(index == 0)
                .value(move || values.with(|values| values.get(index).cloned().unwrap_or_default()))
                .on_change(move |typed: &str| {
                    values.update(|values| {
                        if let Some(value) = values.get_mut(index) {
                            *value = typed.to_owned();
                        }
                    });
                })
                .on_submit(move |_: &str| send())
                .min_w(px(0.0))
                .text_color(color(Token::Ink))
                .placeholder_color(move || for_appearance(color(Token::Ink4)))
                .caret_color(move || for_appearance(color(Token::Ink)))
                .selection_color(move || for_appearance(color(Token::Sel)))
                .bg(AdaptiveColor::TRANSPARENT)
                .border_1()
                .border_color(color(Token::Rule2))
                .focus(|s| s.border_color(color(Token::Focus)));
            let entry = match form {
                Form::Terminal => entry.h(term::rows(1.0)).px(term::cells(1.0)),
                Form::Desktop => entry
                    .font_family(FONT_TEXT)
                    .text_size(px(13.0))
                    .h(px(32.0))
                    .px(px(10.0))
                    .rounded(px(6.0)),
                Form::Phone => entry
                    .font_family(FONT_TEXT)
                    .text_size(px(16.0))
                    .min_h(px(TARGET))
                    .px(px(12.0))
                    .rounded(px(10.0)),
            };
            let offered = (!field.suggestions.is_empty()).then(|| {
                let all = field.suggestions.clone();
                let typed = computed(move || {
                    values.with(|values| {
                        projects::suggest(&all, values.get(index).map_or("", String::as_str))
                    })
                });
                let pick = move |value: String| {
                    values.update(|values| {
                        if let Some(slot) = values.get_mut(index) {
                            *slot = value;
                        }
                    });
                };
                let group = field.label.to_lowercase().replace(' ', "-");
                suggestions(app, projects::BRANCHES_CAPTION, group, typed, pick)
            });
            view()
                .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
                .child(caption)
                .child(entry)
                .children(offered)
                .into_element()
        })
        .collect();
    let error = dynamic(move || {
        app.work.with(|state| {
            let line = state.composer.as_ref()?.error.clone()?;
            let words = text(line).text_color(hue_ink(Hue::Held));
            Some(match form {
                Form::Terminal => words.into_element(),
                _ => words
                    .font_family(FONT_TEXT)
                    .text_size(px(12.5))
                    .line_height(px(18.0))
                    .into_element(),
            })
        })
    });
    let phone = form == Form::Phone;
    let cancel = button_with(
        app,
        CONFIRM_CANCEL,
        ButtonKind::Ghost,
        ButtonOptions {
            wide: phone,
            key_hint: Some("Esc"),
            ..ButtonOptions::default()
        },
        "btn-verb-cancel",
        move || press(app, Press::CancelVerb),
    );
    let submit = dynamic(move || {
        button_with(
            app,
            submit.clone(),
            ButtonKind::Primary,
            ButtonOptions {
                wide: phone,
                disabled: !ready.get(),
                ..ButtonOptions::default()
            },
            "btn-verb-send",
            send,
        )
    });
    let actions = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .when(!phone, |row| row.justify_end())
        .child(cancel)
        .child(submit);
    let body = view()
        .gap(px(match form {
            Form::Terminal => 0.0,
            Form::Desktop => 12.0,
            Form::Phone => 14.0,
        }))
        .when(form != Form::Terminal, |body| body.pt(px(4.0)))
        .children(rows)
        .child(error)
        .when(form == Form::Terminal, |body| body.child(term::blank()))
        .child(actions)
        .into_element();
    Modal {
        test_id: "verb".into(),
        title,
        sub: None,
        body,
        done: None,
        confirm: None,
    }
}

/// A closed set of values (a status, a verdict, a profile) as one button each, the chosen one
/// filled: a press picks it, Tab walks them, and Return or Space presses the focused one. A
/// chosen button is drawn anew in its filled look, so the keyboard focus is put back on it.
/// test_id `seg-<field>-<value>`, the field label lower-cased with dashes, `none` for an empty
/// value.
fn choice_row(
    app: AppState,
    field: &work::VerbField,
    index: usize,
    values: Signal<Vec<String>>,
    caption: Text,
) -> AnyElement {
    let group = field.label.to_lowercase().replace(' ', "-");
    // Runs under the row, which outlives the buttons a choice redraws.
    let chosen_at = stored(None::<ElementRef>);
    let picks = signal(0u64);
    effect(move || {
        if picks.get() > 0
            && let Some(handle) = chosen_at.get()
        {
            after_answer(move || handle.focus());
        }
    });
    let buttons: Vec<AnyElement> = field
        .choices
        .iter()
        .map(|choice| {
            let shown = if choice.value.is_empty() {
                "none"
            } else {
                choice.value.as_str()
            };
            let id = format!("seg-{group}-{shown}");
            let work::Choice { value, label } = choice.clone();
            let handle = element_ref();
            dynamic(move || {
                let chosen = values.with(|values| values.get(index) == Some(&value));
                let value = value.clone();
                button_with(
                    app,
                    label.clone(),
                    if chosen {
                        ButtonKind::Primary
                    } else {
                        ButtonKind::Ghost
                    },
                    ButtonOptions {
                        focus: Some(handle),
                        ..ButtonOptions::default()
                    },
                    id.clone(),
                    move || {
                        values.update(|values| {
                            if let Some(slot) = values.get_mut(index) {
                                slot.clone_from(&value);
                            }
                        });
                        chosen_at.replace(Some(handle));
                        picks.update(|picks| *picks += 1);
                    },
                )
            })
        })
        .collect();
    view()
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(caption)
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(if app.form == Form::Terminal { 8.0 } else { 6.0 }))
                .role(AccessibilityRole::Radiogroup)
                .label(field.label)
                .children(buttons),
        )
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Id;

    /// RC-03: a task's row, selected in place or opened on the phone, offers its commands; a
    /// local press offers none.
    #[test]
    fn task_rows_carry_their_command_menu_and_local_presses_none() {
        let id = Id::new("T-141");
        assert_eq!(
            row_record(&Press::Select(id.clone())),
            Some(Record::Task(id.clone()))
        );
        assert_eq!(
            row_record(&Press::Open(Live::Task(id.clone()))),
            Some(Record::Task(id))
        );
        assert_eq!(row_record(&Press::Work(work::Action::Back)), None);
    }
}
