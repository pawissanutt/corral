//! A served plan: its head with the headline that says what holds it up, its members as a
//! dependency graph of readable cards, its document (the parent task's body), and beside them
//! (or under them) the path to done with the story of a member chosen with J/K or a card press.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::Id;
use crate::model::plans::{Detail, GraphInput, PathToDone, Story};
use crate::model::text::Rich;
use crate::state::AppState;
use crate::ui::kit::hint::hinted;
use crate::ui::kit::page::{Width, columns, pane as scroll_pane};
use crate::ui::kit::text::rich_at;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::detail::{Side, head};
use super::doc::{document, side_block};
use super::{SIDE, graph, path, story};

/// Desktop and terminal: with `Side::Beside`, the plan's scroll beside the side column's;
/// with `Side::Under`, one scroll with the side column under the members.
pub(super) fn pane(app: AppState, detail: Memo<Detail>, side: Side) -> AnyElement {
    match side {
        Side::Beside => columns(
            app,
            vec![
                (
                    Width::Fill,
                    scroll_pane(app, "plan-main", main(app, detail)),
                ),
                (
                    Width::Fixed(SIDE),
                    scroll_pane(app, "plan-side", aside(app, detail)),
                ),
            ],
            0,
        ),
        Side::Under => scroll_pane(
            app,
            "plan-main",
            view()
                .child(main(app, detail))
                .child(under(app, aside(app, detail)))
                .into_element(),
        ),
    }
}

/// The pushed phone page's body: the head with its headline, the path to done first, then the
/// members stacked (a member press opens its task), then the document.
pub(super) fn phone_body(app: AppState, detail: Memo<Detail>) -> AnyElement {
    view()
        .min_w(px(0.0))
        .children(heading(app, detail))
        .child(path_part(app, detail))
        .child(members(app, detail))
        .children(document_part(app, detail))
        .into_element()
}

/// The side column placed under the plan: ruled off from it.
fn under(app: AppState, side: AnyElement) -> AnyElement {
    match app.form {
        Form::Terminal => view().child(term::hrule()).child(side).into_element(),
        Form::Desktop | Form::Phone => view()
            .border_t(px(1.0))
            .border_color(color(Token::Rule))
            .child(side)
            .into_element(),
    }
}

/// The head, its headline following the plan's members.
fn heading(app: AppState, detail: Memo<Detail>) -> Option<AnyElement> {
    let Detail::Plan(plan) = detail.get_untracked() else {
        return None;
    };
    Some(head(app, &plan.header, Some(headline(app, detail))))
}

/// The members first, since where the work stands is what a plan is opened for; the document
/// after them at reading measure.
fn main(app: AppState, detail: Memo<Detail>) -> AnyElement {
    view()
        .min_w(px(0.0))
        .children(heading(app, detail))
        .child(members(app, detail))
        .children(document_part(app, detail))
        .into_element()
}

fn aside(app: AppState, detail: Memo<Detail>) -> AnyElement {
    view()
        .min_w(px(0.0))
        .child(path_part(app, detail))
        .child(story_part(app, detail))
        .into_element()
}

/// The plan's document, ruled off from the members.
fn document_part(app: AppState, detail: Memo<Detail>) -> Option<AnyElement> {
    let Detail::Plan(plan) = detail.get_untracked() else {
        return None;
    };
    Some(document(app, &plan.document))
}

/// What holds the plan up, in its hue, then how far it has come (`.dhead p`); tracked, so a
/// member's change rewords it in place.
fn headline(app: AppState, detail: Memo<Detail>) -> AnyElement {
    let line = computed(move || {
        detail.with(|detail| match detail {
            Detail::Plan(plan) => plan.brief.headline.clone(),
            _ => Rich::new(),
        })
    });
    let size = match app.form {
        Form::Phone => 14.5,
        Form::Desktop | Form::Terminal => 13.5,
    };
    dynamic(move || {
        let words = line.get();
        (!words.is_empty()).then(|| rich_at(app, &words, size, Token::Ink2))
    })
    .into_element()
}

/// The members block (`.blk`): its caption with what the layout means, then the graph, or
/// what stands in for it while the plan has no member.
fn members(app: AppState, detail: Memo<Detail>) -> AnyElement {
    let Detail::Plan(plan) = detail.get_untracked() else {
        return view().into_element();
    };
    let form = app.form;
    let caption = text(plan.members_heading)
        .role(AccessibilityRole::Header)
        .number_of_lines(1u32)
        .font_weight(FontWeight::SEMIBOLD);
    let caption = if form == Form::Terminal {
        caption
            .font_weight(FontWeight::BOLD)
            .text_color(color(Token::Ink))
    } else {
        caption
            .font_family(FONT_TEXT)
            .text_size(px(if form == Form::Phone { 13.0 } else { 12.0 }))
            .text_color(color(Token::Ink3))
    };
    let empty = computed(move || {
        detail.with(|detail| match detail {
            Detail::Plan(plan) => plan.brief.graph.members.is_empty(),
            _ => true,
        })
    });
    let none = plan.members_empty;
    let block = match form {
        Form::Desktop => view().px(px(20.0)).py(px(16.0)),
        Form::Phone => view().px(px(18.0)).py(px(14.0)),
        Form::Terminal => view().px(term::cells(1.0)).pt(term::rows(1.0)),
    };
    block
        .min_w(px(0.0))
        .child(
            view()
                .flex_row()
                .when(form != Form::Terminal, |v| v.mb(px(10.0)))
                .child(hinted(app, caption, plan.members_hint)),
        )
        .child(dynamic(move || {
            if empty.get() {
                text(none)
                    .text_color(color(Token::Ink3))
                    .when(form != Form::Terminal, |t| {
                        t.font_family(FONT_TEXT).text_size(px(13.0))
                    })
                    .into_element()
            } else {
                untracked(|| graph::drawing(app, move || graph_of(detail)))
            }
        }))
        .into_element()
}

/// The members the model resolved and the selected one, tracked: the graph redraws for a
/// member press or a board change without rebuilding the plan around it.
fn graph_of(detail: Memo<Detail>) -> (GraphInput, Option<Id>) {
    detail.with(|detail| match detail {
        Detail::Plan(plan) => (plan.brief.graph.clone(), plan.selected.clone()),
        _ => (GraphInput::default(), None),
    })
}

/// The path to done, tracked: it moves with the members.
fn path_part(app: AppState, detail: Memo<Detail>) -> AnyElement {
    let shown = computed(move || {
        detail.with(|detail| match detail {
            Detail::Plan(plan) => Some(plan.brief.path.clone()),
            _ => None,
        })
    });
    dynamic(move || {
        let path: Option<PathToDone> = shown.get();
        path.map(|path| path::path(app, &path))
    })
    .into_element()
}

/// The selected member's story under the path, tracked: J/K or a card press swaps it in place.
fn story_part(app: AppState, detail: Memo<Detail>) -> AnyElement {
    let shown = computed(move || {
        detail.with(|detail| match detail {
            Detail::Plan(plan) => plan.story.clone(),
            _ => None,
        })
    });
    dynamic(move || {
        let chosen: Option<Story> = shown.get();
        chosen.map(|chosen| {
            side_block(app)
                .child(story::story(app, &chosen))
                .into_element()
        })
    })
    .into_element()
}
