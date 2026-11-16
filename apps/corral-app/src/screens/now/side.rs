//! Now's side column: one panel for one selection. With nothing selected, what changed since the
//! last visit, one line per task with trouble first (its full sentence on hover or focus) and the
//! way to every event in Changes; for an owner move, what it asks, what it holds up and its one
//! button; for a task, open or closed, its story with the Story, Evidence and Session tabs and
//! its commands. A close control returns the column to the digest, as Escape does.

use nsh_core::prelude::*;

use crate::model::work::{
    self, NowAskPanel, NowDigest, NowDigestRow, NowPage, NowPanel, NowTab, Press,
};
use crate::model::{Destination, Form};
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with, tab_strip};
use crate::ui::kit::glance::stat;
use crate::ui::kit::hint::{hint_on, hinted};
use crate::ui::kit::marks::mark;
use crate::ui::kit::text::dim;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color, text_size};

use super::inspector::content;
use super::parts::{act, go_scoped, model_button, press};
use super::rows::{figures, kind_mark, words};

const RING: f32 = 2.0;
/// The digest's columns (`.dg li`): the mark and the task id.
const MARK: (f32, f32) = (16.0, 1.0);
const TASK: (f32, f32) = (50.0, 6.0);

/// Which panel shows, apart from what it holds: switching kind builds the column again, while
/// a change inside one panel rebuilds only its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Digest,
    Ask,
    Task,
}

/// The side column's content for `page`'s panel, without its scroll.
pub(crate) fn panel(app: AppState, page: Memo<NowPage>) -> AnyElement {
    let kind = computed(move || {
        page.with(|page| match page.panel {
            NowPanel::Digest(_) => Kind::Digest,
            NowPanel::Ask(_) => Kind::Ask,
            NowPanel::Task(_) | NowPanel::Closed(_) => Kind::Task,
        })
    });
    let panel = computed(move || page.with(|page| page.panel.clone()));
    let close_label = untracked(|| page.with(|page| page.close_label));
    dynamic(move || match kind.get() {
        Kind::Digest => untracked(|| digest_panel(app, panel, page)),
        Kind::Ask => untracked(|| ask_panel(app, panel, close_label)),
        Kind::Task => untracked(|| {
            let clear = page.with(|page| page.clear_selection.clone());
            task_panel(app, panel, close_label, clear)
        }),
    })
}

/// A block of the column (`.sec`): 14 18 on desktop and phone, a hairline under it; a Cell in
/// on a terminal.
fn sec(form: Form) -> View {
    match form {
        Form::Desktop | Form::Phone => view()
            .px(px(18.0))
            .py(px(14.0))
            .border_b_1()
            .border_b_color(color(Token::Rule)),
        Form::Terminal => view().px(term::cells(1.0)),
    }
}

/// The column's head (`.phd`): 14 18 12, a hairline under it.
fn head(form: Form) -> View {
    match form {
        Form::Desktop | Form::Phone => view()
            .px(px(18.0))
            .pt(px(14.0))
            .pb(px(12.0))
            .gap(px(4.0))
            .border_b_1()
            .border_b_color(color(Token::Rule)),
        Form::Terminal => view().px(term::cells(1.0)),
    }
}

/// The head's first line (`.phd .r1`): 24 pt, its parts 8 pt apart.
fn first_line(form: Form) -> View {
    view()
        .flex_row()
        .items_center()
        .gap(if form == Form::Terminal {
            term::cells(1.0)
        } else {
            px(8.0)
        })
        .when(form != Form::Terminal, |line| line.min_h(px(24.0)))
}

/// The close control at the end of a head's first line: Escape's press.
fn close(app: AppState, label: &'static str, action: Press) -> AnyElement {
    button_with(
        app,
        label,
        ButtonKind::Quiet,
        ButtonOptions {
            icon: Some(Icon::Close),
            icon_only: true,
            ..ButtonOptions::default()
        },
        "btn-clear-selection",
        move || press(app, action.clone()),
    )
}

fn spacer() -> View {
    view().flex_1().min_w(px(0.0))
}

