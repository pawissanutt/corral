//! The tag field of an operation's form: the task's tags as one removable button each, a field
//! to add any other label exactly as typed (Return or Add; a label the engine would refuse is
//! said, never trimmed), and the built-in and already used tags to add with one press. The
//! form's value stays one tag a line (`model::tasks::meta`). Ids: the field `field-verb-<n>`,
//! `btn-tag-add`, `btn-tag-remove-<n>` for the n-th held tag, `tag-problem`, and `seg-tags-<tag>`
//! for each offered tag.

use nsh_core::prelude::*;

use crate::model::tasks::meta;
use crate::model::work::VerbField;
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::composer::copyable_input;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::entry;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::suggest::suggestions;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// Said above the tags a task can take that it does not hold yet.
const OFFERED: &str = "Add a tag";

/// The input is part of the whole form's draft: Save also adds any label still typed here.
#[derive(Clone, Copy)]
pub(super) struct TagDraft {
    typed: Signal<String>,
    problem: Signal<Option<String>>,
}

impl TagDraft {
    pub(super) fn new() -> Self {
        Self {
            typed: signal(String::new()),
            problem: signal(None),
        }
    }

    fn add(self, held: &str, tag: &str) -> Option<String> {
        match meta::add_tag(held, tag) {
            Ok(next) => {
                self.problem.set(None);
                Some(next)
            }
            Err(why) => {
                self.problem.set(Some(why));
                None
            }
        }
    }

    pub(super) fn finish(self, held: &mut String) -> bool {
        let tag = self.typed.get_untracked();
        if tag.is_empty() {
            return true;
        }
        let Some(next) = self.add(held, &tag) else {
            return false;
        };
        *held = next;
        true
    }
}

/// The field's rows; `values[index]` is its value and `send` sends the form.
pub(super) fn tag_row(
    app: AppState,
    field: &VerbField,
    index: usize,
    values: Signal<Vec<String>>,
    draft: TagDraft,
    send: impl Fn() + Clone + 'static,
) -> AnyElement {
    let form = app.form;
    let TagDraft { typed, problem } = draft;
    let held = move || values.with(|values| values.get(index).cloned().unwrap_or_default());
    let change = move |next: String| {
        values.update(|values| {
            if let Some(slot) = values.get_mut(index) {
                *slot = next;
            }
        });
    };
    let add = move |tag: String| {
        if let Some(next) = draft.add(&held(), &tag) {
            change(next);
            true
        } else {
            false
        }
    };
    let add_typed = move || {
        let tag = typed.get_untracked();
        if !tag.is_empty() && add(tag) {
            typed.set(String::new());
        }
    };
    let gap = px(if form == Form::Terminal { 0.0 } else { 6.0 });

    let tags = dynamic(move || {
        let tags = meta::split_tags(&held());
        if tags.is_empty() {
            let none = text(meta::UNTAGGED)
                .number_of_lines(1u32)
                .text_color(color(Token::Ink3));
            return match form {
                Form::Terminal => none.into_element(),
                _ => none
                    .font_family(FONT_TEXT)
                    .text_size(px(13.0))
                    .into_element(),
            };
        }
        let buttons: Vec<AnyElement> = tags
            .into_iter()
            .enumerate()
            .map(|(n, tag)| {
                let id = format!("btn-tag-remove-{n}");
                button(app, format!("{tag} ×"), ButtonKind::Ghost, id, move || {
                    change(meta::remove_tag(&held(), &tag));
                    problem.set(None);
                })
            })
            .collect();
        view()
            .flex_row()
            .flex_wrap()
            .gap(px(if form == Form::Terminal { 8.0 } else { 6.0 }))
            .role(AccessibilityRole::Group)
            .label("Tags")
            .children(buttons)
            .into_element()
    });

    let entry = copyable_input(app, None, move || typed.get())
        .test_id(&format!("field-verb-{index}"))
        .label(field.label)
        .placeholder(field.placeholder)
        .submit_behavior(SubmitBehavior::BlurAndSubmit)
        .on_change(move |value: &str| {
            typed.set(value.to_owned());
            problem.set(None);
        })
        .on_submit({
            let send = send.clone();
            move |_: &str| {
                if typed.get_untracked().is_empty() {
                    send();
                } else {
                    add_typed();
                }
            }
        })
        .flex_1();
    let entry = entry::look(entry, form, false);
    let adder = button(app, "Add", ButtonKind::Ghost, "btn-tag-add", add_typed);
    let entry_row = view()
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 8.0 } else { 6.0 }))
        .child(entry)
        .child(adder);

    let said = dynamic(move || {
        problem.with(|problem| {
            let line = problem.clone()?;
            let words = text(line).text_color(hue_ink(Hue::Held));
            let words = match form {
                Form::Terminal => words.into_element(),
                _ => words
                    .font_family(FONT_TEXT)
                    .text_size(px(12.5))
                    .line_height(px(18.0))
                    .into_element(),
            };
            Some(view().test_id("tag-problem").child(words))
        })
    });

    let offered = {
        let all = field.suggestions.clone();
        let shown = computed(move || meta::offered_tags(&all, &held(), &typed.get()));
        suggestions(app, OFFERED, "tags", shown, move |tag| {
            add(tag);
        })
    };

    view()
        .gap(gap)
        .child(entry::caption(form, field.label))
        .child(tags)
        .child(entry_row)
        .child(said)
        .child(offered)
        .into_element()
}
