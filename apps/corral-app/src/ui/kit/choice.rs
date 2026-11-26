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
//!
//! The menu's rows are keyed (each value by itself, the foot by its place), so a page arriving
//! adds its branches without making the others again: the keyboard focus stays on the row it
//! was on. More branches, Loading, Retry and First branches at the end are one row that reads
//! each in turn, so the focus a press of More left there stays through the page it reads, and
//! Up goes on from it to the last branch read.

use std::rc::Rc;

use corral_protocol::detail::{DetailGet, DetailRef};
use nsh_base::{MenuEvent, menu, menu_item};
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
/// First branches' words: back to the first page.
pub const FIRST: &str = "First branches";

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
    // Git branches can change without a project row changing (especially beyond its top 100).
    // Revalidate a retained list each time this form enters a project.
    effect(move || {
        if let Some(target) = target.get() {
            untracked(|| {
                if app
                    .feed
                    .with_untracked(|feed| feed.detail(&target).is_some())
                {
                    app.refresh(target);
                }
            });
        }
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
    let count = counted(app);
    let picker = menu()
        .popup_style(popup(form))
        .backdrop_style(backdrop)
        .on_open_change(move |event: &MenuEvent| {
            count(event);
            let opened = matches!(event, MenuEvent::OpenChange(true, _));
            // A press on the trigger leaves the form's text field (the title takes the focus as
            // the form opens) editing, and an editing field keeps the keys: End and Up moved its
            // caret rather than the menu's focus, and Return sent the form. Opening ends the
            // edit, as a press on a desktop's control takes the focus from the field, so the
            // menu's keys reach its rows. A menu the keyboard opened from a focused trigger has
            // no field editing; the focus on its row stays.
            if opened && form != Form::Phone {
                dismiss_keyboard();
            }
            if opened && let Some((target, _)) = paging.and_then(|paging| paging.get_untracked()) {
                app.detail(DetailGet {
                    target,
                    etag: None,
                    page: None,
                });
            }
        })
        .trigger(menu_trigger_reactive(
            app,
            move || shown().into(),
            move || format!("{label}: {}", shown()).into(),
            &format!("field-verb-{index}"),
        ))
        .child(rows(app, &group, offered, paging, values, index));
    view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .child(caption)
        // In a row, so the trigger keeps the width of its words rather than the form's.
        .child(view().flex_row().child(picker.into_element()))
        .into_element()
}

/// The menu's rows: the offered values, then (for a field that pages) the foot. Both are keyed,
/// so a page arriving, a project's branches read again or a value chosen makes only the rows
/// that came, and the one the keyboard focus is on stays with it.
fn rows(
    app: AppState,
    group: &str,
    offered: Memo<Vec<Choice>>,
    paging: Option<Paging>,
    values: Signal<Vec<String>>,
    index: usize,
) -> AnyElement {
    let items = {
        let group = group.to_owned();
        each(
            move || offered.get(),
            |choice: &Choice| choice.value.clone(),
            move |choice: ReadSignal<Choice>, _| item(app, &group, choice, values, index),
        )
    };
    let tail = paging.map(|paging| {
        let group = group.to_owned();
        each(
            move || {
                paging
                    .get()
                    .map_or_else(Vec::new, |(target, page)| foot(target, page))
            },
            Foot::slot,
            move |part: ReadSignal<Foot>, _| foot_row(app, &group, part),
        )
    });
    view().child(items).children(tail).into_element()
}

/// A part of what follows the branches served so far, known by its place in the foot.
#[derive(Clone, PartialEq)]
enum Foot {
    /// Why the branches could not all be read; those already read stay offered.
    Why(String),
    /// The row that pages, as it reads now.
    Lead(DetailRef, Lead),
    /// First branches, under More.
    First(DetailRef),
}

/// What the foot's paging row does: one menu row through all of them, so the keyboard focus on
/// it stays as a page is asked for, read, or fails.
#[derive(Clone, PartialEq)]
enum Lead {
    /// More branches: reads the page corrald named next.
    More(String),
    /// A page on its way; pressing it does nothing.
    Loading,
    /// A page failed: reads it again.
    Retry,
    /// Every page read: back to the first.
    First,
}

impl Foot {
    /// Its place in the foot, which keys its row.
    fn slot(&self) -> &'static str {
        match self {
            Self::Why(_) => "why",
            Self::Lead(..) => "lead",
            Self::First(_) => "first",
        }
    }

    /// The branches it reads, and how (its paging row's), if it acts.
    fn act(&self) -> Option<(DetailRef, Lead)> {
        match self {
            Self::Why(_) => None,
            Self::Lead(target, lead) => Some((target.clone(), lead.clone())),
            Self::First(target) => Some((target.clone(), Lead::First)),
        }
    }
}

