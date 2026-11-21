//! The opened file's diff in the Changes tab (`session::FileDiff`): its path and where its read
//! stands, its status, rename, authors and the current hunk's `i/n`, the walk (hunks with `p`
//! and `n`, files with `k` and `j`), Add file to message, the newer patch waiting behind
//! Apply, the bound's notice, and the hunks: a header with Add to message, then the rows as
//! the Review screen draws them. The current hunk is framed in ink with its header bold, and
//! scrolled into view as the walk reaches it. Drawn again only when another file opens; the
//! walk, the read and a newer patch update in place, so a focused button keeps the focus.
//! test ids `btn-changes-<files|prev-hunk|next-hunk|prev-file|next-file|add-file|apply>`,
//! `hunk-<n>`, `btn-capture-hunk-<n>`.

use nsh_core::EllipsizeMode;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::review::NumberedLine;
use crate::model::session::{Action, ChangesTab, FileDiff, HunkView};
use crate::model::text::span;
use crate::screens::task::candidate::diff::unified_line;
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::notice::notice;
use crate::ui::kit::rows::revealing;
use crate::ui::kit::text::{dim, mono};
use crate::ui::tokens::{FONT_MONO, Token, color};

use super::super::super::run;
use super::super::tabs::part;

/// One hunk as it is drawn; whether it is current is followed apart.
#[derive(Clone, PartialEq)]
struct Shape {
    header: String,
    lines: Vec<NumberedLine>,
}

pub(super) fn diff(app: AppState, shown: Memo<Option<ChangesTab>>) -> AnyElement {
    let opened =
        computed(move || shown.with(|tab| tab.as_ref().and_then(|tab| tab.selected.clone())));
    let path = computed(move || opened.with(|diff| diff.as_ref().map(|diff| diff.path.clone())));
    dynamic(move || path.get().map(|_| untracked(|| file(app, opened))))
}

/// The press of an action the diff names now, read when pressed.
fn press(
    app: AppState,
    opened: Memo<Option<FileDiff>>,
    pick: impl Fn(&FileDiff) -> Option<Action> + 'static,
) -> impl Fn() + 'static {
    move || {
        if let Some(action) = opened.with_untracked(|diff| diff.as_ref().and_then(&pick)) {
            run(app, action);
        }
    }
}

fn walk(
    app: AppState,
    opened: Memo<Option<FileDiff>>,
    label: &'static str,
    key: &'static str,
    slug: &str,
    pick: impl Fn(&FileDiff) -> Option<Action> + 'static,
) -> AnyElement {
    button_with(
        app,
        label,
        ButtonKind::Quiet,
        ButtonOptions {
            // A terminal's key line names the walk keys; the caps would push the walk past
            // one row at 80 columns.
            key_hint: (app.form != Form::Terminal).then_some(key),
            ..ButtonOptions::default()
        },
        format!("btn-changes-{slug}"),
        press(app, opened, pick),
    )
}

