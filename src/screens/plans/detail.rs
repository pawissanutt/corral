//! The selected plan's detail: which variant shows, the head and pipeline every variant
//! shares, and the awaiting plan's review document with its decide bar.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::model::plans::{
    self, AwaitingView, Detail, DocumentSection, Header, PipelineStage, ProposedRow, ReviewRow,
    SectionBody,
};
use crate::model::text::Rich;
use crate::state::AppState;
use crate::ui::kit::chip::{chip, illus};
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::notice::empty;
use crate::ui::kit::page::{bar, bar_pane, pane as scroll_pane, qhead};
use crate::ui::kit::text::{rich, rich_small};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

use super::{command, tracker};

/// Which detail shows; the executing plan's id names its graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Awaiting,
    Executing(&'static str),
    Approved,
    Scale,
}

/// The tracked detail of the selected plan, and which variant it is. The variant changes only
/// when the selection does, so a member press or a connection change redraws the parts that
/// read them, not the scroll they sit in.
fn memos(app: AppState) -> (Memo<Detail>, Memo<Kind>) {
    let detail = computed(move || {
        let env = app.env();
        app.plans.with(|state| plans::detail(state, env))
    });
    let kind = computed(move || {
        let plan = app.plans.with(|state| plans::plan(state.plan).id);
        detail.with(|detail| match detail {
            Detail::Awaiting(_) => Kind::Awaiting,
            Detail::Executing(_) => Kind::Executing(plan),
            Detail::Approved(_) => Kind::Approved,
            Detail::Scale(_) => Kind::Scale,
        })
    });
    (detail, kind)
}

/// The detail column on desktop and in a terminal: the review with its pinned decide bar, the
/// tracker beside its member story, the approved plan's empty state, or the 64-member strips.
pub(super) fn pane(app: AppState) -> AnyElement {
    let (detail, kind) = memos(app);
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(dynamic(move || match kind.get() {
            Kind::Awaiting => bar_pane(
                app,
                "plan-detail",
                awaiting_content(app, detail),
                decide(app, detail),
            ),
            Kind::Executing(plan) => tracker::executing_pane(app, detail, plan),
            Kind::Approved => scroll_pane(app, "plan-detail", approved(app, detail)),
            Kind::Scale => scroll_pane(app, "plan-detail", tracker::scale(app, detail)),
        }))
        .into_element()
}

/// The pushed phone page's body; the shell scrolls it.
pub(super) fn phone_body(app: AppState) -> AnyElement {
    let (detail, kind) = memos(app);
    view()
        .test_id("plan-detail")
        .child(dynamic(move || match kind.get() {
            Kind::Awaiting => awaiting_content(app, detail),
            Kind::Executing(plan) => tracker::executing_body(app, detail, plan),
            Kind::Approved => approved(app, detail),
            Kind::Scale => tracker::scale(app, detail),
        }))
        .into_element()
}

/// The phone's decide bar (`.pdecide`): Changes and the wide Accept, or the refused Accept
/// alone while offline.
pub(super) fn phone_bar(app: AppState) -> AnyElement {
    let (detail, _) = memos(app);
    dynamic(move || match detail.get() {
        Detail::Awaiting(review) => Some(bar(
            app,
            review
                .decide
                .iter()
                .map(|button| command(app, button, button.primary))
                .collect(),
            None,
        )),
        _ => None,
    })
}

/// Room around a detail's content (`.plan-doc`, the tracker body): 16 24 on desktop, 14 18 on
/// the phone, a Cell each side in a terminal.
pub(super) fn inset(app: AppState) -> View {
    match app.form {
        Form::Desktop => view().px(px(24.0)).py(px(16.0)),
        Form::Phone => view().px(px(18.0)).py(px(14.0)),
        Form::Terminal => view().px(term::cells(1.0)),
    }
}