/// A panel's title (`.phd h2`): 16 pt semibold, wrapping.
fn title(form: Form, content: String) -> Text {
    let title = text(content)
        .role(AccessibilityRole::Header)
        .font_weight(if form == Form::Terminal {
            FontWeight::BOLD
        } else {
            FontWeight::SEMIBOLD
        })
        .text_color(color(Token::Ink));
    if form == Form::Terminal {
        title
    } else {
        title.font_family(FONT_TEXT).text_size(px(16.0))
    }
}

/// What changed since the last visit, in Now's scope.
fn digest_panel(app: AppState, panel: Memo<NowPanel>, page: Memo<NowPage>) -> AnyElement {
    let digest = computed(move || {
        panel.with(|panel| match panel {
            NowPanel::Digest(digest) => Some(digest.clone()),
            _ => None,
        })
    });
    dynamic(move || {
        digest.with(|digest| digest.as_ref().map(|digest| digest_view(app, digest, page)))
    })
}

fn digest_view(app: AppState, digest: &NowDigest, page: Memo<NowPage>) -> AnyElement {
    let form = app.form;
    let since = hinted(
        app,
        words(form, digest.since.clone(), 12.0, color(Token::Ink3)),
        digest.coverage.clone(),
    );
    let counts = view()
        .flex_row()
        .items_center()
        .gap(if form == Form::Terminal {
            term::cells(2.0)
        } else {
            px(12.0)
        })
        .children(
            digest
                .stats
                .iter()
                .map(|count| stat(app, count))
                .collect::<Vec<_>>(),
        );
    let mut out = vec![
        head(form)
            .test_id("now-digest")
            .child(first_line(form).child(since).child(spacer()).child(counts))
            .into_element(),
    ];
    if let Some(notice) = &digest.notice {
        out.push(
            sec(form)
                .child(dim(app, notice.text().to_owned()))
                .into_element(),
        );
    }
    if !digest.rows.is_empty() {
        out.push(
            sec(form)
                .role(AccessibilityRole::List)
                .label(digest.since.clone())
                .children(
                    digest
                        .rows
                        .iter()
                        .enumerate()
                        .map(|(index, row)| digest_row(app, row, index))
                        .collect::<Vec<_>>(),
                )
                .into_element(),
        );
    }
    if digest.events.count > 0 {
        out.push(
            sec(form)
                .flex_row()
                .child(button_with(
                    app,
                    digest.more.label.to_string(),
                    ButtonKind::Quiet,
                    ButtonOptions {
                        icon: Some(Icon::Chevron),
                        ..ButtonOptions::default()
                    },
                    "btn-changes",
                    move || go_scoped(app, page, Destination::Changes),
                ))
                .into_element(),
        );
    }
    view().children(out).into_element()
}

/// One task's line of the digest (`.dg li`): its mark in the outcome's hue, the task id, the
/// short phrase and the time; the full sentence on hover or focus. A line about a task opens
/// it. test id `since-<n>`.
fn digest_row(app: AppState, row: &NowDigestRow, index: usize) -> AnyElement {
    let form = app.form;
    let id = row
        .task
        .as_ref()
        .map_or_else(String::new, ToString::to_string);
    let label = [id.as_str(), row.phrase, row.time.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let line = view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(if form == Form::Terminal {
            term::cells(1.0)
        } else {
            px(8.0)
        })
        .when(form != Form::Terminal, |line| line.py(px(6.0)))
        .test_id(&format!("since-{index}"))
        .accessible(true)
        .label(label)
        .child(super::rows::column(
            form,
            MARK,
            false,
            mark(app, row.mark, row.hue),
        ))
        .child(super::rows::column(
            form,
            TASK,
            false,
            figures(form, id, 12.0, Token::Ink3),
        ))
        .child(
            words(form, row.phrase, 12.5, color(Token::Ink2))
                .flex_1()
                .min_w(px(0.0)),
        )
        .child(figures(form, row.time.clone(), 12.0, Token::Ink3).flex_shrink_0());
    let line = match row.action.clone() {
        Some(action) => line
            .cursor_pointer()
            .role(AccessibilityRole::Button)
            .on_press(move || press(app, action.clone()))
            .when(form == Form::Desktop, |line| {
                line.hover(|s| s.bg(color(Token::Hover)))
            })
            .focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(-RING))
            }),
        None => line
            .role(AccessibilityRole::Text)
            .focusable(form != Form::Phone),
    };
    hint_on(app, line, row.hint.clone()).into_element()
}

