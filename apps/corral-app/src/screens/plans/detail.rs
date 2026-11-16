//! The selected plan's detail: which variant shows, the head and pipeline every variant
//! shares, the review document with its owner decisions and decide bar, the plan document, the
//! revision diff, and the loading and notice states of a served plan.

use nsh_core::prelude::*;

use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::model::plans::{
    self, ApprovedView, AwaitingView, Decide, Detail, DiffLine, DiffLineKind, DiffView,
    DocumentSection, DocumentView, ExecutingView, Header, NoticeView, PipelineStage, PlanningView,
    ProposedRow, ReviewRow, SectionBody,
};
use crate::model::projects::FilterScreen;
use crate::model::text::span;
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::kit::chip::{chip, illus};
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::notice::{empty, error, loading as loading_notice, notice};
use crate::ui::kit::page::{bar, bar_pane, pane as scroll_pane, qhead};
use crate::ui::kit::text::{rich, rich_small};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

use super::{command, decide, tracker, verbs};

/// Which detail shows, with the content its builder reads untracked. Parts that track their own
/// reads (the decide bar, the graph, the story, typed fields) are blanked here, so a keystroke,
/// a member press or a board change redraws only those parts, not the scroll they sit in.
#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Awaiting(Box<AwaitingView>),
    Executing(Box<ExecutingView>),
    Approved(ApprovedView),
    Document(Box<DocumentView>),
    Diff(Box<DiffView>),
    Loading,
    Notice(NoticeView),
    Planning(Box<PlanningView>),
}

fn blank_decide() -> Decide {
    Decide {
        buttons: vec![],
        compose: None,
        note: None,
        attribution: vec![],
    }
}

fn kind_of(detail: &Detail) -> Kind {
    match detail {
        Detail::Awaiting(review) => {
            let mut review = review.clone();
            review.decide = blank_decide();
            for section in &mut review.sections {
                if let SectionBody::Decisions(rows) = &mut section.body {
                    for row in rows {
                        row.resolution.clear();
                    }
                }
            }
            Kind::Awaiting(Box::new(review))
        }
        Detail::Executing(tracker) => {
            let mut tracker = tracker.clone();
            tracker.members = Default::default();
            tracker.selected = None;
            tracker.story = None;
            Kind::Executing(Box::new(tracker))
        }
        Detail::Approved(state) => Kind::Approved(state.clone()),
        Detail::Document(document) => Kind::Document(Box::new(document.clone())),
        Detail::Diff(diff) => {
            let mut diff = diff.clone();
            diff.decide = diff.decide.map(|_| blank_decide());
            Kind::Diff(Box::new(diff))
        }
        Detail::Loading => Kind::Loading,
        Detail::Notice(view) => Kind::Notice(view.clone()),
        Detail::Planning(view) => Kind::Planning(Box::new(view.clone())),
    }
}

/// The tracked detail of the shown plan, and which variant it is. `plan` names the plan (the
/// phone page's); otherwise the root selection, or the first plan the Plans scope lists, is shown.
fn memos(app: AppState, plan: Option<Id>) -> (Memo<Detail>, Memo<Kind>) {
    let detail = computed(move || {
        let env = app.env();
        let shown = plan.clone().or_else(|| {
            let selection = app.selection();
            let filter = app.project_filter(FilterScreen::Plans);
            app.feed
                .with(|feed| plans::current_plan_filtered(&selection, feed, &filter))
        });
        app.plans.with(|state| {
            app.feed
                .with(|feed| plans::live_detail(state, shown.as_ref(), feed, env))
        })
    });
    let kind = computed(move || detail.with(kind_of));
    (detail, kind)
}

/// The detail column on desktop and in a terminal: the review with its pinned decide bar, the
/// tracker beside its member story, the approved plan's empty state, the document, the diff,
/// or what stands in for a plan not yet received.
pub(super) fn pane(app: AppState) -> AnyElement {
    let (detail, kind) = memos(app, None);
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(dynamic(move || match kind.get() {
            Kind::Awaiting(_) => bar_pane(
                app,
                "plan-detail",
                awaiting_content(app, detail),
                decide::decide_bar(app, detail),
            ),
            Kind::Diff(diff) if diff.decide.is_some() => bar_pane(
                app,
                "plan-detail",
                diff_content(app, detail),
                decide::decide_bar(app, detail),
            ),
            Kind::Diff(_) => scroll_pane(app, "plan-detail", diff_content(app, detail)),
            Kind::Executing(_) => tracker::executing_pane(app, detail),
            Kind::Approved(_) => scroll_pane(app, "plan-detail", approved(app, detail)),
            Kind::Document(_) => scroll_pane(app, "plan-detail", document_page(app, detail)),
            Kind::Loading => scroll_pane(app, "plan-detail", loading(app)),
            Kind::Notice(_) => scroll_pane(app, "plan-detail", notice_view(app, detail)),
            Kind::Planning(_) => scroll_pane(app, "plan-detail", planning(app, detail)),
        }))
        .into_element()
}

