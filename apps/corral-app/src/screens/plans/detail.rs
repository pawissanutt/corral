//! The selected plan's detail: which variant shows, the head every variant shares (`.dhead`),
//! the document awaiting acceptance with what accepting publishes and its pinned decide bar,
//! the planning, approved, document and diff states, and the loading and notice states of a
//! served plan. The executing brief is `executing.rs`.

use nsh_core::prelude::*;

use crate::model::board::Mark;
use crate::model::hue::Hue;
use crate::model::plans::{
    self, ApprovedView, AwaitingView, Decide, Detail, DiffLine, DiffLineKind, DiffView,
    DocumentView, ExecutingView, Header, NoticeView, PipelineStage, PlanningView, SectionBody,
};
use crate::model::projects::FilterScreen;
use crate::model::text::{flat, span};
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::kit::chip::{chip, illus};
use crate::ui::kit::hint::hinted;
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::notice::{empty, error, loading as loading_notice, notice};
use crate::ui::kit::page::{Width, bar, bar_pane, columns, pane as scroll_pane};
use crate::ui::kit::text::rich_small;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::doc::{awaiting_side, doc_inset, sections};
use super::{SIDE, command, decide, executing, verbs};

/// Where the side column of a plan stands: beside it, or under it in the same scroll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Side {
    Beside,
    Under,
}

