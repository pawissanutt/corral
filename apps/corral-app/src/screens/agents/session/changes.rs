//! The Changes tab (`session::ChangesTab`): what the session changed, as its host serves it.
//! On desktop, over the content, fixed: the summary and where the read stands, the comparison
//! switch (one the host cannot serve muted, its reason said under the switch), the compared
//! commits, the repository and the totals, the repositories to read, then the path filter and
//! the author chips. A terminal and a phone fold that head to one line (the summary, the
//! comparison, what narrows the list, when it was read) with `⋯ More`, which unfolds the rest
//! as the list's first rows, so the files keep their rows on an 80 × 24 terminal; what keeps the
//! list from showing (unavailable, a failed read, the host's error) stays under the line. Under
//! the head the file list and the opened file's diff (`changes_diff.rs`): beside each other
//! from 900 pt of tab width on desktop (the list 320 pt) and 100 Cells in a terminal (36
//! Cells); narrower, the list above the diff, cut to five rows while a file is open (a
//! terminal gives way down to two, so the diff keeps six); the phone shows one at a time, the
//! diff with a way back to the files. test ids `session-changes`, `btn-changes-more`,
//! `changes-more`, `seg-compare-<slug>`, `seg-scope-<n>`, `seg-author-<id>`,
//! `field-changes-filter`, `change-file-<n>`.

#[path = "changes_diff.rs"]
mod diff;
#[path = "changes_head.rs"]
mod head;

use nsh_core::EllipsizeMode;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::{Body, ChangesTab, FileRow, SessionPage};
use crate::screens::now::list::revealing;
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::rows::{list_row, list_row_tall};
use crate::ui::kit::text::{dim, mono};
use crate::ui::term;
use crate::ui::tokens::{Token, color};

use super::super::run;
use head::{extras, folded, head};

/// Tab widths from which the list stands beside the diff: points, and Cells in a terminal.
const BESIDE: f32 = 900.0;
const BESIDE_CELLS: f32 = 100.0;
/// The list column beside the diff.
const LIST: f32 = 320.0;
const LIST_CELLS: f32 = 36.0;
/// The rows the list keeps over an opened diff when it stands above it.
const LIST_ROWS: f32 = 5.0;
/// A terminal's rows the tab spends besides the list and the diff: the folded head's line and
/// the two rules.
const FIXED_ROWS: f32 = 3.0;
/// The least rows a terminal keeps for the diff under the list: its path and walk lines, then
/// a hunk's header and three of its rows.
const DIFF_ROWS: f32 = 6.0;
const ROW: f32 = 30.0;

pub(super) fn tab(app: AppState, page: Memo<SessionPage>) -> AnyElement {
    let form = app.form;
    let shown = computed(move || {
        page.with(|p| match &p.body {
            Body::Changes(view) => Some(view.clone()),
            _ => None,
        })
    });
    let open =
        computed(move || shown.with(|tab| tab.as_ref().is_some_and(|t| t.selected.is_some())));
    let room = signal(None::<(f32, f32)>);
    let beside = computed(move || {
        let width = room.get().map(|(width, _)| width);
        match form {
            Form::Phone => false,
            Form::Desktop => width.is_some_and(|width| width >= BESIDE),
            Form::Terminal => width.is_some_and(|width| width >= term::cells(BESIDE_CELLS).0),
        }
    });
    // A terminal keeps the diff at least DIFF_ROWS rows under the list: the list gives way
    // first, down to two rows.
    let kept = computed(move || match (form, room.get()) {
        (Form::Terminal, Some((_, height))) => {
            let rows = (height / term::rows(1.0).0).floor();
            (rows - FIXED_ROWS - DIFF_ROWS).clamp(2.0, LIST_ROWS)
        }
        _ => LIST_ROWS,
    });
    let content = dynamic(move || {
        let (beside, open, kept) = (beside.get(), open.get(), kept.get());
        untracked(|| body(app, shown, beside, open, kept))
    });
    let root = view()
        .test_id("session-changes")
        .flex_1()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .flex_col()
        .on_layout(move |frame: &Bounds<Points>| {
            let size = Some((frame.size.width.0, frame.size.height.0));
            if room.get_untracked() != size {
                room.set(size);
            }
        });
    match form {
        // The phone scrolls the whole tab: its folded head, then the list or the diff.
        Form::Phone => root
            .child(
                scroll().flex_1().min_h(px(0.0)).child(
                    view()
                        .px(px(16.0))
                        .py(px(10.0))
                        .gap(px(12.0))
                        .child(folded(app, shown))
                        .child(content),
                ),
            )
            .into_element(),
        Form::Desktop => root
            .child(
                view()
                    .px(px(18.0))
                    .pt(px(12.0))
                    .pb(px(10.0))
                    .child(head(app, shown)),
            )
            .child(content)
            .into_element(),
        Form::Terminal => root
            .child(view().px(term::cells(1.0)).child(folded(app, shown)))
            .child(content)
            .into_element(),
    }
}

