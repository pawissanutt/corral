//! The published trackers: an executing plan's pipeline, warning, member graph and blockers
//! with the selected member's story beside them, and the synthetic 64-member strips.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::plans::{Blocker, Detail, ExecutingView, LegendEntry, ScaleLayerView, ScaleLine};
use crate::state::AppState;
use crate::ui::kit::marks::{dot, hue_ink, mark};
use crate::ui::kit::page::{Width, columns, context, pane as scroll_pane};
use crate::ui::kit::table::steps;
use crate::ui::kit::text::{dim, h3, plain};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::detail::{header, inset, pipeline};
use super::{graph, square_fill, square_glyph, squares, status_ink, story};

/// The executing plan on desktop (the tracker beside the 340 pt story column) and in a
/// terminal (one scrolling pane, the story under the tracker).
pub(super) fn executing_pane(
    app: AppState,
    detail: Memo<Detail>,
    plan: &'static str,
) -> AnyElement {
    // The detail is cloned out before elements are built, so no memo is borrowed while the
    // story's own bindings run.
    let story = dynamic(move || match detail.get() {
        Detail::Executing(tracker) => tracker
            .story
            .as_ref()
            .map(|inspector| story::story(app, inspector)),
        _ => None,
    });
    match app.form {
        Form::Terminal => scroll_pane(
            app,
            "plan-detail",
            view()
                .child(executing_body(app, detail, plan))
                .child(term::hrule())
                .child(story)
                .into_element(),
        ),
        Form::Desktop | Form::Phone => columns(
            app,
            vec![
                (
                    Width::Fill,
                    scroll_pane(app, "plan-main", executing_body(app, detail, plan)),
                ),
                (Width::Fixed(340.0), context(app, "plan-side", story)),
            ],
            0,
        ),
    }
}

/// The tracker's head and body: pipeline, the unmapped-criteria warning, the member graph and
/// what blocks final acceptance. The phone page shows it alone.
pub(super) fn executing_body(
    app: AppState,
    detail: Memo<Detail>,
    plan: &'static str,
) -> AnyElement {
    let Detail::Executing(tracker) = detail.get_untracked() else {
        return view().into_element();
    };
    let selected = move || detail_selected(detail);
    view()
        .min_w(px(0.0))
        .child(header(app, &tracker.header))
        .child(
            inset(app)
                .child(pipeline(app, &tracker.pipeline))
                .child(warning(app, &tracker))
                .child(flush_caption(app, tracker.members_heading))
                .child(graph::drawing(app, plan, selected))
                .child(h3(
                    app,
                    tracker.blockers_heading,
                    Some(tracker.blocker_count.to_string()),
                ))
                .child(steps(
                    app,
                    tracker
                        .blockers
                        .iter()
                        .map(|blocker| (blocker.mark, blocker.hue, blocker_text(app, blocker)))
                        .collect(),
                )),
        )
        .into_element()
}

/// The member the model resolved as selected, tracked.
fn detail_selected(detail: Memo<Detail>) -> &'static str {
    detail.with(|detail| match detail {
        Detail::Executing(tracker) => tracker.selected.unwrap_or_default(),
        _ => "",
    })
}

/// A section caption that starts its block (`.h3` with `margin-top: 0`): the kit caption
/// without its space above.
fn flush_caption(app: AppState, label: &'static str) -> AnyElement {
    let caption = h3(app, label, None);
    match app.form {
        Form::Terminal => caption,
        Form::Desktop => view().mt(px(-18.0)).child(caption).into_element(),
        Form::Phone => view().mt(px(-14.0)).child(caption).into_element(),
    }
}

/// The warning bar (`.warnbar`): the stale mark and the sentence in the held colour on its
/// ground, padding 7 12, radius 6, 14 pt under it. A terminal writes the mark and sentence.
fn warning(app: AppState, tracker: &ExecutingView) -> AnyElement {
    let ink = hue_ink(Hue::Held);
    let words = text(tracker.warning.clone())
        .flex_1()
        .min_w(px(0.0))
        .text_color(ink);
    if app.form == Form::Terminal {
        return view()
            .child(
                view()
                    .flex_row()
                    .items_start()
                    .child(mark(app, tracker.warning_mark, Hue::Held))
                    .child(term::space(1.0))
                    .child(words),
            )
            .child(term::blank())
            .into_element();
    }
    let size = if app.form == Form::Phone { 13.5 } else { 12.5 };
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(12.0))
        .py(px(7.0))
        .mb(px(14.0))
        .rounded(px(6.0))
        .bg(color(Token::HeldBg))
        .role(AccessibilityRole::Alert)
        .child(mark(app, tracker.warning_mark, Hue::Held))
        .child(
            words
                .font_family(FONT_TEXT)
                .text_size(px(size))
                .line_height(px((size * 1.45).round())),
        )
        .into_element()
}

fn blocker_text(app: AppState, blocker: &Blocker) -> AnyElement {
    plain(app, blocker.text)
        .text_color(match blocker.hue {
            Hue::Neutral => color(Token::Ink),
            hue => hue_ink(hue),
        })
        .into_element()
}

/// The synthetic tracker: head, pipeline, the explanation, a strip per layer with its moving
/// members, and the legend.
pub(super) fn scale(app: AppState, detail: Memo<Detail>) -> AnyElement {
    let Detail::Scale(tracker) = detail.get_untracked() else {
        return view().into_element();
    };
    view()
        .min_w(px(0.0))
        .child(header(app, &tracker.header))
        .child(
            inset(app)
                .child(pipeline(app, &tracker.pipeline))
                .child(explanation(app, tracker.explanation))
                .children(
                    tracker
                        .layers
                        .iter()
                        .map(|layer| scale_layer(app, layer))
                        .collect::<Vec<_>>(),
                )
                .child(legend(app, &tracker.legend)),
        )
        .into_element()
}

