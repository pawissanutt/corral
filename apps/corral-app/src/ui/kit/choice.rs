//! A form's closed set of values (a status, a priority, a profile, a branch) as one dropdown:
//! a trigger naming the chosen value, and a menu of every value offered, the chosen one marked.
//! A value the field holds but no longer offers (a profile since removed from the configuration)
//! still names itself on the trigger, so an edit that leaves it alone keeps it. Ids: the trigger
//! `field-verb-<n>`, each value `seg-<field>-<value>`, the field label lower-cased with dashes
//! and `none` for an empty value.

use nsh_base::{menu, menu_item};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::work::{self, Choice, VerbField};
use crate::state::AppState;
use crate::ui::kit::button::menu_trigger_reactive;
use crate::ui::kit::more::{backdrop, popup};
use crate::ui::kit::overlay::counted;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The phone's least touch target.
const TARGET: f32 = 44.0;

/// The `seg-<field>-…` group of a field's values.
fn group(label: &str) -> String {
    label.to_lowercase().replace(' ', "-")
}

/// The values field `index` offers: its own, or for a field that follows another's project
/// (a task's base) that project's branches as the feed serves them now, keeping the value the
/// form opened with and the one chosen, so changing the project never drops either.
pub fn offered(
    app: AppState,
    field: &VerbField,
    index: usize,
    values: Signal<Vec<String>>,
) -> Memo<Vec<Choice>> {
    let Some(project) = field.follows else {
        let choices = field.choices.clone();
        return computed(move || choices.clone());
    };
    let initial = field.value.clone();
    let task = field.base_task.clone();
    computed(move || {
        let (project, chosen) = values.with(|values| {
            let at = |at: usize| values.get(at).cloned().unwrap_or_default();
            (at(project), at(index))
        });
        app.feed.with(|feed| {
            work::base_choices(
                feed,
                &project,
                task.as_deref(),
                &[initial.as_str(), chosen.as_str()],
            )
        })
    })
}

/// The dropdown of the composer's field `index`, under `caption`, offering `offered` (read
/// again as it changes, so values that follow another field stay current) and writing the
/// chosen value into `values`.
pub fn choice_field(
    app: AppState,
    field: &VerbField,
    index: usize,
    values: Signal<Vec<String>>,
    caption: Text,
    offered: Memo<Vec<Choice>>,
) -> AnyElement {
    let form = app.form;
    let label = field.label;
    let group = group(label);
    let current = move || values.with(|values| values.get(index).cloned().unwrap_or_default());
    let shown = move || {
        let value = current();
        offered
            .get()
            .into_iter()
            .find(|choice| choice.value == value)
            .map_or(value, |choice| choice.label)
    };
    let picker = menu()
        .popup_style(popup(form))
        .backdrop_style(backdrop)
        .on_open_change(counted(app))
        .trigger(menu_trigger_reactive(
            app,
            move || shown().into(),
            move || format!("{label}: {}", shown()).into(),
            &format!("field-verb-{index}"),
        ))
        .child(dynamic(move || {
            let chosen = current();
            let choices = offered.get();
            untracked(|| {
                view()
                    .children(
                        choices
                            .into_iter()
                            .map(|choice| {
                                let on = choice.value == chosen;
                                item(app, &group, choice, on, values, index)
                            })
                            .collect::<Vec<_>>(),
                    )
                    .into_element()
            })
        }));
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(caption)
        // In a row, so the trigger keeps the width of its words rather than the form's.
        .child(view().flex_row().child(picker.into_element()))
        .into_element()
}

/// One offered value: its words, bold on a selected ground while it is the chosen one.
fn item(
    app: AppState,
    group: &str,
    choice: Choice,
    chosen: bool,
    values: Signal<Vec<String>>,
    index: usize,
) -> AnyElement {
    let Choice { value, label } = choice;
    let id = format!(
        "seg-{group}-{}",
        if value.is_empty() {
            "none"
        } else {
            value.as_str()
        }
    );
    let words = text(label.clone())
        .number_of_lines(1u32)
        .truncate()
        .text_color(color(Token::Ink))
        .when(chosen, |t| t.font_weight(FontWeight::BOLD));
    let content = match app.form {
        Form::Terminal => view()
            .flex_row()
            .items_center()
            .h(term::rows(1.0))
            .px(term::cells(1.0))
            .child(words),
        Form::Desktop => view()
            .px(px(12.0))
            .py(px(6.0))
            .child(words.font_family(FONT_TEXT).text_size(px(13.0))),
        Form::Phone => view()
            .justify_center()
            .min_h(px(TARGET + 4.0))
            .px(px(18.0))
            .py(px(8.0))
            .child(words.font_family(FONT_TEXT).text_size(px(16.0))),
    };
    menu_item()
        .accessibility_label(if chosen {
            format!("{label}, chosen")
        } else {
            label
        })
        .on_select(move || {
            values.update(|values| {
                if let Some(slot) = values.get_mut(index) {
                    slot.clone_from(&value);
                }
            });
        })
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .focus(|s| s.bg(color(Token::Hover)))
        .child(
            content
                .when(chosen, |c| c.bg(color(Token::Sel)))
                .test_id(&id),
        )
        .into_element()
}
