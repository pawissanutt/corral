//! The shown build's detail: the handover head (outcome chip, id, project and update, title),
//! the override's callout when the build landed over open review findings, then the model's
//! sections in their fixed order, each block drawn with the kit: facts as a key and value
//! list, the owner's prompts as quotes, marked entries as step lists (a round's findings set
//! in under it), usage as a figure table, notes in the quiet ink.

use nsh_core::prelude::*;

use crate::model::builds::{self, Block, BuildView, Detail, Line, Section};
use crate::model::text::span;
use crate::model::{Form, Hue, Id};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::chip::chip;
use crate::ui::kit::notice::{empty, error, loading, notice};
use crate::ui::kit::page::{Width, pane, qhead, section};
use crate::ui::kit::table::{kv, steps, utable};
use crate::ui::kit::text::{dim, mono, plain, rich, rich_small};
use crate::ui::term;
use crate::ui::tokens::{Token, color};

/// The tracked detail of the shown build: `build` names it (the phone page's); otherwise the
/// root selection, or the first build listed. None while there is no build at all.
fn shown(app: AppState, build: Option<Id>) -> Memo<Option<Detail>> {
    computed(move || {
        let selection = app.selection();
        app.feed.with(|feed| {
            build
                .clone()
                .or_else(|| builds::current_build(&selection, feed))
                .map(|id| builds::detail_of(feed, &id))
        })
    })
}

/// The detail column on desktop and in a terminal.
pub(super) fn pane_view(app: AppState) -> AnyElement {
    pane(app, "build-detail", content(app, shown(app, None)))
}

/// The pushed phone page's body; the shell scrolls it.
pub(super) fn phone_body(app: AppState, build: Id) -> AnyElement {
    view()
        .test_id("build-detail")
        .child(content(app, shown(app, Some(build))))
        .into_element()
}

/// The padding of the detail's own blocks: the head's sides.
fn inset(app: AppState) -> View {
    match app.form {
        Form::Desktop => view().px(px(24.0)).py(px(14.0)),
        Form::Phone => view().px(px(18.0)).py(px(14.0)),
        Form::Terminal => view().px(term::cells(1.0)),
    }
}

fn content(app: AppState, detail: Memo<Option<Detail>>) -> AnyElement {
    dynamic(move || {
        let detail = detail.get();
        untracked(move || match detail {
            None => inset(app)
                .child(dim(app, builds::NONE_SELECTED))
                .into_element(),
            Some(Detail::Loading) => inset(app)
                .child(loading(
                    app,
                    builds::DETAIL_LOADING,
                    &[(0.4, 12.0), (0.8, 20.0), (0.6, 12.0), (0.7, 54.0)],
                ))
                .into_element(),
            Some(Detail::Failed { id, line }) => error(
                app,
                &vec![span(line)],
                Some(button(
                    app,
                    "Retry",
                    ButtonKind::Ghost,
                    "btn-retry",
                    move || {
                        app.run(builds::reduce(builds::Action::Retry(id.clone())));
                    },
                )),
            ),
            Some(Detail::Missing(id)) => empty(
                app,
                "Build not listed",
                format!("corrald no longer lists build {id}, and this device holds no copy of it."),
                None,
            ),
            Some(Detail::Ready(build)) => ready(app, &build),
        })
    })
    .into_element()
}