impl Lead {
    fn words(&self) -> &'static str {
        match self {
            Self::More(_) => MORE,
            Self::Loading => LOADING,
            Self::Retry => RETRY,
            Self::First => FIRST,
        }
    }

    fn id(&self, group: &str) -> String {
        match self {
            Self::More(_) => format!("btn-{group}-more"),
            Self::Loading => format!("{group}-loading"),
            Self::Retry => format!("btn-{group}-retry"),
            Self::First => format!("btn-{group}-first"),
        }
    }

    /// Reads what it names of `target`'s branches.
    fn run(self, app: AppState, target: DetailRef) {
        match self {
            Self::More(next) => {
                app.detail(DetailGet {
                    target,
                    etag: None,
                    page: Some(next),
                });
            }
            Self::Loading => {}
            Self::Retry => {
                app.refresh(target);
            }
            Self::First => {
                app.detail(DetailGet {
                    target,
                    etag: None,
                    page: None,
                });
            }
        }
    }
}

/// What follows the branches served so far: why they could not all be read with Retry, More
/// branches (and First branches) while corrald names a further page, Loading while one is on
/// its way, First branches once all are read.
fn foot(target: DetailRef, page: BranchPage) -> Vec<Foot> {
    if let Some(error) = page.error {
        return vec![Foot::Why(error), Foot::Lead(target, Lead::Retry)];
    }
    if let Some(next) = page.next {
        return vec![
            Foot::Lead(target.clone(), Lead::More(next)),
            Foot::First(target),
        ];
    }
    if page.loading {
        return vec![Foot::Lead(target, Lead::Loading)];
    }
    if !page.branches.is_empty() {
        return vec![Foot::Lead(target, Lead::First)];
    }
    Vec::new()
}

/// One part of the foot, made once for its place and reading `part` as it changes.
fn foot_row(app: AppState, group: &str, part: ReadSignal<Foot>) -> AnyElement {
    let form = app.form;
    let run = move || {
        if let Some((target, lead)) = part.with_untracked(Foot::act) {
            lead.run(app, target);
        }
    };
    match part.with_untracked(Foot::slot) {
        "why" => {
            let id = format!("{group}-error");
            dynamic(move || {
                let error = part.with(|part| match part {
                    Foot::Why(error) => error.clone(),
                    _ => String::new(),
                });
                untracked(|| {
                    let why = format!("Branches unavailable: {error}");
                    content(form, words(form, why, Token::Ink2))
                        .border_t_1()
                        .border_color(color(Token::Rule))
                        .test_id(&id)
                        .into_element()
                })
            })
        }
        "lead" => {
            let group = group.to_owned();
            action(
                app,
                move || {
                    let lead = part.with(|part| match part {
                        Foot::Lead(_, lead) => lead.clone(),
                        _ => Lead::Loading,
                    });
                    // After a failure the note above carries the rule.
                    let ruled = lead != Lead::Retry;
                    (lead.words(), lead.id(&group), ruled)
                },
                run,
            )
        }
        _ => {
            let id = Lead::First.id(group);
            action(app, move || (FIRST, id.clone(), false), run)
        }
    }
}

/// A row of the foot that acts rather than chooses: it keeps the menu open, so the branches it
/// brings are there to choose from. One menu item while `look` (its words, test id and whether
/// a rule tops it) changes, so the keyboard focus on it stays.
fn action(
    app: AppState,
    look: impl Fn() -> (&'static str, String, bool) + 'static,
    run: impl Fn() + 'static,
) -> AnyElement {
    let form = app.form;
    let look = Rc::new(look);
    let said = look.clone();
    menu_item()
        .close_on_select(false)
        .accessibility_label(move || SharedString::from(said().0))
        .on_select(run)
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .focus(|s| s.bg(color(Token::Hover)))
        .child(dynamic(move || {
            let (label, id, ruled) = look();
            untracked(|| {
                let row = content(form, words(form, label.to_owned(), Token::Ink));
                let row = if ruled {
                    row.border_t_1().border_color(color(Token::Rule))
                } else {
                    row
                };
                row.test_id(&id).into_element()
            })
        }))
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

/// One offered value: its words, bold on a selected ground while it is the chosen one. Made
/// once for its value (`choice` keyed by it), reading its words and whether it is chosen as
/// they change, so the keyboard focus on it stays.
fn item(
    app: AppState,
    group: &str,
    choice: ReadSignal<Choice>,
    values: Signal<Vec<String>>,
    index: usize,
) -> AnyElement {
    let form = app.form;
    let value = choice.with_untracked(|choice| choice.value.clone());
    let id = format!(
        "seg-{group}-{}",
        if value.is_empty() {
            "none"
        } else {
            value.as_str()
        }
    );
    let chosen = {
        let value = value.clone();
        computed(move || values.with(|values| values.get(index) == Some(&value)))
    };
    let label = move || choice.with(|choice| choice.label.clone());
    menu_item()
        .accessibility_label(move || {
            let label = label();
            SharedString::from(if chosen.get() {
                format!("{label}, chosen")
            } else {
                label
            })
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
        .child(dynamic(move || {
            let (label, on) = (label(), chosen.get());
            untracked(|| {
                let words =
                    words(form, label, Token::Ink).when(on, |t| t.font_weight(FontWeight::BOLD));
                content(form, words)
                    .when(on, |c| c.bg(color(Token::Sel)))
                    .test_id(&id)
                    .into_element()
            })
        }))
        .into_element()
}