fn file(app: AppState, opened: Memo<Option<FileDiff>>) -> AnyElement {
    let form = app.form;
    // A terminal has few rows: the path, its facts and when it was read share one line, and
    // the walk takes the next, so the hunks start on the third.
    let terminal = form == Form::Terminal;
    let body = computed(move || opened.with(|diff| diff.as_ref().map(|diff| diff.part.clone())));
    let body = dynamic(move || {
        body.with(|body| {
            body.as_ref().map(|body| {
                untracked(|| {
                    if terminal && body.unavailable.is_none() && body.read.has_rows() {
                        text(body.summary.clone())
                            .test_id("part-changes-file-summary")
                            .number_of_lines(1u32)
                            .text_overflow(EllipsizeMode::Middle)
                            .min_w(px(0.0))
                            .flex_shrink(1.0)
                            .font_weight(FontWeight::BOLD)
                            .text_color(color(Token::Ink))
                            .into_element()
                    } else {
                        part(app, body, "changes-file")
                    }
                })
            })
        })
    });
    let facts = text(move || {
        opened.with(|diff| {
            let Some(diff) = diff else {
                return String::new();
            };
            let mut facts = vec![diff.status.to_owned()];
            facts.extend(diff.from.as_ref().map(|from| format!("from {from}")));
            if !diff.authors.is_empty() {
                facts.push(format!("by {}", diff.authors.join(", ")));
            }
            facts.extend(diff.position.as_ref().map(|at| format!("hunk {at}")));
            if terminal && diff.part.read.has_rows() {
                facts.push(diff.part.read.words());
            }
            facts.join(" · ")
        })
    })
    .number_of_lines(1u32)
    .text_color(color(Token::Ink3));
    let facts = match form {
        Form::Terminal => facts,
        _ => facts
            .font_family(crate::ui::tokens::FONT_TEXT)
            .text_size(px(12.0)),
    };
    let back = button(
        app,
        if form == Form::Phone {
            "‹ Files"
        } else {
            "Close"
        },
        ButtonKind::Quiet,
        "btn-changes-files",
        press(app, opened, |diff| Some(diff.close.clone())),
    );
    let preview = dynamic(move || {
        opened
            .with(|diff| diff.as_ref()?.open_worktree_file.clone())
            .map(|action| {
                button(
                    app,
                    "View file",
                    ButtonKind::Quiet,
                    "btn-open-worktree-file",
                    move || run(app, action.clone()),
                )
            })
    });
    let walks = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(if terminal { 8.0 } else { 4.0 }))
        .child(preview)
        .child(walk(app, opened, "‹ Hunk", "p", "prev-hunk", |diff| {
            diff.prev_hunk.clone()
        }))
        .child(walk(app, opened, "Hunk ›", "n", "next-hunk", |diff| {
            diff.next_hunk.clone()
        }))
        .child(walk(app, opened, "↑ File", "k", "prev-file", |diff| {
            diff.prev_file.clone()
        }))
        .child(walk(app, opened, "↓ File", "j", "next-file", |diff| {
            diff.next_file.clone()
        }))
        .child(button(
            app,
            if terminal {
                "Add file"
            } else {
                "Add file to message"
            },
            ButtonKind::Ghost,
            "btn-changes-add-file",
            press(app, opened, |diff| Some(diff.capture_file.clone())),
        ))
        .child(view().flex_1())
        .child(back);
    let newer = computed(move || {
        opened.with(|diff| diff.as_ref().is_some_and(|diff| diff.newer.is_some()))
    });
    let newer = dynamic(move || {
        newer.get().then(|| {
            untracked(|| {
                let apply = button_with(
                    app,
                    "Apply",
                    ButtonKind::Ghost,
                    ButtonOptions {
                        key_hint: Some("r"),
                        ..ButtonOptions::default()
                    },
                    "btn-changes-apply",
                    press(app, opened, |diff| diff.newer.clone()),
                );
                notice(
                    app,
                    Hue::Neutral,
                    &vec![span(
                        "New diff available; the hunks shown are the ones read before",
                    )],
                    Some(apply),
                )
            })
        })
    });
    let said = computed(move || {
        opened.with(|diff| {
            diff.as_ref()
                .map(|diff| {
                    (
                        diff.truncated.clone(),
                        diff.notes.clone(),
                        diff.empty.clone(),
                    )
                })
                .unwrap_or_default()
        })
    });
    let said = dynamic(move || {
        let (truncated, notes, empty) = said.get();
        untracked(|| {
            view()
                .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
                .children(truncated.map(|words| notice(app, Hue::Held, &vec![span(words)], None)))
                .children(notes.into_iter().map(|note| {
                    mono(app, note)
                        .text_color(color(Token::Ink3))
                        .into_element()
                }))
                .children(empty.map(|words| dim(app, words).into_element()))
                .into_element()
        })
    });
    let head = if terminal {
        view()
            .flex_row()
            .items_center()
            .min_w(px(0.0))
            .gap(px(16.0))
            .child(body)
            .child(facts)
            .into_element()
    } else {
        view().child(body).child(facts).into_element()
    };
    view()
        .test_id("changes-file")
        .gap(px(if terminal { 0.0 } else { 10.0 }))
        .child(head)
        .child(walks)
        .child(newer)
        .child(said)
        .child(hunks(app, opened))
        .into_element()
}