/// An owner move beside the list: what it asks, what it holds up, who asked and when, its one
/// button and the record it concerns.
fn ask_panel(app: AppState, panel: Memo<NowPanel>, close_label: &'static str) -> AnyElement {
    let ask = computed(move || {
        panel.with(|panel| match panel {
            NowPanel::Ask(ask) => Some(ask.clone()),
            _ => None,
        })
    });
    dynamic(move || ask.with(|ask| ask.as_ref().map(|ask| ask_view(app, ask, close_label))))
}

fn ask_view(app: AppState, ask: &NowAskPanel, close_label: &'static str) -> AnyElement {
    let form = app.form;
    let row = &ask.row;
    let first = first_line(form)
        .child(kind_mark(form, row.kind, 14.0))
        .child(figures(form, row.reference.clone(), 12.0, Token::Ink3).flex_shrink_0())
        .children(
            row.project
                .clone()
                .map(|project| words(form, project, 12.0, color(Token::Ink3))),
        )
        .child(spacer())
        .child(close(app, close_label, ask.close.clone()));
    let meta = [ask.asked_by.clone(), ask.asked_at.clone()]
        .into_iter()
        .flatten()
        .map(|part| words(form, part, 12.0, color(Token::Ink3)).into_element())
        .collect::<Vec<_>>();
    let header = head(form)
        .test_id("now-ask")
        .child(first)
        .child(title(form, row.title.clone()))
        .when(!meta.is_empty(), |header| {
            header.child(view().flex_row().flex_wrap().gap(px(8.0)).children(meta))
        });
    let consequence = text(row.consequence.clone()).text_color(color(Token::Ink2));
    let consequence = if form == Form::Terminal {
        consequence
    } else {
        consequence
            .font_family(FONT_TEXT)
            .text_size(px(text_size(form)))
    };
    let actions = view()
        .flex_row()
        .flex_wrap()
        .gap(if form == Form::Terminal {
            term::cells(2.0)
        } else {
            px(8.0)
        })
        .child(model_button(app, &row.button, "btn-ask-open".into(), false))
        .children(
            ask.link
                .as_ref()
                .map(|link| model_button(app, link, "btn-ask-link".into(), false)),
        );
    view()
        .child(header)
        .child(sec(form).child(consequence))
        .child(sec(form).child(actions))
        .into_element()
}

/// A task beside the list: the Story, Evidence and Session tabs with the close control, over
/// the inspector's content for the model's tab.
fn task_panel(
    app: AppState,
    panel: Memo<NowPanel>,
    close_label: &'static str,
    clear: Press,
) -> AnyElement {
    let form = app.form;
    let options: Vec<(NowTab, &'static str)> =
        NowTab::ALL.iter().map(|&tab| (tab, tab.label())).collect();
    let tabs = tab_strip(
        app,
        &options,
        move || app.work.with(|state| state.now_tab),
        move |tab| act(app, work::Action::NowTab(tab)),
        "seg-inspector",
    );
    let bar = first_line(form)
        .when(form != Form::Terminal, |bar| bar.px(px(10.0)).pt(px(6.0)))
        .when(form == Form::Terminal, |bar| bar.px(term::cells(1.0)))
        .child(view().flex_1().min_w(px(0.0)).child(tabs))
        .child(close(app, close_label, clear));
    let inspector = computed(move || {
        panel.with(|panel| match panel {
            NowPanel::Task(inspector) | NowPanel::Closed(inspector) => Some(inspector.clone()),
            _ => None,
        })
    });
    view()
        .test_id("now-task")
        .child(bar)
        .child(dynamic(move || {
            inspector.with(|inspector| inspector.as_ref().map(|inspector| content(app, inspector)))
        }))
        .into_element()
}