/// The list and the diff, laid out for the form and the room.
fn body(
    app: AppState,
    shown: Memo<Option<ChangesTab>>,
    beside: bool,
    open: bool,
    kept: f32,
) -> AnyElement {
    let form = app.form;
    if form == Form::Phone {
        return if open {
            view()
                .child(unfolded(app, shown))
                .child(diff::diff(app, shown))
                .into_element()
        } else {
            list_content(app, shown)
        };
    }
    let rule = |side: bool| -> AnyElement {
        match (form, side) {
            (Form::Terminal, true) => term::vrule(),
            (Form::Terminal, false) => term::hrule(),
            (_, true) => view()
                .w(px(1.0))
                .flex_none()
                .bg(color(Token::Rule))
                .into_element(),
            (_, false) => view()
                .h(px(1.0))
                .flex_none()
                .bg(color(Token::Rule))
                .into_element(),
        }
    };
    let inset = |content: AnyElement| -> AnyElement {
        match form {
            Form::Terminal => view().px(term::cells(1.0)).child(content).into_element(),
            _ => view()
                .px(px(18.0))
                .py(px(10.0))
                .child(content)
                .into_element(),
        }
    };
    let list = scroll()
        .test_id("changes-files")
        .min_h(px(0.0))
        .child(inset(list_content(app, shown)));
    let patch = || {
        scroll()
            .test_id("changes-diff")
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .child(inset(diff::diff(app, shown)))
    };
    if beside && open {
        let list = match form {
            Form::Terminal => list.w(term::cells(LIST_CELLS)),
            _ => list.w(px(LIST)),
        };
        return view()
            .flex_1()
            .min_h(px(0.0))
            .flex_row()
            .border_t(px(if form == Form::Terminal { 0.0 } else { 1.0 }))
            .border_color(color(Token::Rule))
            .child(list.flex_none())
            .child(rule(true))
            .child(patch())
            .into_element();
    }
    if !open {
        return view()
            .flex_1()
            .min_h(px(0.0))
            .flex_col()
            .child(rule(false))
            .child(list.flex_1())
            .into_element();
    }
    let kept = match form {
        Form::Terminal => term::rows(kept),
        _ => px(kept * ROW + 20.0),
    };
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .child(rule(false))
        .child(list.flex_none().max_h(kept))
        .child(rule(false))
        .child(patch())
        .into_element()
}

/// What the list's scroll holds: where the head folds, the unfolded head as its first rows,
/// then the files, then the bound's notice; on desktop the files alone.
fn list_content(app: AppState, shown: Memo<Option<ChangesTab>>) -> AnyElement {
    if app.form == Form::Desktop {
        return files(app, shown);
    }
    let bound = computed(move || shown.with(|tab| tab.as_ref().and_then(|tab| tab.bound.clone())));
    let bound = dynamic(move || {
        bound
            .get()
            .map(|words| untracked(|| dim(app, words).into_element()))
    });
    view()
        .child(unfolded(app, shown))
        .child(files(app, shown))
        .child(bound)
        .into_element()
}

/// What `⋯ More` unfolds of a folded head, while it is unfolded.
fn unfolded(app: AppState, shown: Memo<Option<ChangesTab>>) -> AnyElement {
    let form = app.form;
    let more = computed(move || shown.with(|tab| tab.as_ref().is_some_and(|tab| tab.more)));
    dynamic(move || {
        more.get().then(|| {
            untracked(|| {
                view()
                    .test_id("changes-more")
                    .pb(if form == Form::Terminal {
                        term::rows(1.0)
                    } else {
                        px(10.0)
                    })
                    .child(extras(app, shown, false))
                    .into_element()
            })
        })
    })
}

