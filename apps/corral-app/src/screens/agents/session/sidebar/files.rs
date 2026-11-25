//! The Changes section's body (`session::FilesTab`), summary first: the added and removed lines
//! (`+` in the Ok ink, `−` in the Fail ink, so the sign says it without the colour) beside the
//! link to the full Changes view; why the host could not list the files in its tone; the
//! conflicts and edits outside an agent's files across every file, in their tone; then the
//! fold of the short preview. Unfolded, a row per file: its state mark, `!` when it was edited
//! outside its agent's files, its name, its folder (quiet) and its lines; a row opens the file's
//! diff. A preview that does not list every file ends in a link to the rest.
//! test ids `side-files-all`, `side-files-preview`, `side-file-<n>`, `side-files-more`.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{Action, FilesTab, SideFile};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::rows::list_row;
use crate::ui::kit::text::{dim, mono};
use crate::ui::tokens::{Token, color};

use super::super::super::run;
use super::super::hue_words;
use super::inset;

fn link(
    app: AppState,
    words: &str,
    kind: ButtonKind,
    id: &'static str,
    action: &Action,
) -> AnyElement {
    let action = action.clone();
    button(app, words.to_owned(), kind, id, move || {
        run(app, action.clone())
    })
    .into_element()
}

/// `+340 −22`.
fn lines(app: AppState, added: &str, removed: &str) -> AnyElement {
    view()
        .flex_row()
        .flex_shrink_0()
        .gap(px(if app.form == Form::Terminal { 8.0 } else { 4.0 }))
        .child(mono(app, added.to_owned()).text_color(hue_ink(Hue::Ok)))
        .child(mono(app, removed.to_owned()).text_color(hue_ink(Hue::Fail)))
        .into_element()
}

pub(super) fn files(app: AppState, tab: &FilesTab) -> AnyElement {
    let mut out: Vec<AnyElement> = Vec::new();
    if tab.totals.is_some() || tab.all.is_some() {
        let summary = view()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .children(
                tab.totals
                    .as_ref()
                    .map(|(added, removed)| lines(app, added, removed)),
            )
            .child(view().flex_1().min_w(px(0.0)))
            .children(tab.all.as_ref().map(|(words, action)| {
                link(app, words, ButtonKind::Ghost, "side-files-all", action)
            }));
        out.push(inset(app, summary));
    }
    if let Some((hue, note)) = &tab.note {
        out.push(inset(
            app,
            dim(app, note.clone()).text_color(hue_words(app.form, *hue)),
        ));
    }
    if let Some((hue, cues)) = &tab.cues {
        out.push(inset(
            app,
            dim(app, cues.clone())
                .number_of_lines(2u32)
                .text_color(hue_words(app.form, *hue)),
        ));
    }
    if let Some(empty) = &tab.empty {
        out.push(inset(app, dim(app, empty.clone())));
    }
    if let Some((words, action)) = &tab.preview {
        out.push(inset(
            app,
            view().flex_row().child(link(
                app,
                words,
                ButtonKind::Quiet,
                "side-files-preview",
                action,
            )),
        ));
    }
    out.extend(tab.rows.iter().map(|row| file(app, row)));
    if let Some((words, action)) = &tab.more {
        out.push(inset(
            app,
            view().flex_row().child(link(
                app,
                words,
                ButtonKind::Quiet,
                "side-files-more",
                action,
            )),
        ));
    }
    view()
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 2.0 }))
        .pb(px(if app.form == Form::Terminal { 0.0 } else { 8.0 }))
        .children(out)
        .into_element()
}

/// One file: the name leads and keeps its room; the folder takes what is left.
fn file(app: AppState, row: &SideFile) -> AnyElement {
    let action = row.action.clone();
    let mut content = vec![
        mono(app, row.mark)
            .flex_shrink_0()
            .text_color(hue_words(app.form, row.mark_hue))
            .into_element(),
    ];
    content.extend(row.outside.then(|| {
        mono(app, "!")
            .font_weight(FontWeight::BOLD)
            .flex_shrink_0()
            .text_color(hue_ink(Hue::Held))
            .into_element()
    }));
    content.push(
        mono(app, row.name.clone())
            .number_of_lines(1u32)
            .truncate()
            .min_w(px(0.0))
            .text_color(color(Token::Ink))
            .into_element(),
    );
    content.push(
        dim(app, row.folder.clone())
            .number_of_lines(1u32)
            .truncate()
            .min_w(px(0.0))
            .flex_1()
            .text_color(color(Token::Ink3))
            .into_element(),
    );
    content.push(match (&row.added, &row.removed, &row.other) {
        (Some(added), Some(removed), _) => lines(app, added, removed),
        (_, _, other) => dim(app, other.clone().unwrap_or_default())
            .flex_shrink_0()
            .into_element(),
    });
    list_row(
        app,
        row.label.clone(),
        row.test_id.clone(),
        || false,
        move || run(app, action.clone()),
        content,
    )
}