fn ready(app: AppState, build: &BuildView) -> AnyElement {
    let mut meta: Vec<AnyElement> = build
        .chips
        .iter()
        .map(|(label, hue)| chip(app, label.clone(), *hue))
        .collect();
    meta.push(
        mono(app, build.id.to_string())
            .text_color(color(Token::Ink3))
            .into_element(),
    );
    if !build.meta.is_empty() {
        meta.push(dim(app, build.meta.clone()).into_element());
    }
    view()
        .min_w(px(0.0))
        .child(qhead(app, meta, build.title.clone(), None, vec![]))
        .children(build.stale.as_ref().map(|line| {
            inset(app)
                .pb(px(0.0))
                .child(notice(app, Hue::Held, &vec![span(line.as_str())], None))
                .into_element()
        }))
        .children(build.alert.as_ref().map(|alert| {
            inset(app)
                .pb(px(0.0))
                .test_id("build-override")
                .child(notice(app, Hue::Held, alert, None))
                .into_element()
        }))
        .children(
            build
                .sections
                .iter()
                .map(|part| section_view(app, part))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

fn section_view(app: AppState, part: &Section) -> AnyElement {
    let gap = match app.form {
        Form::Desktop => 10.0,
        Form::Phone => 12.0,
        Form::Terminal => 0.0,
    };
    let blocks = view()
        .gap(px(gap))
        .children(
            part.blocks
                .iter()
                .map(|block| block_view(app, block))
                .collect::<Vec<_>>(),
        )
        .into_element();
    view()
        .test_id(&format!("build-section-{}", part.key))
        .child(section(
            app,
            Some(part.heading.to_owned()),
            part.note.clone(),
            Hue::Neutral,
            blocks,
        ))
        .into_element()
}

fn block_view(app: AppState, block: &Block) -> AnyElement {
    match block {
        Block::Facts(rows) => kv(
            app,
            rows.iter()
                .map(|(label, value)| (label.clone(), rich_small(app, value)))
                .collect(),
        ),
        Block::Quote { at, text } => quote(app, at, text),
        Block::Lines { lines, nested } => {
            let list = steps(
                app,
                lines
                    .iter()
                    .map(|line| (line.mark, line.hue, line_view(app, line)))
                    .collect(),
            );
            if !*nested {
                return list;
            }
            let indent = match app.form {
                Form::Terminal => view().pl(term::cells(2.0)),
                Form::Desktop | Form::Phone => view().pl(px(20.0)),
            };
            indent.child(list).into_element()
        }
        Block::Table { header, end, rows } => {
            let widths: Vec<Width> = header
                .iter()
                .enumerate()
                .map(|(at, _)| {
                    if at == 0 {
                        Width::Fill
                    } else {
                        Width::Fixed(96.0)
                    }
                })
                .collect();
            let cells: Vec<Vec<AnyElement>> = rows
                .iter()
                .map(|row| {
                    row.iter()
                        .enumerate()
                        .map(|(at, cell)| {
                            if at == 0 {
                                plain(app, cell.clone()).into_element()
                            } else {
                                mono(app, cell.clone()).into_element()
                            }
                        })
                        .collect()
                })
                .collect();
            utable(app, &widths, header, end, cells)
        }
        Block::Note(words) => dim(app, words.clone()).into_element(),
    }
}

/// One marked entry: its chips and head on the first line, each body line under it. A live
/// session's children and agent messages are drawn with it too.
pub(crate) fn line_view(app: AppState, line: &Line) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let first = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(if terminal { 0.0 } else { 6.0 }))
        .children(
            line.chips
                .iter()
                .flat_map(|(label, hue)| {
                    let mut parts = vec![chip(app, label.clone(), *hue)];
                    if terminal {
                        parts.push(term::space(1.0));
                    }
                    parts
                })
                .collect::<Vec<_>>(),
        )
        .child(view().flex_1().min_w(px(0.0)).child(rich(app, &line.head)));
    view()
        .min_w(px(0.0))
        .gap(px(if terminal { 0.0 } else { 2.0 }))
        .pb(px(if terminal { 0.0 } else { 6.0 }))
        .child(first)
        .children(
            line.body
                .iter()
                .map(|words| rich_small(app, words))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// The owner's words as said, set off by a rule (a Cell's indent in a terminal), with when.
fn quote(app: AppState, at: &str, words: &str) -> AnyElement {
    let said = view()
        .min_w(px(0.0))
        .gap(px(if app.form == Form::Terminal { 0.0 } else { 2.0 }))
        .child(dim(
            app,
            if at.is_empty() {
                "Owner".to_owned()
            } else {
                format!("Owner · {at}")
            },
        ))
        .child(rich(app, &vec![span(words)]));
    match app.form {
        Form::Terminal => view()
            .flex_row()
            .child(term::quiet("│"))
            .child(term::space(1.0))
            .child(said.flex_1())
            .into_element(),
        Form::Desktop | Form::Phone => said
            .pl(px(12.0))
            .border_l(px(1.0))
            .border_l_color(color(Token::Rule2))
            .into_element(),
    }
}
