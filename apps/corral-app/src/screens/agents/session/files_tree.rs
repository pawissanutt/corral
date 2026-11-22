//! The explorer's tree: the filter, the root's read, then one compact row per entry indented by
//! its depth, a folder led by its chevron and unfolding in place, a file by its kind's mark. The
//! selected row is banded as a list's selection is. test ids `files-tree`,
//! `field-session-files`, `btn-files-filter`, `file-entry-<path>`, `files-note-<n>`.
use nsh_core::prelude::*;

use crate::model::{
    Form, Id,
    session::{Action, FileIcon, FilesAction, FilesView, TreeItem, TreeRow},
};
use crate::state::AppState;
use crate::ui::composer::copyable_input;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::rows::revealing;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::super::super::run;
use super::super::tabs;

/// A row's height on desktop and on the phone, in points; a terminal's row is one Cell high.
const ROW: f32 = 22.0;
const PHONE_ROW: f32 = 36.0;
/// What each level of depth indents: points on desktop and phone, Cells in a terminal.
const INDENT: f32 = 12.0;
const PHONE_INDENT: f32 = 14.0;
const INDENT_CELLS: f32 = 2.0;

/// The tree with what stands above it; `back` returns to the view the explorer was opened over.
pub(super) fn explorer(
    app: AppState,
    shown: Memo<Option<FilesView>>,
    back: Option<(&'static str, Action)>,
) -> AnyElement {
    let form = app.form;
    let filter = computed(move || {
        shown.with(|files| {
            files.as_ref().map(|files| {
                (
                    files.session.clone(),
                    files.filter_in.clone(),
                    files.filter.clone(),
                )
            })
        })
    });
    let part = computed(move || shown.with(|files| files.as_ref().map(|files| files.tree.clone())));
    let head = view()
        .flex_none()
        .flex_col()
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .children(back.map(|(label, action)| {
            view()
                .flex_row()
                .child(button(
                    app,
                    label,
                    ButtonKind::Quiet,
                    "btn-files-back",
                    move || run(app, action.clone()),
                ))
                .into_element()
        }))
        .child(dynamic(move || {
            filter.get().map(|(session, folder, current)| {
                untracked(|| filter_bar(app, session, &folder, current))
            })
        }))
        .child(dynamic(move || {
            part.get()
                .map(|part| untracked(|| tabs::part(app, &part, "files")))
        }));
    let head = match form {
        Form::Terminal => head.px(term::cells(1.0)),
        Form::Desktop => head.px(px(10.0)).pt(px(8.0)).pb(px(6.0)),
        Form::Phone => head.px(px(14.0)).pt(px(10.0)).pb(px(8.0)),
    };
    view()
        .flex_1()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .flex_col()
        .child(head)
        .child(
            scroll()
                .test_id("files-tree")
                .flex_1()
                .min_h(px(0.0))
                .child(rows(app, shown)),
        )
        .into_element()
}

fn filter_bar(app: AppState, session: Id, folder: &str, current: String) -> AnyElement {
    let draft = signal(current);
    let enter = session.clone();
    let height = match app.form {
        Form::Terminal => term::rows(1.0),
        Form::Desktop => px(26.0),
        Form::Phone => px(40.0),
    };
    let label = if folder.is_empty() {
        "Filter names in the worktree's top folder".to_owned()
    } else {
        format!("Filter names in {folder}")
    };
    let search = copyable_input(app, None, move || draft.get())
        .test_id("field-session-files")
        .label(label)
        .placeholder("Filter names in the selected folder")
        .on_change(move |typed: &str| draft.set(typed.to_owned()))
        .on_submit(move |typed: &str| {
            run(
                app,
                Action::Files(FilesAction::Filter {
                    session: enter.clone(),
                    text: typed.to_owned(),
                }),
            )
        })
        .flex_1()
        .min_w(px(0.0))
        .h(height)
        .text_color(color(Token::Ink));
    view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .child(search)
        .child(button(
            app,
            "Apply",
            ButtonKind::Quiet,
            "btn-files-filter",
            move || {
                run(
                    app,
                    Action::Files(FilesAction::Filter {
                        session: session.clone(),
                        text: draft.get_untracked(),
                    }),
                );
            },
        ))
        .into_element()
}

/// The rows, drawn again when the tree changes and not when another row is selected.
fn rows(app: AppState, shown: Memo<Option<FilesView>>) -> AnyElement {
    let items = computed(move || {
        shown.with(|files| {
            files
                .as_ref()
                .map(|files| files.items.clone())
                .unwrap_or_default()
        })
    });
    let selected = computed(move || {
        shown.with(|files| {
            files
                .as_ref()
                .map(|files| files.selected.clone())
                .unwrap_or_default()
        })
    });
    let drawn = dynamic(move || {
        let items = items.get();
        untracked(|| {
            view()
                .flex_col()
                .py(px(if app.form == Form::Terminal { 0.0 } else { 2.0 }))
                .children(
                    items
                        .iter()
                        .enumerate()
                        .map(|(n, item)| match item {
                            TreeItem::Row(row) => entry(app, selected, row),
                            TreeItem::Note {
                                depth,
                                words,
                                retry,
                            } => note(app, n, *depth, words, retry.clone()),
                        })
                        .collect::<Vec<_>>(),
                )
                .into_element()
        })
    });
    view().child(drawn).into_element()
}

/// The leading room `depth` takes past the row's own inset (in a terminal, past its mark's
/// Cell), and `more` beyond it: points, or Cells in a terminal.
fn indent(form: Form, depth: usize, more: f32) -> Points {
    let depth = depth as f32;
    match form {
        Form::Terminal => term::cells(depth * INDENT_CELLS + more),
        Form::Desktop => px(6.0 + depth * INDENT + more),
        Form::Phone => px(10.0 + depth * PHONE_INDENT + more),
    }
}

fn entry(app: AppState, selected: Memo<String>, row: &TreeRow) -> AnyElement {
    let form = app.form;
    let path = row.path.clone();
    let chosen = computed(move || selected.with(|selected| *selected == path));
    let open = row.open;
    let directory = row.directory;
    let press = row.press.clone();
    let name = text(row.name.clone())
        .number_of_lines(1u32)
        .truncate()
        .min_w(px(0.0))
        .text_color(color(Token::Ink));
    let name = match form {
        Form::Terminal if directory => name.font_weight(FontWeight::BOLD),
        Form::Terminal => name,
        Form::Desktop => name.font_family(FONT_TEXT).text_size(px(13.0)),
        Form::Phone => name.font_family(FONT_TEXT).text_size(px(15.0)),
    };
    let label = match (directory, open) {
        (true, true) => format!("Fold folder {}", row.path),
        (true, false) => format!("Unfold folder {}", row.path),
        (false, _) => format!("Open {}", row.path),
    };
    let handle = element_ref();
    let line = view()
        .element_ref(&handle)
        .test_id(&format!("file-entry-{}", row.path))
        .role(AccessibilityRole::Button)
        .label(label)
        .accessibility_state(move || AccessibilityState {
            selected: chosen.get(),
            expanded: directory.then_some(open),
            ..AccessibilityState::default()
        })
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .cursor_pointer()
        .min_w(px(0.0))
        .on_press(move || run(app, press.clone()));
    let line = match form {
        // The leading Cell marks the keyboard focus, else the selection, as a list row does.
        Form::Terminal => {
            let focused = term::focus_of(handle);
            line.h(term::rows(1.0))
                .child(
                    view()
                        .w(term::cells(1.0))
                        .flex_shrink_0()
                        .accessibility_elements_hidden(true)
                        .child(
                            text(move || {
                                if focused.get() {
                                    term::FOCUS_MARK
                                } else if chosen.get() {
                                    "▌"
                                } else {
                                    " "
                                }
                            })
                            .font_weight(FontWeight::BOLD)
                            .text_color(color(Token::Ink)),
                        ),
                )
                .child(view().w(indent(form, row.depth, 0.0)).flex_shrink_0())
                .style_when(chosen, |s| s.bg(color(Token::Sel)))
        }
        _ => line
            .h(px(if form == Form::Phone { PHONE_ROW } else { ROW }))
            .pl(indent(form, row.depth, 0.0))
            .pr(px(8.0))
            .style_when(chosen, |s| s.bg(color(Token::Sel)))
            .hover(|s| s.bg(color(Token::Hover)))
            .focus_visible(|s| {
                s.outline(px(1.0))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(-1.0))
            }),
    };
    let lead = if directory {
        chevron(form, open)
    } else {
        // A file keeps the chevron's column blank, then its kind's mark.
        view()
            .flex_row()
            .flex_shrink_0()
            .items_center()
            .child(chevron_room(form))
            .child(mark(form, row.icon))
            .into_element()
    };
    let line = line
        .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
        .child(lead)
        .child(name);
    revealing(chosen, line.into_element())
}