/// The handover head of a plan: state chips, the id, the meta line and its illustrative tag,
/// the title, and on desktop the Ask command.
pub(super) fn header(app: AppState, head: &Header) -> AnyElement {
    let mut meta: Vec<AnyElement> = head
        .chips
        .iter()
        .map(|state| chip(app, state.label.clone(), state.hue))
        .collect();
    let id = text(head.id.clone())
        .flex_shrink_0()
        .number_of_lines(1u32)
        .text_color(color(if app.form == Form::Phone {
            Token::Ink2
        } else {
            Token::Ink3
        }));
    meta.push(if app.form == Form::Terminal {
        id.into_element()
    } else {
        id.font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(if app.form == Form::Phone { 13.0 } else { 12.0 }))
            .into_element()
    });
    if !head.meta.is_empty() {
        meta.push(rich_small(app, &head.meta));
    }
    meta.extend(head.illustrative.map(|label| illus(app, label)));
    let actions = head
        .ask
        .iter()
        .map(|button| command(app, button, false))
        .collect();
    qhead(app, meta, head.title, None, actions)
}

/// The pipeline (`.pipe`): the four stages as outlined steps joined by short rules; the active
/// one in ink with a heavier edge, passed ones in ink-2, later ones quiet. A terminal writes
/// them on a line, joined by `›` (a lone `─` would be read as a divider and joined).
pub(super) fn pipeline(app: AppState, stages: &[PipelineStage]) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let mut parts = Vec::with_capacity(stages.len() * 2);
    for (at, stage) in stages.iter().enumerate() {
        if at > 0 {
            parts.push(if terminal {
                view()
                    .flex_row()
                    .child(term::space(1.0))
                    .child(term::quiet("›"))
                    .child(term::space(1.0))
                    .into_element()
            } else {
                view()
                    .w(px(22.0))
                    .h(px(1.0))
                    .bg(color(Token::Rule2))
                    .into_element()
            });
        }
        parts.push(pipe_stage(app, stage));
    }
    let row = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .role(AccessibilityRole::List)
        .children(parts);
    if terminal {
        row.into_element()
    } else {
        row.gap_y(px(4.0)).mt(px(4.0)).mb(px(14.0)).into_element()
    }
}

fn pipe_stage(app: AppState, stage: &PipelineStage) -> AnyElement {
    let on = stage.mark == Mark::Active;
    let ink = color(match stage.mark {
        Mark::Active => Token::Ink,
        Mark::Done => Token::Ink2,
        _ => Token::Ink3,
    });
    let label = text(stage.label.clone())
        .number_of_lines(1u32)
        .text_color(if app.form == Form::Terminal {
            color(Token::Ink)
        } else {
            ink
        })
        .when(on, |t| t.font_weight(FontWeight::BOLD));
    let item = view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .accessible(true)
        .label(format!("{}, {}", stage.label, stage.mark.label()))
        .child(mark(app, stage.mark, stage.hue));
    if app.form == Form::Terminal {
        return item.child(term::space(1.0)).child(label).into_element();
    }
    item.gap(px(7.0))
        .px(px(12.0))
        .py(px(6.0))
        .rounded(px(6.0))
        .border(px(if on { 1.5 } else { 1.0 }))
        .border_color(color(if on { Token::Ink } else { Token::Rule }))
        .child(
            label
                .font_family(FONT_TEXT)
                .text_size(px(12.5))
                .font_weight(if on {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                }),
        )
        .into_element()
}

/// The awaiting plan's head over its document.
fn awaiting_content(app: AppState, detail: Memo<Detail>) -> AnyElement {
    match detail.get_untracked() {
        Detail::Awaiting(review) => element_view()
            .child(header(app, &review.header))
            .child(document(app, &review))
            .into_element(),
        _ => element_view().into_element(),
    }
}

fn element_view() -> View {
    view().min_w(px(0.0))
}

