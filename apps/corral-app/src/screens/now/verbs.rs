//! The record verbs of the model's table (`work::record_verbs`) where a row or a page offers
//! them: a row's context menu (desktop and terminal: the secondary button, Shift+F10 or the
//! Menu key; phone: a held finger opens it as the action sheet), a control's press-opened menu,
//! the reasons under an action row's unavailable commands, and the work state's overlays the
//! shells host once beside the confirmation: the command menu the keyboard opens (`.`) and the
//! composer a text verb opens (a comment, a park reason).

use nsh_base::{button as press_target, context_menu, menu, menu_item};
use nsh_core::element::{StyleRefinement, color as for_appearance, input};
use nsh_core::prelude::*;

use crate::model::commands::CONFIRM_CANCEL;
use crate::model::work::{self, Button, Press, Record, VerbAction};
use crate::model::{Form, Hue, Live};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::overlay::{Modal, modal};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::parts::{press, pressable};

/// The phone's least touch target.
const TARGET: f32 = 44.0;
/// The desktop menu's width range: room for "Remove from queue" and a short reason under it,
/// never a line as wide as the inspector.
const MENU_MIN: f32 = 220.0;
const MENU_MAX: f32 = 340.0;

/// The record a list row's press is about: a daemon task's select or open. Fixture rows (local
/// demo actions) carry no daemon commands, so they get no menu.
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

/// Every verb of the record, in the table's order: the available ones run when chosen (a verb
/// with fields opens the composer instead), the unavailable ones stay in place, dimmed, with
/// the model's reason under them.
fn items(app: AppState, record: &Record) -> AnyElement {
    let verbs = app.feed.with(|feed| work::record_verbs(feed, record));
    view()
        .children(
            verbs
                .into_iter()
                .map(|verb| item(app, verb))
                .collect::<Vec<_>>(),
        )
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

/// The verbs of a record as an action row where no story carries them (a Review candidate):
/// each verb's button (`btn-<slug>`), dimmed while it cannot run, then the reasons why. Nothing
/// when the record has no verbs.
pub(crate) fn verb_buttons(app: AppState, verbs: Vec<VerbAction>) -> Option<AnyElement> {
    if verbs.is_empty() {
        return None;
    }
    let buttons: Vec<Button> = verbs
        .into_iter()
        .map(|verb| {
            let mut button =
                Button::new(verb.label, verb.press, work::ButtonKind::Ghost).enabled(verb.enabled);
            button.reason = verb.reason;
            button
        })
        .collect();
    Some(
        view()
            .child(
                view().flex_row().flex_wrap().gap(px(8.0)).children(
                    buttons
                        .iter()
                        .map(|button| pressable(app, button, false))
                        .collect::<Vec<_>>(),
                ),
            )
            .children(reasons(app, &buttons))
            .into_element(),
    )
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
/// chosen in the menu that needs words replaces it with its composer.
pub fn verb_composer(app: AppState) -> AnyElement {
    let shown = computed(move || app.work.with(overlay));
    let host = signal(Overlay::Composer);
    effect(move || {
        let now = shown.get();
        if now != Overlay::Hidden && untracked(|| host.get()) != now {
            host.set(now);
        }
    });
    dynamic(move || {
        let kind = host.get();
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

/// The keyboard's command menu for the record the work state names: every verb of the table in
/// its order, the first available one focused, so Tab moves, Return runs, Escape closes; an
/// unavailable verb stays in place, quieter, with its reason. test_id "verbs".
fn command_menu(app: AppState) -> Modal {
    let form = app.form;
    let record = app.work.with_untracked(|state| state.menu.clone());
    let verbs = record.as_ref().map_or_else(Vec::new, |record| {
        app.feed
            .with_untracked(|feed| work::record_verbs(feed, record))
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
            command_row(app, verb, focus)
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
            view()
                .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
                .child(caption)
                .child(entry)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Id;

    /// RC-03: a daemon task's row, selected in place or opened on the phone, offers its commands;
    /// a fixture row (a local demo action) offers none.
    #[test]
    fn daemon_task_rows_carry_their_command_menu_and_fixture_rows_none() {
        let id = Id::new("T-141");
        assert_eq!(
            row_record(&Press::Select(id.clone())),
            Some(Record::Task(id.clone()))
        );
        assert_eq!(
            row_record(&Press::Open(Live::Task(id.clone()))),
            Some(Record::Task(id))
        );
        assert_eq!(
            row_record(&Press::Work(work::Action::Select("T-890"))),
            None
        );
    }
}
