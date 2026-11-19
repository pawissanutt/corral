//! The Files tab's rows (`session::FilesTab`): the total, then one row per changed file (its
//! authors or status letter, `!` when it was edited outside its agent's files, the last two parts
//! of its path, its added and removed lines); a row opens the file in the Changes tab. Why the
//! host could not list the files is said in its tone; past the rail's rows, a link to Changes.
//! test ids `side-file-<n>`, `side-files-more`.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{FilesTab, SideFile};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::rows::list_row;
use crate::ui::kit::text::{dim, mono};

use super::super::super::run;
use super::super::hue_words;
use super::inset;

pub(super) fn files(app: AppState, tab: &FilesTab) -> AnyElement {
    let mut out: Vec<AnyElement> = Vec::new();
    if let Some(total) = &tab.total {
        out.push(inset(app, dim(app, total.clone())));
    }
    if let Some((hue, note)) = &tab.note {
        out.push(inset(
            app,
            dim(app, note.clone()).text_color(hue_words(app.form, *hue)),
        ));
    }
    if let Some(empty) = &tab.empty {
        out.push(inset(app, dim(app, empty.clone())));
    }
    out.extend(tab.rows.iter().map(|row| file(app, row)));
    if let Some((words, action)) = tab.more.clone() {
        out.push(inset(
            app,
            button(
                app,
                words,
                ButtonKind::Quiet,
                "side-files-more",
                move || run(app, action.clone()),
            ),
        ));
    }
    view()
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 2.0 }))
        .pb(px(if app.form == Form::Terminal { 0.0 } else { 8.0 }))
        .children(out)
        .into_element()
}

fn file(app: AppState, row: &SideFile) -> AnyElement {
    let action = row.action.clone();
    let label = format!(
        "{}, {}, {}{}",
        row.path,
        row.lead,
        row.counts,
        if row.outside {
            ", edited outside its agent's files"
        } else {
            ""
        }
    );
    let outside = row.outside.then(|| {
        mono(app, "!")
            .font_weight(FontWeight::BOLD)
            .flex_shrink_0()
            .text_color(hue_ink(Hue::Held))
            .into_element()
    });
    let mut content = vec![
        mono(app, row.lead.clone())
            .number_of_lines(1u32)
            .flex_shrink_0()
            .text_color(hue_words(app.form, Hue::Neutral))
            .into_element(),
    ];
    content.extend(outside);
    content.push(
        mono(app, row.shown.clone())
            .number_of_lines(1u32)
            .truncate()
            .min_w(px(0.0))
            .flex_1()
            .into_element(),
    );
    content.push(
        dim(app, row.counts.clone())
            .number_of_lines(1u32)
            .flex_shrink_0()
            .into_element(),
    );
    list_row(
        app,
        label,
        row.test_id.clone(),
        || false,
        move || run(app, action.clone()),
        content,
    )
}