/// The review document (`.plan-doc`): pipeline, then each section's caption over its
/// paragraph, list, proposed members or verdicts, then the document's path.
fn document(app: AppState, review_doc: &AwaitingView) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    inset(app)
        .child(pipeline(app, &review_doc.pipeline))
        .children(
            review_doc
                .sections
                .iter()
                .map(|section| doc_section(app, section))
                .collect::<Vec<_>>(),
        )
        .child(
            element_view()
                .when(!terminal, |v| v.mt(px(10.0)))
                .when(terminal, |v| v.child(term::blank()))
                .child(rich_small(app, &review_doc.artifact)),
        )
        .into_element()
}

/// A section caption (`.plan-doc h3`): 12 pt spaced capitals in ink-3, 18 pt above and 6
/// below, no rule; a terminal leaves a blank row and writes it bold.
fn doc_heading(app: AppState, heading: &'static str) -> AnyElement {
    let caption = text(heading)
        .role(AccessibilityRole::Header)
        .uppercase()
        .number_of_lines(1u32);
    if app.form == Form::Terminal {
        return element_view()
            .child(term::blank())
            .child(
                caption
                    .font_weight(FontWeight::BOLD)
                    .text_color(color(Token::Ink)),
            )
            .into_element();
    }
    caption
        .mt(px(18.0))
        .mb(px(6.0))
        .font_family(FONT_TEXT)
        .text_size(px(12.0))
        .letter_spacing(px(12.0 * 0.07))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(Token::Ink3))
        .into_element()
}

fn doc_section(app: AppState, section: &DocumentSection) -> AnyElement {
    let body = match &section.body {
        SectionBody::Paragraph(words) => element_view()
            .when(app.form == Form::Desktop, |v| v.max_w(px(620.0)))
            .child(rich(app, words))
            .into_element(),
        SectionBody::Bullets(items) => bullets(app, items),
        SectionBody::Proposed(rows) => element_view()
            .gap(px(4.0))
            .children(
                rows.iter()
                    .map(|row| proposed(app, row))
                    .collect::<Vec<_>>(),
            )
            .into_element(),
        SectionBody::Reviews(rows) => element_view()
            .gap(px(6.0))
            .children(rows.iter().map(|row| review(app, row)).collect::<Vec<_>>())
            .into_element(),
    };
    element_view()
        .child(doc_heading(app, section.heading))
        .child(body)
        .into_element()
}

/// A plain bulleted list (`.plan-doc ul`): items 4 pt apart, 18 pt in.
fn bullets(app: AppState, items: &'static [&'static str]) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let size = text_size(app.form);
    element_view()
        .role(AccessibilityRole::List)
        .gap(px(if terminal { 0.0 } else { 4.0 }))
        .children(
            items
                .iter()
                .map(|item| {
                    let bullet = text("•").text_color(color(Token::Ink2));
                    let words = text(*item)
                        .flex_1()
                        .min_w(px(0.0))
                        .text_color(color(Token::Ink));
                    let (bullet, words) = if terminal {
                        (bullet, words)
                    } else {
                        (
                            bullet.font_family(FONT_TEXT).text_size(px(size)),
                            words
                                .font_family(FONT_TEXT)
                                .text_size(px(size))
                                .line_height(px((size * 1.45).round())),
                        )
                    };
                    element_view()
                        .flex_row()
                        .items_start()
                        .child(
                            element_view()
                                .w(if terminal { term::cells(2.0) } else { px(18.0) })
                                .flex_shrink_0()
                                .items_center()
                                .child(bullet),
                        )
                        .child(words)
                        .into_element()
                })
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// A proposed member: its untouched mark, the name in mono, "can start now" or "after …" at
/// the trailing edge.
fn proposed(app: AppState, row: &ProposedRow) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let name = text(row.name)
        .flex_1()
        .min_w(px(0.0))
        .truncate()
        .text_color(color(Token::Ink));
    let sub = text(row.sub.clone())
        .flex_shrink_0()
        .number_of_lines(1u32)
        .text_color(color(Token::Ink3));
    let (name, sub) = if terminal {
        (name, sub)
    } else {
        (
            name.font_family(FONT_MONO).text_size(px(12.0)),
            sub.font_family(FONT_TEXT).text_size(px(12.0)),
        )
    };
    element_view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(
            element_view()
                .w(if terminal { term::cells(2.0) } else { px(22.0) })
                .flex_shrink_0()
                .child(mark(app, row.mark, Hue::Neutral)),
        )
        .child(name)
        .child(sub)
        .into_element()
}