/// The file list, or what stands for it.
fn files(app: AppState, shown: Memo<Option<ChangesTab>>) -> AnyElement {
    // The rows redraw when the list changes, not when another one is selected.
    let rows = computed(move || {
        shown.with(|tab| {
            tab.as_ref()
                .map(|tab| {
                    tab.files
                        .iter()
                        .map(|row| FileRow {
                            selected: false,
                            ..row.clone()
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    let empty = computed(move || shown.with(|tab| tab.as_ref().and_then(|tab| tab.empty.clone())));
    let list = dynamic(move || {
        let rows = rows.get();
        untracked(|| {
            view()
                .children(
                    rows.iter()
                        .map(|row| file(app, shown, row))
                        .collect::<Vec<_>>(),
                )
                .into_element()
        })
    });
    let empty = dynamic(move || {
        empty
            .get()
            .map(|words| untracked(|| dim(app, words).into_element()))
    });
    view().child(list).child(empty).into_element()
}

/// One file: its status, path (cut in the middle), authors, flag and counts. A flag or a rename
/// takes a second line on desktop and phone; a terminal keeps one row.
fn file(app: AppState, shown: Memo<Option<ChangesTab>>, row: &FileRow) -> AnyElement {
    let form = app.form;
    let path = row.path.clone();
    let selected = computed(move || {
        shown.with(|tab| {
            tab.as_ref()
                .is_some_and(|tab| tab.files.iter().any(|one| one.selected && one.path == path))
        })
    });
    let status = mono(app, row.status.letter)
        .text_color(if row.status.letter == "!" {
            hue_ink(Hue::Fail)
        } else {
            color(Token::Ink3)
        })
        .flex_shrink_0();
    let name = mono(app, row.path.clone())
        .number_of_lines(1u32)
        .text_overflow(EllipsizeMode::Middle)
        .min_w(px(0.0));
    let mut second = Vec::new();
    if let Some(from) = &row.from {
        second.push(
            dim(app, format!("from {from}"))
                .number_of_lines(1u32)
                .into_element(),
        );
    }
    if let Some(flag) = &row.flag
        && form != Form::Terminal
    {
        second.push(
            // The row's own `!` mark leads it already.
            dim(app, flag.trim_start_matches("! ").to_owned())
                .text_color(hue_ink(Hue::Held))
                .number_of_lines(1u32)
                .into_element(),
        );
    }
    let tall = !second.is_empty();
    let title = if tall {
        view()
            .flex_1()
            .min_w(px(0.0))
            .gap(px(1.0))
            .child(name)
            .child(
                view()
                    .flex_row()
                    .gap(px(8.0))
                    .min_w(px(0.0))
                    .children(second),
            )
            .into_element()
    } else {
        view().flex_1().min_w(px(0.0)).child(name).into_element()
    };
    let authors = (!row.authors.is_empty()).then(|| {
        dim(app, row.authors.join(" "))
            .number_of_lines(1u32)
            .flex_shrink_0()
            .into_element()
    });
    let mark = row.flag.as_ref().map(|flag| {
        let words = if form == Form::Terminal {
            flag.clone()
        } else {
            "!".to_owned()
        };
        text(words)
            .number_of_lines(1u32)
            .text_color(hue_ink(Hue::Held))
            .font_weight(FontWeight::BOLD)
            .flex_shrink_0()
            .into_element()
    });
    let counts = counts(app, row);
    let content = vec![status.into_element(), title]
        .into_iter()
        .chain(authors)
        .chain(mark)
        .chain(std::iter::once(counts))
        .collect::<Vec<_>>();
    let open = row.open.clone();
    let press = move || run(app, open.clone());
    let shape = if tall && form != Form::Terminal {
        list_row_tall(
            app,
            row.label.clone(),
            row.id.clone(),
            move || selected.get(),
            press,
            content,
        )
    } else {
        list_row(
            app,
            row.label.clone(),
            row.id.clone(),
            move || selected.get(),
            press,
            content,
        )
    };
    revealing(selected, shape)
}

/// `+12 −3` in the Ok and Fail inks, `bin` when git counts no lines.
fn counts(app: AppState, row: &FileRow) -> AnyElement {
    let figure =
        |words: String, ink: AdaptiveColor| mono(app, words).text_color(ink).flex_shrink_0();
    if row.binary {
        return figure("bin".into(), color(Token::Ink3)).into_element();
    }
    view()
        .flex_row()
        .flex_shrink_0()
        .gap(px(if app.form == Form::Terminal { 8.0 } else { 4.0 }))
        .child(figure(
            format!("+{}", row.added.unwrap_or(0)),
            hue_ink(Hue::Ok),
        ))
        .child(figure(
            format!("\u{2212}{}", row.removed.unwrap_or(0)),
            hue_ink(Hue::Fail),
        ))
        .into_element()
}
