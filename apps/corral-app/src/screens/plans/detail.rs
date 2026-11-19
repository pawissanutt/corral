//! The selected plan's detail: which variant shows, the head (`.dhead`) with the plan's
//! Actions menu, and the loading and notice states of a served plan. The plan itself (members,
//! document, path to done, story) is `overview.rs`.

use nsh_core::prelude::*;

use crate::model::hue::Hue;
use crate::model::plans::{self, Detail, Header};
use crate::model::projects::FilterScreen;
use crate::model::text::{flat, span};
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::kit::chip::chip;
use crate::ui::kit::hint::hinted;
use crate::ui::kit::notice::{empty, error, loading as loading_notice, notice};
use crate::ui::kit::page::pane as scroll_pane;
use crate::ui::kit::text::rich_small;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::{command, overview, verbs};

/// Where the side column of a plan stands: beside it, or under it in the same scroll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Side {
    Beside,
    Under,
}

/// Which detail shows, with the content its builder reads untracked. Parts that track their own
/// reads (the graph, the headline, the path, the story) are blanked here, so a member press or a
/// board change redraws only those parts, not the scroll they sit in.
#[derive(Debug, Clone, PartialEq)]
enum Kind {
    Plan(Box<plans::PlanView>),
    Loading,
    Notice(plans::NoticeView),
}

fn kind_of(detail: &Detail) -> Kind {
    match detail {
        Detail::Plan(plan) => {
            let mut plan = plan.clone();
            plan.brief.headline.clear();
            plan.brief.graph = Default::default();
            plan.brief.path.steps.clear();
            plan.brief.path.current = None;
            plan.selected = None;
            plan.story = None;
            // Whether the document folds stays; whether it is open is the document's own part.
            if let Some(fold) = &mut plan.document_fold {
                fold.open = false;
                fold.toggle.label.clear();
            }
            Kind::Plan(plan)
        }
        Detail::Loading => Kind::Loading,
        Detail::Notice(view) => Kind::Notice(view.clone()),
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

/// The plan on desktop and in a terminal, with its side column `side`, or what stands in for a
/// plan not yet received. test id "plan-detail"; the plan's scroll "plan-main", a side column
/// beside it "plan-side".
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
                Kind::Plan(_) => overview::pane(app, detail, side),
                Kind::Loading => scroll_pane(app, "plan-main", loading(app)),
                Kind::Notice(_) => scroll_pane(app, "plan-main", notice_view(app, detail)),
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
                Kind::Plan(_) => overview::phone_body(app, detail),
                Kind::Loading => loading(app),
                Kind::Notice(_) => notice_view(app, detail),
            })
        }))
        .into_element()
}

/// Room around a state's content: 16 20 on desktop, 14 18 on the phone, a Cell each side in a
/// terminal.
fn inset(app: AppState) -> View {
    match app.form {
        Form::Desktop => view().min_w(px(0.0)).px(px(20.0)).py(px(16.0)),
        Form::Phone => view().min_w(px(0.0)).px(px(18.0)).py(px(14.0)),
        Form::Terminal => view().min_w(px(0.0)).px(term::cells(1.0)),
    }
}

/// The head of a plan (`.dhead`): its status chip (whose hint on desktop is when it last
/// changed), the id, the project, Ask and the plan's Actions menu on the first line; the title
/// (test id "plan-title"); `lead` (the headline); on the phone and in a terminal, which have no
/// hover, the facts written out; then a stale copy's notice. Desktop padding 16 20 14, title
/// 19 pt; phone 14 18 16, title 20 pt; terminal a Cell in, a joined rule under it.
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
    let mut actions: Vec<AnyElement> = head
        .actions
        .iter()
        .map(|button| command(app, button, false))
        .collect();
    actions.push(verbs::head_menu(app, head.plan.clone()));
    let gap = if terminal { term::cells(1.0) } else { px(8.0) };
    let actions = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(gap)
        .children(actions)
        .into_element();
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
        (line.child(view().flex_1()).child(actions), None)
    } else {
        (line, Some(actions))
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