/// A verdict line (`28px 20px 70px 1fr`): revision, mark, verdict, who and note; a
/// superseded revision at 75 % opacity.
fn review(app: AppState, row: &ReviewRow) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let column = |points: f32, cells: f32| {
        element_view()
            .w(if terminal {
                term::cells(cells)
            } else {
                px(points)
            })
            .flex_shrink_0()
    };
    let rev = text(row.rev)
        .number_of_lines(1u32)
        .text_color(color(Token::Ink));
    let rev = if terminal {
        rev
    } else {
        rev.font_family(FONT_MONO).text_size(px(11.5))
    };
    element_view()
        .flex_row()
        .items_start()
        .gap(px(8.0))
        .when(row.superseded && !terminal, |v| v.opacity(0.75))
        .child(column(28.0, 3.0).child(rev))
        .child(
            column(20.0, 2.0)
                .pt(px(2.0))
                .child(mark(app, row.mark, Hue::Neutral)),
        )
        .child(column(70.0, 22.0).child(rich_small(app, &row.verdict)))
        .child(
            element_view()
                .flex_1()
                .min_w(px(0.0))
                .child(rich_small(app, &row.note)),
        )
        .into_element()
}

/// The desktop and terminal decide bar (`.decide`): the actions, then the exact command and
/// attribution, or the offline refusal in the failure colour.
fn decide(app: AppState, detail: Memo<Detail>) -> AnyElement {
    dynamic(move || match detail.get() {
        Detail::Awaiting(review) => {
            let actions = review
                .decide
                .iter()
                .map(|button| command(app, button, false))
                .collect();
            let note = match review.refusal {
                Some(refusal) => Some(refused(app, refusal)),
                None if review.attribution.is_empty() => None,
                None => Some(attribution(app, &review.attribution)),
            };
            Some(bar(app, actions, note))
        }
        _ => None,
    })
}

fn refused(app: AppState, refusal: &'static str) -> AnyElement {
    let words = text(refusal).text_color(hue_ink(Hue::Fail));
    if app.form == Form::Terminal {
        return words.number_of_lines(1u32).into_element();
    }
    words
        .text_right()
        .font_family(FONT_TEXT)
        .text_size(px(11.5))
        .line_height(px(15.5))
        .into_element()
}

/// The command a decision runs (`.decide .attr`): 11.5 pt quiet, right-aligned, its mono runs
/// in the mono font. The kit's rich text sets no alignment, and these lines sit flush right.
fn attribution(app: AppState, words: &Rich) -> AnyElement {
    if app.form == Form::Terminal {
        return rich_small(app, words);
    }
    text("")
        .text_right()
        .font_family(FONT_TEXT)
        .text_size(px(11.5))
        .line_height(px(15.5))
        .text_color(color(Token::Ink3))
        .children(words.iter().map(|run| {
            let piece = text(run.text.clone());
            if run.mono {
                piece.font_family(FONT_MONO).text_size(px(11.0))
            } else {
                piece
            }
        }))
        .into_element()
}

/// The approved plan's empty state (`.empty`).
fn approved(app: AppState, detail: Memo<Detail>) -> AnyElement {
    match detail.get_untracked() {
        Detail::Approved(state) => empty(app, state.heading, state.body, None),
        _ => element_view().into_element(),
    }
}