/// The hunks, drawn again when their text changes; which one is current is followed per hunk.
fn hunks(app: AppState, opened: Memo<Option<FileDiff>>) -> AnyElement {
    let form = app.form;
    let shapes = computed(move || {
        opened.with(|diff| {
            diff.as_ref()
                .map(|diff| {
                    diff.hunks
                        .iter()
                        .map(|hunk| Shape {
                            header: hunk.header.clone(),
                            lines: hunk.lines.clone(),
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
    });
    dynamic(move || {
        let shapes = shapes.get();
        untracked(|| {
            view()
                .gap(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
                .children(
                    shapes
                        .iter()
                        .enumerate()
                        .map(|(n, shape)| hunk(app, opened, n, shape))
                        .collect::<Vec<_>>(),
                )
                .into_element()
        })
    })
}

fn current_of(opened: Memo<Option<FileDiff>>, n: usize) -> Memo<bool> {
    computed(move || {
        opened.with(|diff| {
            diff.as_ref()
                .and_then(|diff| diff.hunks.get(n))
                .is_some_and(|hunk| hunk.current)
        })
    })
}

fn pick_hunk(
    n: usize,
    pick: fn(&HunkView) -> Action,
) -> impl Fn(&FileDiff) -> Option<Action> + 'static {
    move |diff| diff.hunks.get(n).map(pick)
}

/// One hunk: its header (pressing it makes the hunk current) with `i/n` while current and
/// Add to message, then its rows. A terminal draws no frame: the header quiet, `▌` before the
/// current one's, the rows under it.
fn hunk(app: AppState, opened: Memo<Option<FileDiff>>, n: usize, shape: &Shape) -> AnyElement {
    let form = app.form;
    let current = current_of(opened, n);
    let capture = button_with(
        app,
        "Add to message",
        ButtonKind::Quiet,
        ButtonOptions {
            key_hint: Some("a"),
            ..ButtonOptions::default()
        },
        format!("btn-capture-hunk-{n}"),
        press(app, opened, pick_hunk(n, |hunk| hunk.capture.clone())),
    );
    let select = press(app, opened, pick_hunk(n, |hunk| hunk.select.clone()));
    let header = text(shape.header.clone())
        .number_of_lines(1u32)
        .text_overflow(EllipsizeMode::Clip)
        .min_w(px(0.0))
        .flex_shrink(1.0)
        .text_color(move || {
            if current.get() {
                color(Token::Ink)
            } else {
                color(Token::Ink3)
            }
        })
        .style_when(move || current.get(), |s| s.font_weight(FontWeight::BOLD));
    let header = match form {
        Form::Terminal => header,
        _ => header.font_family(FONT_MONO).text_size(px(11.5)),
    };
    let count = move || opened.with(|diff| diff.as_ref().map_or(0, |diff| diff.hunks.len()));
    let position = text(move || {
        if current.get() {
            format!("{}/{}", n + 1, count())
        } else {
            String::new()
        }
    })
    .text_color(color(Token::Ink3))
    .flex_shrink_0();
    let position = match form {
        Form::Terminal => position,
        _ => position
            .font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(11.0)),
    };
    let mark = (form == Form::Terminal).then(|| {
        text(move || (if current.get() { "▌" } else { " " }).to_owned())
            .text_color(color(Token::Ink))
            .flex_shrink_0()
    });
    let head = view()
        .test_id(&format!("hunk-{n}"))
        .role(AccessibilityRole::Button)
        .label(format!("Hunk {}: {}", n + 1, shape.header))
        .accessibility_state(move || AccessibilityState {
            selected: current.get(),
            ..AccessibilityState::default()
        })
        .cursor_pointer()
        .on_press(select)
        .flex_1()
        .min_w(px(0.0))
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .children(mark)
        .child(header)
        .child(position);
    let bar = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(head)
        .child(capture);
    let rows: Vec<AnyElement> = shape
        .lines
        .iter()
        .map(|line| unified_line(app, line))
        .collect();
    let block = if form == Form::Terminal {
        view()
            .child(bar)
            .child(
                crate::ui::composer::terminal_selectable(app, view().selectable(true))
                    .children(rows),
            )
            .into_element()
    } else {
        view()
            .rounded(px(6.0))
            .border_1()
            .border_color(move || {
                if current.get() {
                    color(Token::Ink2)
                } else {
                    color(Token::Rule)
                }
            })
            .bg(color(Token::Surface))
            .overflow_hidden()
            .child(
                bar.min_h(px(if form == Form::Phone { 44.0 } else { 30.0 }))
                    .pl(px(10.0))
                    .pr(px(4.0))
                    .bg(color(Token::BgSunk))
                    .border_b(px(1.0))
                    .border_color(color(Token::Rule)),
            )
            .children(rows)
            .into_element()
    };
    revealing(current, block)
}