/// Which detail shows, with the content its builder reads untracked. Parts that track their own
/// reads (the decide bar, the graph, the headline, the path, the story, typed fields) are
/// blanked here, so a keystroke, a member press or a board change redraws only those parts,
/// not the scroll they sit in.
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
            review.brief.decide = blank_decide();
            for section in &mut review.sections {
                if let SectionBody::Decisions(rows) = &mut section.body {
                    for row in rows {
                        row.resolution.clear();
                    }
                }
            }
            Kind::Awaiting(Box::new(review))
        }
        Detail::Executing(plan) => {
            let mut plan = plan.clone();
            plan.brief.headline.clear();
            plan.brief.graph = Default::default();
            plan.brief.path.steps.clear();
            plan.brief.path.current = None;
            plan.selected = None;
            plan.story = None;
            Kind::Executing(Box::new(plan))
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

/// The plan on desktop and in a terminal, with its side column `side`: the executing brief,
/// the document with its pinned decide bar, the approved, planning, document or diff state,
/// or what stands in for a plan not yet received. test id "plan-detail"; the plan's scroll
/// "plan-main", a side column beside it "plan-side".
pub(super) fn pane(app: AppState, side: Side) -> AnyElement {
    let (detail, kind) = memos(app, None);
    view()
        .test_id("plan-detail")
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(dynamic(move || {
            let shown = kind.get();
            untracked(move || match shown {
                Kind::Awaiting(_) => awaiting_pane(app, detail, side),
                Kind::Diff(diff) if diff.decide.is_some() => bar_pane(
                    app,
                    "plan-main",
                    diff_content(app, detail),
                    decide::decide_bar(app, detail),
                ),
                Kind::Diff(_) => scroll_pane(app, "plan-main", diff_content(app, detail)),
                Kind::Executing(_) => executing::pane(app, detail, side),
                Kind::Approved(_) => scroll_pane(app, "plan-main", approved(app, detail)),
                Kind::Document(_) => scroll_pane(app, "plan-main", document_page(app, detail)),
                Kind::Loading => scroll_pane(app, "plan-main", loading(app)),
                Kind::Notice(_) => scroll_pane(app, "plan-main", notice_view(app, detail)),
                Kind::Planning(_) => scroll_pane(app, "plan-main", planning(app, detail)),
            })
        }))
        .into_element()
}

/// The pushed phone page's body; the shell scrolls it.
pub(super) fn phone_body(app: AppState, plan: Option<Id>) -> AnyElement {
    let (detail, kind) = memos(app, plan);
    view()
        .test_id("plan-detail")
        .child(dynamic(move || {
            let shown = kind.get();
            untracked(move || match shown {
                Kind::Awaiting(_) => view()
                    .child(awaiting_main(app, detail))
                    .child(awaiting_brief(app, detail))
                    .into_element(),
                Kind::Executing(_) => executing::phone_body(app, detail),
                Kind::Approved(_) => approved(app, detail),
                Kind::Document(_) => document_page(app, detail),
                Kind::Diff(_) => diff_content(app, detail),
                Kind::Loading => loading(app),
                Kind::Notice(_) => notice_view(app, detail),
                Kind::Planning(_) => planning(app, detail),
            })
        }))
        .into_element()
}

/// The phone's decide bar (`.pdecide`): Accept plan wide and Request changes, refused ones
/// disabled, or the request-changes field with Send and Cancel; a served plan's note sits
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

/// Room around a state's content (planning, approved, diff): 16 20 on desktop, 14 18 on the
/// phone, a Cell each side in a terminal.
fn inset(app: AppState) -> View {
    match app.form {
        Form::Desktop => view().min_w(px(0.0)).px(px(20.0)).py(px(16.0)),
        Form::Phone => view().min_w(px(0.0)).px(px(18.0)).py(px(14.0)),
        Form::Terminal => view().min_w(px(0.0)).px(term::cells(1.0)),
    }
}

/// The head of a plan (`.dhead`): its state chip (whose hint on desktop is what the head knows
/// of it: accepted, delivered to, design or tracker), any further chips, the id, the project
/// and the commands on the first line; the title (test id "plan-title"); `lead` (the executing
/// plan's headline); on the phone and in a terminal, which have no hover, the state's facts
/// written out; then a stale copy's notice. Desktop padding 16 20 14, title 19 pt; phone 14 18
/// 16, title 20 pt; terminal a Cell in, a joined rule under it.
pub(super) fn head(app: AppState, head: &Header, lead: Option<AnyElement>) -> AnyElement {
    let form = app.form;
    let terminal = form == Form::Terminal;
    let facts = flat(&head.meta);
    let mut first: Vec<AnyElement> = head
        .chips
        .iter()
        .enumerate()
        .map(|(at, state)| {
            let drawn = chip(app, state.label.clone(), state.hue);
            if at == 0 && form == Form::Desktop {
                hinted(app, drawn, facts.clone())
            } else {
                drawn
            }
        })
        .collect();
    let id = text(head.id.clone())
        .flex_shrink_0()
        .number_of_lines(1u32)
        .text_color(color(Token::Ink3));
    first.push(if terminal {
        id.into_element()
    } else {
        id.font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(if form == Form::Phone { 13.0 } else { 12.0 }))
            .into_element()
    });
    first.extend(
        head.project
            .clone()
            .map(|project| chip(app, project, Hue::Neutral)),
    );
    first.extend(head.illustrative.map(|label| illus(app, label)));
    let mut actions: Vec<AnyElement> = head
        .actions
        .iter()
        .map(|button| command(app, button, false))
        .collect();
    // A served plan's commands (cancel, add or move a member) follow its own actions.
    actions.extend(head.plan.clone().map(|plan| verbs::head_menu(app, plan)));
    let gap = if terminal { term::cells(1.0) } else { px(8.0) };
    let actions = (!actions.is_empty()).then(|| {
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(gap)
            .children(actions)
            .into_element()
    });
    let title = text(head.title.clone())
        .test_id("plan-title")
        .role(AccessibilityRole::Header)
        .min_w(px(0.0))
        .text_color(color(Token::Ink));
    let title = match form {
        Form::Terminal => title.font_weight(FontWeight::BOLD),
        Form::Desktop => title
            .max_w(px(560.0))
            .mt(px(6.0))
            .mb(px(4.0))
            .font_family(FONT_TEXT)
            .text_size(px(19.0))
            .line_height(px(25.0))
            .letter_spacing(px(-0.19))
            .font_weight(FontWeight::SEMIBOLD),
        Form::Phone => title
            .mt(px(8.0))
            .mb(px(4.0))
            .font_family(FONT_TEXT)
            .text_size(px(20.0))
            .line_height(px(25.0))
            .font_weight(FontWeight::BOLD),
    };
    let line = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_x(gap)
        .gap_y(px(6.0))
        .children(first);
    // Desktop keeps the commands at the first line's end; the phone and a terminal give them a
    // line of their own under the head, where they wrap rather than squeeze the title.
    let (line, below) = if form == Form::Desktop {
        (
            line.when_some(actions, |line, actions| {
                line.child(view().flex_1()).child(actions)
            }),
            None,
        )
    } else {
        (line, actions)
    };
    let written =
        (form != Form::Desktop && !head.meta.is_empty()).then(|| rich_small(app, &head.meta));
    let body = view()
        .flex_none()
        .min_w(px(0.0))
        .child(line)
        .child(title)
        .children(lead)
        .children(written)
        .children(below.map(|below| {
            view()
                .when(!terminal, |v| v.mt(px(12.0)))
                .child(below)
                .into_element()
        }));
    let framed = match form {
        Form::Desktop => body
            .px(px(20.0))
            .pt(px(16.0))
            .pb(px(14.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .into_element(),
        Form::Phone => body
            .px(px(18.0))
            .pt(px(14.0))
            .pb(px(16.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .into_element(),
        Form::Terminal => view()
            .flex_none()
            .child(body.px(term::cells(1.0)))
            .child(term::hrule())
            .into_element(),
    };
    match &head.stale {
        None => framed,
        Some(line) => view()
            .min_w(px(0.0))
            .child(framed)
            .child(inset(app).pb(px(0.0)).child(notice(
                app,
                Hue::Held,
                &vec![span(line.as_str())],
                None,
            )))
            .into_element(),
    }
}

/// The awaiting plan: its document, and what accepting publishes and what the reviewers said
/// beside it or under it; the decide bar pinned under the document.
fn awaiting_pane(app: AppState, detail: Memo<Detail>, side: Side) -> AnyElement {
    match side {
        Side::Beside => columns(
            app,
            vec![
                (
                    Width::Fill,
                    bar_pane(
                        app,
                        "plan-main",
                        awaiting_main(app, detail),
                        decide::decide_bar(app, detail),
                    ),
                ),
                (
                    Width::Fixed(SIDE),
                    scroll_pane(app, "plan-side", awaiting_brief(app, detail)),
                ),
            ],
            0,
        ),
        Side::Under => bar_pane(
            app,
            "plan-main",
            view()
                .child(awaiting_main(app, detail))
                .child(
                    view()
                        .when(app.form != Form::Terminal, |v| {
                            v.border_t(px(1.0)).border_color(color(Token::Rule))
                        })
                        .when(app.form == Form::Terminal, |v| v.child(term::hrule()))
                        .child(awaiting_brief(app, detail)),
                )
                .into_element(),
            decide::decide_bar(app, detail),
        ),
    }
}

/// The awaiting plan's head over its document at reading measure.
fn awaiting_main(app: AppState, detail: Memo<Detail>) -> AnyElement {
    match detail.get_untracked() {
        Detail::Awaiting(review) => view()
            .min_w(px(0.0))
            .child(head(app, &review.header, None))
            .child(doc_inset(app).child(sections(app, &review.sections)))
            .into_element(),
        _ => view().into_element(),
    }
}

/// What accepting publishes and what the reviewers said.
fn awaiting_brief(app: AppState, detail: Memo<Detail>) -> AnyElement {
    match detail.get_untracked() {
        Detail::Awaiting(review) => awaiting_side(app, &review.brief),
        _ => view().into_element(),
    }
}

/// A served plan's document outside review: its head with the way back, then its sections.
fn document_page(app: AppState, detail: Memo<Detail>) -> AnyElement {
    match detail.get_untracked() {
        Detail::Document(page) => view()
            .min_w(px(0.0))
            .child(head(app, &page.header, None))
            .child(doc_inset(app).child(sections(app, &page.sections)))
            .into_element(),
        _ => view().into_element(),
    }
}

/// The design stages of a plan not yet accepted, on a line: each stage's mark and label, the
/// active one in ink; a terminal joins them with `›` (a lone `─` would be read as a divider).
fn pipeline(app: AppState, stages: &[PipelineStage]) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let mut parts = Vec::with_capacity(stages.len() * 2);
    for (at, stage) in stages.iter().enumerate() {
        if at > 0 {
            parts.push(if terminal {
                text(" › ")
                    .number_of_lines(1u32)
                    .text_color(color(Token::Ink3))
                    .into_element()
            } else {
                view()
                    .w(px(18.0))
                    .h(px(1.0))
                    .bg(color(Token::Rule2))
                    .into_element()
            });
        }
        let on = stage.mark == Mark::Active;
        let label = text(stage.label.clone())
            .number_of_lines(1u32)
            .text_color(color(if on || terminal {
                Token::Ink
            } else {
                Token::Ink3
            }))
            .when(on, |t| t.font_weight(FontWeight::SEMIBOLD));
        let label = if terminal {
            label
        } else {
            label.font_family(FONT_TEXT).text_size(px(12.5))
        };
        parts.push(
            view()
                .flex_row()
                .items_center()
                .flex_shrink_0()
                .gap(if terminal { term::cells(1.0) } else { px(6.0) })
                .accessible(true)
                .label(format!("{}, {}", stage.label, stage.mark.label()))
                .child(mark(app, stage.mark, stage.hue))
                .child(label)
                .into_element(),
        );
    }
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_x(if terminal { px(0.0) } else { px(8.0) })
        .gap_y(px(4.0))
        .role(AccessibilityRole::List)
        .children(parts)
        .into_element()
}

/// A plan still being drafted or reviewed, or whose planning failed: its head, the design
/// stages, then what is happening and what stands in the way. No decide bar: nothing awaits the
/// owner's acceptance yet.
fn planning(app: AppState, detail: Memo<Detail>) -> AnyElement {
    let Detail::Planning(state) = detail.get_untracked() else {
        return view().into_element();
    };
    view()
        .min_w(px(0.0))
        .child(head(app, &state.header, None))
        .child(inset(app).pb(px(0.0)).child(pipeline(app, &state.pipeline)))
        .child(empty(app, state.heading, state.body, None))
        .into_element()
}

/// The approved plan: its head when corrald serves it, the empty state with its tracker and
/// document commands wrapping 8 pt apart (two Cells in a terminal), and a stale copy's notice.
fn approved(app: AppState, detail: Memo<Detail>) -> AnyElement {
    let Detail::Approved(state) = detail.get_untracked() else {
        return view().into_element();
    };
    let terminal = app.form == Form::Terminal;
    let actions = (!state.actions.is_empty()).then(|| {
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(if terminal { term::cells(2.0) } else { px(8.0) })
            .children(
                state
                    .actions
                    .iter()
                    .map(|button| command(app, button, false))
                    .collect::<Vec<_>>(),
            )
            .into_element()
    });
    view()
        .min_w(px(0.0))
        .children(state.header.as_ref().map(|header| head(app, header, None)))
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
        return view().into_element();
    };
    view()
        .min_w(px(0.0))
        .child(head(app, &diff.header, None))
        .child(
            inset(app)
                .child(
                    view()
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
                    view()
                        .when(app.form != Form::Terminal, |v| v.mt(px(10.0)))
                        .child(decide::note(app, line, false))
                        .into_element()
                }))
                .children(diff.more.as_ref().map(|button| {
                    view()
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
        return view().into_element();
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
