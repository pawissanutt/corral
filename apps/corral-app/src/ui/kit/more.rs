//! The More menu: every operation of a record beside its one next step. A task's lists the
//! fifteen engine operations in the engine's order (`work::record_verbs`), each with a few words
//! on what it does and all enabled, the next step marked; a session's lists its controls
//! (Cancel turn, Pause, Resume, Rewind…, Terminate or Stop, Relaunch), then for a run Reply and
//! its task's menu as a submenu. It opens as a row's context menu (desktop and terminal: the
//! secondary button, Shift+F10 or the Menu key; phone: a held finger opens it as the action
//! sheet), from a pressed control ([`more_menu`]), and from the keyboard (`.`). The work
//! state's overlays the shells host once beside the confirmation are here too ([`overlays`]):
//! the keyboard's menu and the composer an operation that asks for input opens.

use nsh_base::{button as press_target, context_menu, menu, menu_item, submenu};
use nsh_core::element::StyleRefinement;
use nsh_core::prelude::*;

use crate::model::commands::CONFIRM_CANCEL;
use crate::model::projects;
use crate::model::work::{self, Press, Record, VerbAction};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::composer::copyable_input;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::overlay::{Modal, counted, counted_context, modal};
use crate::ui::kit::suggest::suggestions;
use crate::ui::kit::walk::walk;
use crate::ui::kit::{entry, tags};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// Runs any press: local actions through the work reducer, record presses into root effects
/// against the feed.
pub fn press(app: AppState, press: Press) {
    app.press_work(press);
}

/// What marks a row as the record's one next step.
const NEXT_STEP: &str = "next step";

/// The phone's least touch target.
const TARGET: f32 = 44.0;
/// The desktop menu's width range: room for "Remove from queue" and a short reason under it,
/// never a line as wide as the inspector.
const MENU_MIN: f32 = 220.0;
const MENU_MAX: f32 = 340.0;