/// The pushed phone page's body; the shell scrolls it.
pub(super) fn phone_body(app: AppState, plan: Option<Id>) -> AnyElement {
    let (detail, kind) = memos(app, plan);
    view()
        .test_id("plan-detail")
        .child(dynamic(move || match kind.get() {
            Kind::Awaiting(_) => awaiting_content(app, detail),
            Kind::Executing(_) => tracker::executing_body(app, detail),
            Kind::Approved(_) => approved(app, detail),
            Kind::Document(_) => document_page(app, detail),
            Kind::Diff(_) => diff_content(app, detail),
            Kind::Loading => loading(app),
            Kind::Notice(_) => notice_view(app, detail),
            Kind::Planning(_) => planning(app, detail),
        }))
        .into_element()
}

/// The phone's decide bar (`.pdecide`): Changes and the wide Accept, the refused Accept alone
/// while offline, or the request-changes field with Send and Cancel; a served plan's note sits
/// above it.
pub(super) fn phone_bar(app: AppState, plan: Option<Id>) -> AnyElement {
    let (detail, _) = memos(app, plan);
    let shape = computed(move || detail.with(decide::shape));
    dynamic(move || {
        shape.get().map(|bar_now| {
            view()
                .children(bar_now.note.as_ref().map(|line| {
                    view()
                        .px(px(14.0))
                        .pt(px(8.0))
                        .bg(color(Token::Bg))
                        .border_t(px(1.0))
                        .border_color(color(Token::Rule))
                        .child(decide::note(app, line, false))
                        .into_element()
                }))
                .child(bar(app, decide::actions(app, &bar_now, true), None))
                .into_element()
        })
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
/// the title and its commands, then a stale copy's notice.
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
    let mut actions: Vec<AnyElement> = head
        .actions
        .iter()
        .map(|button| command(app, button, false))
        .collect();
    // A served plan's commands (cancel, add or move a member) follow its own actions.
    actions.extend(head.plan.clone().map(|plan| verbs::head_menu(app, plan)));
    let top = qhead(app, meta, head.title.clone(), None, actions);
    match &head.stale {
        None => top,
        Some(line) => element_view()
            .child(top)
            .child(inset(app).pb(px(0.0)).child(notice(
                app,
                Hue::Held,
                &vec![span(line.as_str())],
                None,
            )))
            .into_element(),
    }
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
/// paragraph, list, proposed members, verdicts or decisions, then the document's path.
fn document(app: AppState, review_doc: &AwaitingView) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    inset(app)
        .child(pipeline(app, &review_doc.pipeline))
        .children(sections(app, &review_doc.sections))
        .when(!review_doc.artifact.is_empty(), |doc| {
            doc.child(
                element_view()
                    .when(!terminal, |v| v.mt(px(10.0)))
                    .when(terminal, |v| v.child(term::blank()))
                    .child(rich_small(app, &review_doc.artifact)),
            )
        })
        .into_element()
}

fn sections(app: AppState, sections: &[DocumentSection]) -> Vec<AnyElement> {
    sections
        .iter()
        .map(|section| doc_section(app, section))
        .collect()
}

/// A served plan's document outside review: its head with the way back, then its sections.
fn document_page(app: AppState, detail: Memo<Detail>) -> AnyElement {
    match detail.get_untracked() {
        Detail::Document(page) => element_view()
            .child(header(app, &page.header))
            .child(inset(app).children(sections(app, &page.sections)))
            .into_element(),
        _ => element_view().into_element(),
    }
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
        SectionBody::Decisions(rows) => element_view()
            .gap(px(if app.form == Form::Terminal {
                0.0
            } else {
                10.0
            }))
            .children(
                rows.iter()
                    .map(|row| decide::decision(app, row))
                    .collect::<Vec<_>>(),
            )
            .into_element(),
    };
    element_view()
        .child(doc_heading(app, section.heading))
        .child(body)
        .into_element()
}

/// A plain bulleted list (`.plan-doc ul`): items 4 pt apart, 18 pt in.
fn bullets(app: AppState, items: &[String]) -> AnyElement {
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
                    let words = text(item.clone())
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
    let name = text(row.name.clone())
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
    let rev = text(row.rev.clone())
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

/// A plan still being drafted or reviewed, or whose planning failed: its head with its state,
/// the pipeline at the design stage, then what is happening and what stands in the way. No
/// decide bar: nothing awaits the owner's acceptance yet.
fn planning(app: AppState, detail: Memo<Detail>) -> AnyElement {
    let Detail::Planning(state) = detail.get_untracked() else {
        return element_view().into_element();
    };
    element_view()
        .child(header(app, &state.header))
        .child(inset(app).pb(px(0.0)).child(pipeline(app, &state.pipeline)))
        .child(empty(app, state.heading, state.body, None))
        .into_element()
}

/// The approved plan: its head (state, id and revision, title) when corrald serves it, the
/// empty state, its tracker and document commands wrapping 8 pt apart (two Cells in a
/// terminal), and a stale copy's notice.
fn approved(app: AppState, detail: Memo<Detail>) -> AnyElement {
    match detail.get_untracked() {
        Detail::Approved(state) => {
            let terminal = app.form == Form::Terminal;
            let actions = (!state.actions.is_empty()).then(|| {
                let mut row = view().flex_row().flex_wrap().items_center();
                for (at, button) in state.actions.iter().enumerate() {
                    if terminal && at > 0 {
                        row = row.child(term::space(2.0));
                    }
                    row = row.child(command(app, button, false));
                }
                let row = if terminal { row } else { row.gap(px(8.0)) };
                row.into_element()
            });
            element_view()
                .children(state.header.as_ref().map(|head| header(app, head)))
                .children(state.stale.as_ref().map(|line| {
                    inset(app)
                        .pb(px(0.0))
                        .child(notice(app, Hue::Held, &vec![span(line.as_str())], None))
                        .into_element()
                }))
                .child(empty(
                    app,
                    state.heading.clone(),
                    state.body.clone(),
                    actions,
                ))
                .into_element()
        }
        _ => element_view().into_element(),
    }
}

/// One diff line in mono: additions in the ok ink, removals in the failure ink, headers quiet.
fn diff_line(app: AppState, line: &DiffLine) -> AnyElement {
    let ink = match line.kind {
        DiffLineKind::Added => hue_ink(Hue::Ok),
        DiffLineKind::Removed => hue_ink(Hue::Fail),
        DiffLineKind::Header => color(Token::Ink3),
        DiffLineKind::Context => color(Token::Ink2),
    };
    let words = text(line.text.clone()).text_color(ink);
    if app.form == Form::Terminal {
        return words.into_element();
    }
    words
        .font_family(FONT_MONO)
        .text_size(px(if app.form == Form::Phone { 12.0 } else { 11.5 }))
        .line_height(px(if app.form == Form::Phone { 17.0 } else { 16.0 }))
        .into_element()
}

/// The revision diff: its head with the earlier revisions to compare, the lines received so
/// far, the loading, error, empty or truncation note, and "Show more" while pages remain.
fn diff_content(app: AppState, detail: Memo<Detail>) -> AnyElement {
    let Detail::Diff(diff) = detail.get_untracked() else {
        return element_view().into_element();
    };
    element_view()
        .child(header(app, &diff.header))
        .child(
            inset(app)
                .child(
                    element_view()
                        .test_id("plan-diff")
                        .role(AccessibilityRole::List)
                        .children(
                            diff.lines
                                .iter()
                                .map(|line| diff_line(app, line))
                                .collect::<Vec<_>>(),
                        ),
                )
                .children(diff.note.as_ref().map(|line| {
                    element_view()
                        .when(app.form != Form::Terminal, |v| v.mt(px(10.0)))
                        .child(decide::note(app, line, false))
                        .into_element()
                }))
                .children(diff.more.as_ref().map(|button| {
                    element_view()
                        .flex_row()
                        .when(app.form != Form::Terminal, |v| v.mt(px(12.0)))
                        .child(command(app, button, false))
                        .into_element()
                })),
        )
        .into_element()
}

/// corrald has not delivered the plan yet: the skeleton, and the caption once the wait is felt.
fn loading(app: AppState) -> AnyElement {
    inset(app)
        .child(loading_notice(
            app,
            plans::DETAIL_LOADING,
            &[
                (0.3, 12.0),
                (0.75, 20.0),
                (0.6, 10.0),
                (0.7, 10.0),
                (0.45, 10.0),
            ],
        ))
        .into_element()
}

/// No plan, an unserved plan, or a failed request with its retry.
fn notice_view(app: AppState, detail: Memo<Detail>) -> AnyElement {
    let Detail::Notice(state) = detail.get_untracked() else {
        return element_view().into_element();
    };
    let action = state
        .action
        .as_ref()
        .map(|button| command(app, button, false));
    if state.hue == Hue::Fail {
        return error(
            app,
            &vec![
                span(state.heading.as_str()).bold().hue(Hue::Fail),
                span(format!(" · {}", state.body)),
            ],
            action,
        );
    }
    empty(app, state.heading, state.body, action)
}