fn explanation(app: AppState, words: &'static str) -> AnyElement {
    if app.form == Form::Terminal {
        return view()
            .child(dim(app, words))
            .child(term::blank())
            .into_element();
    }
    dim(app, words)
        .text_size(px(if app.form == Form::Phone { 13.5 } else { 12.5 }))
        .mb(px(8.0))
        .into_element()
}

/// Words at the strip's size: 12.5 pt on desktop, 13 on the phone, the terminal's own.
fn strip_text(app: AppState, words: impl Into<SharedString>, ink: AdaptiveColor) -> Text {
    let words = text(words.into()).number_of_lines(1u32).text_color(ink);
    match app.form {
        Form::Terminal => words,
        Form::Desktop => words.font_family(FONT_TEXT).text_size(px(12.5)),
        Form::Phone => words.font_family(FONT_TEXT).text_size(px(13.0)),
    }
}

/// One layer: label, strip and counts; the moving members under them (110 pt in on desktop),
/// then "+N more". A rule under each layer; a terminal writes it as one line per row.
fn scale_layer(app: AppState, layer: &ScaleLayerView) -> AnyElement {
    let form = app.form;
    let indent = match form {
        Form::Desktop => px(110.0),
        Form::Phone => px(0.0),
        Form::Terminal => term::cells(8.0),
    };
    let label =
        strip_text(app, layer.label.clone(), color(Token::Ink)).font_weight(FontWeight::BOLD);
    let summary = strip_text(app, layer.summary.clone(), color(Token::Ink3));
    let head = match form {
        Form::Desktop => view()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .child(view().w(px(110.0)).flex_shrink_0().child(label))
            .child(squares(app, &layer.squares))
            .child(summary.flex_1().min_w(px(0.0)).truncate())
            .into_element(),
        Form::Phone => view()
            .gap(px(4.0))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .child(view().w(px(70.0)).flex_shrink_0().child(label))
                    .child(squares(app, &layer.squares)),
            )
            .child(summary)
            .into_element(),
        Form::Terminal => view()
            .flex_row()
            .items_center()
            .child(view().w(term::cells(8.0)).flex_shrink_0().child(label))
            .child(squares(app, &layer.squares))
            .child(term::space(2.0))
            .child(summary.flex_1().min_w(px(0.0)).truncate())
            .into_element(),
    };
    let block = view()
        .role(AccessibilityRole::List)
        .label(format!("{}, {}", layer.label, layer.summary))
        .child(head)
        .children(
            layer
                .shown
                .iter()
                .map(|line| scale_line(app, line, indent))
                .collect::<Vec<_>>(),
        )
        .children(layer.more.clone().map(|more| {
            view()
                .pl(indent)
                .child(strip_text(app, more, color(Token::Ink3)))
                .into_element()
        }));
    if form == Form::Terminal {
        return block.into_element();
    }
    block
        .py(px(8.0))
        .gap(px(3.0))
        .border_b(px(1.0))
        .border_color(color(Token::Rule))
        .into_element()
}

/// A moving member: dot, id, generated name, its status at the trailing edge.
fn scale_line(app: AppState, line: &ScaleLine, indent: Points) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let id = strip_text(app, line.id, color(Token::Ink3));
    let id = if terminal {
        id
    } else {
        id.font_family(FONT_MONO).tabular_nums()
    };
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .pl(indent)
        .accessible(true)
        .label(format!("{} {}, {}", line.id, line.name, line.status))
        .child(
            view()
                .w(if terminal { term::cells(1.0) } else { px(14.0) })
                .flex_shrink_0()
                .child(dot(app, line.hue)),
        )
        .when(terminal, |row| row.child(term::space(1.0)))
        .child(
            view()
                .w(if terminal { term::cells(6.0) } else { px(52.0) })
                .flex_shrink_0()
                .child(id),
        )
        .child(
            strip_text(app, line.name.clone(), color(Token::Ink))
                .flex_1()
                .min_w(px(0.0))
                .truncate(),
        )
        .child(strip_text(app, line.status, status_ink(line.status_hue)).flex_shrink_0())
        .into_element()
}

/// The legend (`.legend`): an 18 by 8 pt swatch per state and its word, 16 pt apart; a
/// terminal shows each state's glyph.
fn legend(app: AppState, entries: &[LegendEntry]) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .when(!terminal, |row| {
            row.gap_x(px(16.0)).gap_y(px(6.0)).pt(px(10.0))
        })
        .when(terminal, |row| row.child(term::blank()))
        .children(
            entries
                .iter()
                .enumerate()
                .map(|(at, entry)| {
                    let swatch = if terminal {
                        let (glyph, ink) = square_glyph(entry.state, entry.hue);
                        text(glyph).text_color(ink).into_element()
                    } else {
                        view()
                            .w(px(18.0))
                            .h(px(8.0))
                            .rounded(px(2.0))
                            .bg(square_fill(entry.state, entry.hue))
                            .into_element()
                    };
                    let words = text(entry.label)
                        .number_of_lines(1u32)
                        .text_color(color(Token::Ink3));
                    view()
                        .flex_row()
                        .items_center()
                        .when(terminal && at > 0, |item| item.child(term::space(2.0)))
                        .child(swatch)
                        .when(terminal, |item| item.child(term::space(1.0)))
                        .when(!terminal, |item| item.gap(px(6.0)))
                        .child(if terminal {
                            words.into_element()
                        } else {
                            words
                                .font_family(FONT_TEXT)
                                .text_size(px(11.5))
                                .into_element()
                        })
                        .into_element()
                })
                .collect::<Vec<_>>(),
        )
        .into_element()
}
