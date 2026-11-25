//! The Sessions page's controls: the toolbar under the head (the search field beside the mode
//! filter) and each section's sort at the trailing edge of its heading.
//!
//! Desktop: one toolbar row, a bordered search field then the modes as a segmented track
//! (`seg-mode-<mode>`), each option its name and a quiet count; a mode with no session is left
//! out unless chosen. A section's sort is a row of small options (`seg-sort-<state>-<key>`), the
//! chosen one raised. Phone: the field over a wrapping row of modes. Terminal: the field and
//! modes on one row; NSH bands a pressed option itself.

use nsh_core::prelude::*;

use crate::model::{Form, sessions};
use crate::state::AppState;
use crate::ui::kit::filters::{Size, option, search_field, track};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The search field (`field-sessions`).
fn field(app: AppState) -> AnyElement {
    search_field(
        app,
        "field-sessions",
        sessions::FILTER_LABEL,
        move || app.agents.with(|state| state.list.filter.clone()),
        move |typed| {
            app.agents
                .update(|state| state.list.filter = typed.to_owned())
        },
    )
}

/// The toolbar: the search field (`field-sessions`) and the mode filter.
pub(super) fn filter_bar(app: AppState, shown: Memo<sessions::Listing>) -> AnyElement {
    let form = app.form;
    let chips = computed(move || shown.with(|shown| shown.chips.clone()));
    let modes = dynamic(move || {
        let chips = chips.get();
        untracked(move || {
            let options = chips
                .into_iter()
                .filter(|chip| chip.mode == "all" || chip.count > 0 || chip.selected)
                .map(|chip| {
                    let mode = chip.mode.clone();
                    let on = chip.selected;
                    option(
                        app,
                        Size::Bar,
                        title_case(&chip.mode),
                        Some(chip.count),
                        move || on,
                        format!("seg-mode-{}", chip.mode),
                        move || app.agents.update(|state| state.list.set_mode(&mode)),
                    )
                });
            track(app, Size::Bar, "Session mode", options.collect())
        })
    });
    let bar = view().flex_none();
    match form {
        Form::Desktop => bar
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(12.0))
            .px(px(16.0))
            .py(px(8.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(field(app))
            .child(modes)
            .into_element(),
        Form::Phone => bar
            .gap(px(10.0))
            .px(px(16.0))
            .pt(px(4.0))
            .pb(px(10.0))
            .child(field(app))
            .child(modes)
            .into_element(),
        // The modes wrap under the field, and onto further rows, where the list is narrow.
        Form::Terminal => bar
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_x(term::cells(2.0))
            .min_h(term::rows(1.0))
            .px(term::cells(1.0))
            .child(field(app))
            .child(modes)
            .into_element(),
    }
}

/// A section's sort, at the trailing edge of its heading.
pub(super) fn sort_bar(app: AppState, group: String) -> AnyElement {
    dynamic(move || {
        let sort = app.agents.with(|state| state.list.sort(&group));
        let group = group.clone();
        untracked(move || {
            let lead = (app.form == Form::Desktop).then(|| {
                let words = text("Sort")
                    .number_of_lines(1u32)
                    .text_color(color(Token::Ink4));
                match app.form {
                    Form::Terminal => words.into_element(),
                    Form::Desktop | Form::Phone => words
                        .font_family(FONT_TEXT)
                        .text_size(px(11.0))
                        .into_element(),
                }
            });
            let options = sessions::Sort::ALL.into_iter().map(|choice| {
                let group = group.clone();
                option(
                    app,
                    Size::Small,
                    if app.form == Form::Desktop {
                        choice.label()
                    } else {
                        choice.short()
                    }
                    .to_owned(),
                    None,
                    move || sort == choice,
                    format!("seg-sort-{group}-{}", choice.slug()),
                    move || {
                        app.agents.update(|state| {
                            state.list.sorts.insert(group.clone(), choice);
                        })
                    },
                )
            });
            view()
                .flex_row()
                .items_center()
                .gap(px(if app.form == Form::Terminal { 0.0 } else { 6.0 }))
                .children(lead)
                .child(track(
                    app,
                    Size::Small,
                    format!("Sort {group}"),
                    options.collect(),
                ))
                .into_element()
        })
    })
    .into_element()
}

/// `build` as `Build`: a mode's name as a control shows it.
fn title_case(word: &str) -> String {
    let mut letters = word.chars();
    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}