fn chevron_room(form: Form) -> AnyElement {
    let room = match form {
        Form::Terminal => term::cells(2.0),
        _ => px(14.0),
    };
    view().w(room).flex_shrink_0().into_element()
}

/// A folder's chevron, turned down while it is unfolded; a terminal's `▸` and `▾`.
fn chevron(form: Form, open: bool) -> AnyElement {
    if form == Form::Terminal {
        return view()
            .w(term::cells(2.0))
            .flex_shrink_0()
            .accessibility_elements_hidden(true)
            .child(term::quiet(if open { "▾" } else { "▸" }))
            .into_element();
    }
    view()
        .w(px(14.0))
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .accessibility_elements_hidden(true)
        .child(
            view()
                .rotate(deg(if open { 90.0 } else { 0.0 }))
                .child(icon(Icon::Chevron, 12.0, color(Token::Ink3))),
        )
        .into_element()
}

/// A file kind's mark: a short glyph in the kind's ink, the same in every form.
fn mark(form: Form, kind: FileIcon) -> AnyElement {
    let (glyph, ink) = match kind {
        FileIcon::Folder => ("", Token::Ink3),
        FileIcon::Markdown => ("#", Token::You),
        FileIcon::Html => ("<>", Token::Held),
        FileIcon::Image => ("*", Token::Worker),
        FileIcon::Code => ("{}", Token::Corral),
        FileIcon::Data => ("=", Token::Ok),
        FileIcon::Text => ("·", Token::Ink3),
    };
    let words = text(glyph).text_color(color(ink)).number_of_lines(1u32);
    let (room, words) = match form {
        Form::Terminal => (term::cells(3.0), words),
        _ => (
            px(18.0),
            words
                .font_family(FONT_MONO)
                .text_size(px(10.5))
                .font_weight(FontWeight::BOLD),
        ),
    };
    view()
        .w(room)
        .flex_shrink_0()
        .accessibility_elements_hidden(true)
        .child(words)
        .into_element()
}

fn note(app: AppState, n: usize, depth: usize, words: &str, retry: Option<Action>) -> AnyElement {
    let form = app.form;
    let words = text(words.to_owned())
        .number_of_lines(1u32)
        .truncate()
        .min_w(px(0.0))
        .text_color(color(Token::Ink3));
    let words = match form {
        Form::Terminal => words,
        _ => words.font_family(FONT_TEXT).text_size(px(12.0)),
    };
    let line = view()
        .test_id(&format!("files-note-{n}"))
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .min_w(px(0.0))
        // A note lines up with the names of the folder it stands in, past the chevron's column
        // (and in a terminal the mark's Cell).
        .pl(match form {
            Form::Terminal => indent(form, depth, 3.0),
            _ => indent(form, depth, 18.0),
        })
        .pr(px(8.0))
        .child(words)
        .children(retry.map(|action| {
            button(
                app,
                "Retry",
                ButtonKind::Quiet,
                format!("btn-files-retry-{n}"),
                move || run(app, action.clone()),
            )
        }));
    let line = match form {
        Form::Terminal => line.min_h(term::rows(1.0)),
        _ => line.min_h(px(if form == Form::Phone { PHONE_ROW } else { ROW })),
    };
    line.into_element()
}