/// `row` with the verb menu of `record`. The menu's items are made when it opens, from the feed
/// as it stands then, so a row costs nothing until the owner asks for its commands.
pub fn with_menu(app: AppState, record: Record, row: AnyElement) -> AnyElement {
    context_menu()
        .popup_style(popup(app.form))
        .backdrop_style(backdrop)
        .on_open_change(counted_context(app))
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
pub fn more_menu(app: AppState, record: Record, trigger: AnyElement) -> AnyElement {
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
/// decides, and a refusal opens its sheet. A session's More menu holds its controls; a task's
/// holds the fifteen operations with its next step marked.
fn items(app: AppState, record: &Record) -> AnyElement {
    let (verbs, next) = app.feed.with_untracked(|feed| {
        (
            work::record_verbs(record, feed),
            crate::model::next::next_verb(feed, record),
        )
    });
    view()
        .children(
            verbs
                .into_iter()
                .filter(|verb| {
                    crate::model::dock::supported(app.form)
                        || !matches!(verb.press, Press::OpenBeside(_))
                })
                .map(|verb| match &verb.press {
                    Press::Menu(inner) => task_menu(app, inner.clone(), verb.label, verb.slug),
                    _ => {
                        let next = next == Some(verb.slug);
                        item(app, verb, next)
                    }
                })
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// What a row says after its label: that it is the next step, else what its operation does.
fn remark(verb: &VerbAction, next: bool) -> Option<&'static str> {
    if next {
        Some(NEXT_STEP)
    } else {
        work::Verb::from_slug(verb.slug).map(work::Verb::hint)
    }
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

fn item(app: AppState, verb: VerbAction, next: bool) -> AnyElement {
    let form = app.form;
    let remark = remark(&verb, next);
    let VerbAction {
        slug,
        label,
        enabled,
        reason,
        press: action,
        ..
    } = verb;
    let spoken = match (&reason, enabled, remark) {
        (Some(reason), false, _) => format!("{label}, {reason}"),
        (_, _, Some(remark)) => format!("{label}, {remark}"),
        _ => label.clone(),
    };
    // An unavailable verb reads a step quieter, not faded: its reason must stay legible. The
    // next step is marked by weight and its words, never by a colour.
    let words = text(label)
        .number_of_lines(1u32)
        .truncate()
        .when(next, |words| words.font_weight(FontWeight::BOLD))
        .text_color(color(if enabled { Token::Ink } else { Token::Ink3 }));
    let said = reason
        .filter(|_| !enabled)
        .or_else(|| remark.map(str::to_owned));
    let why = said.map(|reason| {
        let why = text(reason)
            .number_of_lines(if form == Form::Terminal { 1u32 } else { 2u32 })
            .truncate()
            .text_color(color(if next { Token::Ink } else { Token::Ink3 }));
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
pub fn overlays(app: AppState) -> AnyElement {
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
/// table's order, the first one focused, so the arrows (or Tab) move, Return runs, Escape
/// closes. test_id "verbs".
fn command_menu(app: AppState) -> Modal {
    let form = app.form;
    let record = app.work.with_untracked(|state| state.menu.clone());
    let (verbs, next) = app.feed.with_untracked(|feed| {
        record.as_ref().map_or_else(
            || (Vec::new(), None),
            |record| {
                (
                    work::record_verbs(record, feed),
                    crate::model::next::next_verb(feed, record),
                )
            },
        )
    });
    let mut walked = Vec::new();
    let rows: Vec<AnyElement> = verbs
        .into_iter()
        .filter(|verb| {
            crate::model::dock::supported(app.form) || !matches!(verb.press, Press::OpenBeside(_))
        })
        .map(|verb| {
            let focus = verb.enabled.then(|| {
                let handle = element_ref();
                walked.push(handle);
                handle
            });
            let label = match &verb.press {
                Press::Menu(_) => format!("{} ›", verb.label),
                _ => verb.label.clone(),
            };
            let next = next == Some(verb.slug);
            command_row(app, VerbAction { label, ..verb }, focus, next)
        })
        .collect();
    if let Some(&first) = walked.first() {
        after_answer(move || first.focus());
    }
    walk(walked);
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
        foot: None,
        done: None,
        confirm: None,
    }
}

/// One verb of the keyboard's menu: its label, and under it (beside it in a terminal) why it
/// cannot run now; choosing it runs it, or opens its composer.
fn command_row(
    app: AppState,
    verb: VerbAction,
    focus: Option<ElementRef>,
    next: bool,
) -> AnyElement {
    let form = app.form;
    let remark = remark(&verb, next);
    let VerbAction {
        slug,
        label,
        enabled,
        reason,
        press: action,
        ..
    } = verb;
    let spoken = match (&reason, enabled, remark) {
        (Some(reason), false, _) => format!("{label}, {reason}"),
        (_, _, Some(remark)) => format!("{label}, {remark}"),
        _ => label.clone(),
    };
    let words = text(label)
        .number_of_lines(1u32)
        .truncate()
        .when(next, |words| words.font_weight(FontWeight::BOLD))
        .text_color(color(if enabled { Token::Ink } else { Token::Ink3 }));
    let said = reason
        .filter(|_| !enabled)
        .or_else(|| remark.map(str::to_owned));
    let why = said.map(|reason| {
        let why = text(reason).text_color(color(if next { Token::Ink } else { Token::Ink3 }));
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
    let tag_index = fields.iter().position(|field| field.tags);
    let tag_draft = tags::TagDraft::new();
    let send = move || {
        if !ready.get_untracked() {
            return;
        }
        let mut submitted = values.get_untracked();
        if let Some(field) = tag_index.and_then(|index| submitted.get_mut(index))
            && !tag_draft.finish(field)
        {
            return;
        }
        press(app, Press::SubmitVerb(submitted));
    };
    let rows: Vec<AnyElement> = fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let caption = entry::caption(form, field.label);
            if !field.choices.is_empty() {
                return choice_row(app, field, index, values, caption);
            }
            if field.tags {
                return tags::tag_row(app, field, index, values, tag_draft, send);
            }
            // A prose field keeps its line breaks: Return breaks the line there, and Tab reaches
            // Send; a one-line field sends on Return.
            let multiline = field.multiline;
            let displayed =
                move || values.with(|values| values.get(index).cloned().unwrap_or_default());
            let entry = copyable_input(app, None, displayed)
                .test_id(&format!("field-verb-{index}"))
                .label(field.label)
                .placeholder(field.placeholder)
                .auto_focus(index == 0)
                .multiline(multiline)
                .submit_behavior(if multiline {
                    SubmitBehavior::Newline
                } else {
                    SubmitBehavior::BlurAndSubmit
                })
                .on_change(move |typed: &str| {
                    values.update(|values| {
                        if let Some(value) = values.get_mut(index) {
                            *value = typed.to_owned();
                        }
                    });
                })
                .on_submit(move |_: &str| send());
            let entry = entry::look(entry, form, multiline);
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
    // Why Send did not go and the buttons stay under the fields, in view however long the form.
    let foot = view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
        .child(error)
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .when(!phone, |row| row.justify_end())
                .child(cancel)
                .child(submit),
        )
        .into_element();
    let body = view()
        .gap(px(match form {
            Form::Terminal => 0.0,
            Form::Desktop => 12.0,
            Form::Phone => 14.0,
        }))
        .when(form != Form::Terminal, |body| body.pt(px(4.0)))
        .children(rows)
        .into_element();
    Modal {
        test_id: "verb".into(),
        title,
        sub: None,
        body,
        foot: Some(foot),
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
