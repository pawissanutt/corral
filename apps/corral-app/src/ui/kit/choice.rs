//! A form's closed set of values (a status, a priority, a profile, a branch) as one dropdown:
//! a trigger naming the chosen value, and a menu of every value offered, the chosen one marked.
//! A value the field holds but no longer offers (a profile since removed from the configuration)
//! still names itself on the trigger, so an edit that leaves it alone keeps it. Ids: the trigger
//! `field-verb-<n>`, each value `seg-<field>-<value>`, the field label lower-cased with dashes
//! and `none` for an empty value.
//!
//! A field that follows another's project (a task's base) offers that project's branches as
//! corrald serves them, a page at a time: the menu ends in More branches (`btn-<field>-more`)
//! while corrald names a further page, which adds it without closing the menu; Loading
//! (`<field>-loading`) while a page is on its way; and why none could be read, with Retry
//! (`<field>-error`, `btn-<field>-retry`), after one failed.

use corral_protocol::detail::{DetailGet, DetailRef};
use nsh_base::{menu, menu_item};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::projects::{BranchPage, branch_page, branches_ref};
use crate::model::work::{self, Choice, VerbField};
use crate::screens::task::keep::keep_details;
use crate::state::AppState;
use crate::ui::kit::button::menu_trigger_reactive;
use crate::ui::kit::more::{backdrop, popup};
use crate::ui::kit::overlay::counted;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The phone's least touch target.
const TARGET: f32 = 44.0;
/// More branches' words.
pub const MORE: &str = "More branches";
/// What stands under the branches while a page is on its way.
pub const LOADING: &str = "Loading branches…";
/// Retry's words, after a page failed.
pub const RETRY: &str = "Retry";

/// The `seg-<field>-…` group of a field's values.
fn group(label: &str) -> String {
    label.to_lowercase().replace(' ', "-")
}

/// The project field `index` follows, read as the form changes; none for a field of its own.
fn followed(field: &VerbField, values: Signal<Vec<String>>) -> Option<Memo<String>> {
    let at = field.follows?;
    Some(computed(move || {
        values.with(|values| values.get(at).cloned().unwrap_or_default())
    }))
}

/// The values field `index` offers: its own, or for a field that follows another's project
/// (a task's base) that project's branches as corrald has served them so far, keeping the value
/// the form opened with and the one chosen, so changing the project or reading further pages
/// never drops either.
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

/// The served branches of the project a following field names, and how far they were read;
/// none while no project is chosen.
type Paging = Memo<Option<(DetailRef, BranchPage)>>;

/// Keeps the followed project's first page of branches loaded (and pinned) while the field is
/// shown, read again when the project changes, and reads how far its pages go.
fn branch_paging(app: AppState, project: Memo<String>) -> Paging {
    let target = computed(move || {
        let project = project.get();
        (!project.is_empty()).then(|| branches_ref(&project))
    });
    keep_details(
        app,
        computed(move || target.get().into_iter().collect()),
        None,
    );
    computed(move || {
        let project = project.get();
        let target = target.get()?;
        let page = app.feed.with(|feed| branch_page(feed, &project));
        Some((target, page))
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
    let paging = followed(field, values).map(|project| branch_paging(app, project));
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
            let paged = paging.and_then(|paging| paging.get());
            untracked(|| {
                let mut rows = choices
                    .into_iter()
                    .map(|choice| {
                        let on = choice.value == chosen;
                        item(app, &group, choice, on, values, index)
                    })
                    .collect::<Vec<_>>();
                if let Some((target, page)) = paged {
                    rows.extend(foot(app, &group, target, page));
                }
                view().children(rows).into_element()
            })
        }));
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(caption)
        // In a row, so the trigger keeps the width of its words rather than the form's.
        .child(view().flex_row().child(picker.into_element()))
        .into_element()
}

/// What follows the branches served so far: why they could not all be read with Retry, More
/// branches while corrald names a further page, Loading while one is on its way. A failed page
/// leaves those already read offered.
fn foot(app: AppState, group: &str, target: DetailRef, page: BranchPage) -> Vec<AnyElement> {
    let form = app.form;
    if let Some(error) = page.error {
        let why = format!("Branches unavailable: {error}");
        let note = content(form, words(form, why, Token::Ink2))
            .border_t_1()
            .border_color(color(Token::Rule))
            .test_id(&format!("{group}-error"));
        let retry = action(
            app,
            RETRY,
            &format!("btn-{group}-retry"),
            false,
            move || {
                app.refresh(target.clone());
            },
        );
        return vec![note.into_element(), retry];
    }
    if let Some(next) = page.next {
        let more = action(app, MORE, &format!("btn-{group}-more"), true, move || {
            app.detail(DetailGet {
                target: target.clone(),
                etag: None,
                page: Some(next.clone()),
            });
        });
        return vec![more];
    }
    if page.loading {
        let note = content(form, words(form, LOADING.to_owned(), Token::Ink2))
            .border_t_1()
            .border_color(color(Token::Rule))
            .test_id(&format!("{group}-loading"));
        return vec![note.into_element()];
    }
    Vec::new()
}

/// A row of the foot that acts rather than chooses: it keeps the menu open, so the branches it
/// brings are there to choose from.
fn action(
    app: AppState,
    label: &'static str,
    id: &str,
    ruled: bool,
    run: impl Fn() + 'static,
) -> AnyElement {
    let form = app.form;
    let row = content(form, words(form, label.to_owned(), Token::Ink));
    let row = if ruled {
        row.border_t_1().border_color(color(Token::Rule))
    } else {
        row
    };
    menu_item()
        .close_on_select(false)
        .accessibility_label(label)
        .on_select(run)
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .focus(|s| s.bg(color(Token::Hover)))
        .child(row.test_id(id))
        .into_element()
}

/// A menu row's words on `form`, in `ink`.
fn words(form: Form, label: String, ink: Token) -> Text {
    let words = text(label)
        .number_of_lines(1u32)
        .truncate()
        .text_color(color(ink));
    match form {
        Form::Terminal => words,
        Form::Desktop => words.font_family(FONT_TEXT).text_size(px(13.0)),
        Form::Phone => words.font_family(FONT_TEXT).text_size(px(16.0)),
    }
}

/// A menu row around `words`, sized for `form`.
fn content(form: Form, words: Text) -> View {
    match form {
        Form::Terminal => view()
            .flex_row()
            .items_center()
            .h(term::rows(1.0))
            .px(term::cells(1.0))
            .child(words),
        Form::Desktop => view().px(px(12.0)).py(px(6.0)).child(words),
        Form::Phone => view()
            .justify_center()
            .min_h(px(TARGET + 4.0))
            .px(px(18.0))
            .py(px(8.0))
            .child(words),
    }
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
    let words = words(app.form, label.clone(), Token::Ink)
        .when(chosen, |t| t.font_weight(FontWeight::BOLD));
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
            content(app.form, words)
                .when(chosen, |c| c.bg(color(Token::Sel)))
                .test_id(&id),
        )
        .into_element()
}
