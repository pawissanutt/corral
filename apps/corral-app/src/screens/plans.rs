//! Plans: the grouped plan list beside the selected plan's detail (desktop and terminal), the
//! Plans tab and the pushed plan page (phone), and the dependency graph Now's panel shows too.
//!
//! Everything shown comes from `model::plans`: `list` for the rows, `detail` for the review
//! document, tracker, approved empty state or 64-member strips, `phone_page` for the pushed
//! page and `layout` for the graph. Every press sends the row's or button's `Action` to
//! `plans::State::reduce` through `AppState::act`; the keys are bound by the app state.

mod detail;
mod graph;
mod story;
mod tracker;

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::plans::{
    self, Action, ButtonKind as Weight, ListSection, MemberSquare, PlanRow, SquareState,
};
use crate::model::text::flat;
use crate::screens::PhonePage;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::illus;
use crate::ui::kit::marks::{dot, hue_ink};
use crate::ui::kit::page::{Width, columns, group_head, head, pane, search};
use crate::ui::kit::rows::list_row;
use crate::ui::kit::text::rich_small;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

const RING: f32 = 2.0;

/// The Plans destination. Desktop: the 380 pt list beside the detail. Terminal: the same two
/// panes, the detail alone on a narrow grid (rows select in place, so the detail is what a
/// key press changes). Phone: the large title over the scrolling list; a row pushes its plan.
pub fn screen(app: AppState) -> AnyElement {
    match app.form {
        Form::Phone => view()
            .flex_1()
            .flex_col()
            .min_h(px(0.0))
            .child(dynamic(move || {
                let env = app.env();
                let title = app.plans.with(|state| plans::list(state, env).title);
                head(app, title, None, vec![search(app)])
            }))
            .child(pane(app, "plans-list", list(app)))
            .into_element(),
        Form::Desktop | Form::Terminal => columns(
            app,
            vec![
                (Width::Fixed(380.0), pane(app, "plans-list", list(app))),
                (Width::Fill, detail::pane(app)),
            ],
            1,
        ),
    }
}

/// The pushed plan page on the phone (`Page::Plan`): the model's title and back label, the
/// detail as the body, the Ask control for an executing tracker, and the decide bar in place of
/// the tab bar while the plan awaits acceptance.
pub fn plan_phone(app: AppState) -> PhonePage {
    let env = app.env();
    let page = app.plans.with(|state| plans::phone_page(state, env));
    let awaiting = matches!(page.detail, plans::Detail::Awaiting(_));
    let trailing = page
        .right
        .zip(page.right_label)
        .map(|(button, label)| icon_command(app, &button, label));
    PhonePage {
        title: page.title.to_owned(),
        back: page.back_label,
        body: detail::phone_body(app),
        trailing,
        bar: awaiting.then(|| detail::phone_bar(app)),
        ask: true,
        scroll: true,
    }
}

/// The plan graph alone, measured to its container: cards and dependency curves where every
/// layer fits, vertical layers otherwise (always in a terminal). A card selects its member, a
/// needs-you member opens its question, a vertical row opens its task.
pub fn graph(app: AppState, plan: &'static str, selected: &'static str) -> AnyElement {
    graph::drawing(app, plan, move || selected)
}

/// Runs one Plans action through the reducer and the app's effects.
fn press(app: AppState, action: Action) {
    app.act(app.plans, move |state, env| state.reduce(action, env));
}

/// The test id of a press target, from its action.
fn target_id(action: Action) -> String {
    match action {
        Action::SelectPlan(id) | Action::PhonePlan(id) => format!("plan-{id}"),
        Action::SelectMember(id) | Action::OpenTask(id) => format!("task-{id}"),
        Action::Question => "btn-answer".into(),
        Action::Accept => "btn-accept".into(),
        Action::RequestChanges => "btn-request-changes".into(),
        Action::Diff => "btn-diff".into(),
        Action::OpenDocument => "btn-open-document".into(),
        Action::Ask(id) => format!("btn-ask-{id}"),
        Action::OpenSession(id) => format!("btn-open-session-{id}"),
        Action::ReviewCandidate => "btn-review-candidate".into(),
    }
}

/// A model button as a kit button: standard is the filled main action, ghost outlined, quiet
/// bare; a refused command is disabled. Ask and session commands lead with the chat icon.
fn command(app: AppState, button: &plans::Button, wide: bool) -> AnyElement {
    let action = button.action;
    let kind = match button.kind {
        Weight::Standard => ButtonKind::Primary,
        Weight::Ghost => ButtonKind::Ghost,
        Weight::Quiet => ButtonKind::Quiet,
    };
    let glyph = matches!(action, Action::Ask(_) | Action::OpenSession(_)).then_some(Icon::Ask);
    button_with(
        app,
        button.label,
        kind,
        ButtonOptions {
            icon: glyph,
            disabled: !button.enabled,
            wide,
            ..ButtonOptions::default()
        },
        target_id(action),
        move || press(app, action),
    )
}

/// An icon-only control in the phone's navigation bar (`.pnav .rt button`): 36 by 44 pt, the
/// chat icon in ink-2, named by `label`.
fn icon_command(app: AppState, button: &plans::Button, label: &'static str) -> AnyElement {
    let action = button.action;
    view()
        .flex_row()
        .items_center()
        .justify_center()
        .min_w(px(36.0))
        .min_h(px(44.0))
        .rounded(px(10.0))
        .role(AccessibilityRole::Button)
        .label(label)
        .test_id(&target_id(action))
        .cursor_pointer()
        .active(|s| s.opacity(0.6))
        .focus_visible(|s| {
            s.outline(px(RING))
                .outline_color(color(Token::Focus))
                .outline_offset(px(-RING))
        })
        .on_press(move || press(app, action))
        .child(icon(Icon::Ask, 18.0, color(Token::Ink2)))
        .into_element()
}

/// The fill of a member square (`.sq i`): landed ink-3, waiting rule-2, otherwise its hue.
fn square_fill(state: SquareState, hue: Hue) -> AdaptiveColor {
    match state {
        SquareState::Landed => color(Token::Ink3),
        SquareState::Waiting => color(Token::Rule2),
        _ => hue_ink(hue),
    }
}

/// A terminal square: shaped like the actor dots, so the strip reads without colour.
fn square_glyph(state: SquareState, hue: Hue) -> (&'static str, AdaptiveColor) {
    match state {
        SquareState::Landed => ("■", color(Token::Ink)),
        SquareState::Waiting => ("□", color(Token::Ink3)),
        SquareState::You => ("●", hue_ink(hue)),
        SquareState::Held => ("◆", hue_ink(hue)),
        SquareState::Active => ("◐", hue_ink(hue)),
    }
}

/// A member strip (`.sq`): one 7 by 12 pt square per member, 2 pt apart, in member order;
/// a terminal shows one glyph per member. Hidden from screen readers: the row says it.
fn squares(app: AppState, members: &[MemberSquare]) -> AnyElement {
    if app.form == Form::Terminal {
        let mut line = text("")
            .flex_shrink_0()
            .number_of_lines(1u32)
            .accessibility_elements_hidden(true);
        for square in members {
            let (glyph, ink) = square_glyph(square.state, square.hue);
            line = line.child(text(glyph).text_color(ink));
        }
        return line.into_element();
    }
    view()
        .flex_row()
        .flex_shrink_0()
        .items_center()
        .gap(px(2.0))
        .accessibility_elements_hidden(true)
        .children(members.iter().map(|square| {
            view()
                .w(px(7.0))
                .h(px(12.0))
                .rounded(px(1.5))
                .bg(square_fill(square.state, square.hue))
                .into_element()
        }))
        .into_element()
}

/// The grouped list (`.list`): each section's head in its hue with its count, then its rows.
fn list(app: AppState) -> AnyElement {
    dynamic(move || {
        let env = app.env();
        let content = app.plans.with(|state| plans::list(state, env));
        let top = if app.form == Form::Desktop { 6.0 } else { 0.0 };
        view()
            .pt(px(top))
            .pb(px(if app.form == Form::Terminal {
                0.0
            } else {
                14.0
            }))
            .children(
                content
                    .sections
                    .iter()
                    .map(|section| group(app, section))
                    .collect::<Vec<_>>(),
            )
            .into_element()
    })
}

fn group(app: AppState, section: &ListSection) -> AnyElement {
    let trailing = section.illustrative.map(|label| illus(app, label));
    view()
        .child(group_head(
            app,
            section.label,
            section.count.map(|n| n.to_string()),
            section.hue,
            None,
            trailing,
        ))
        .children(
            section
                .rows
                .iter()
                .map(|row| plan_row(app, row))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// What a screen reader hears for a plan row: id, title, the sub line and the progress.
fn row_label(row: &PlanRow) -> String {
    let mut parts = vec![format!("{} {}", row.id_label, row.title)];
    let sub = flat(&row.sub);
    if !sub.is_empty() {
        parts.push(sub);
    }
    parts.extend(row.progress.clone());
    parts.extend(row.illustrative.map(str::to_owned));
    parts.join(", ")
}

fn plan_row(app: AppState, row: &PlanRow) -> AnyElement {
    let action = row.action;
    let selected = row.selected;
    match app.form {
        Form::Desktop => desktop_row(app, row),
        Form::Phone => list_row(
            app,
            row_label(row),
            target_id(action),
            move || selected,
            move || press(app, action),
            phone_cells(app, row),
        ),
        Form::Terminal => list_row(
            app,
            row_label(row),
            target_id(action),
            move || selected,
            move || press(app, action),
            terminal_cells(app, row),
        ),
    }
}

/// The title line of a row: the title cut to fit and the synthetic tag after it.
fn title_line(app: AppState, row: &PlanRow, size: f32, weight: FontWeight) -> AnyElement {
    let words = text(row.title)
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .truncate()
        .font_weight(weight)
        .text_color(color(Token::Ink));
    let words = if app.form == Form::Terminal {
        words
    } else {
        words.font_family(FONT_TEXT).text_size(px(size))
    };
    view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .min_w(px(0.0))
        .child(words)
        .children(row.illustrative.map(|label| illus(app, label)))
        .into_element()
}

fn progress(app: AppState, row: &PlanRow, size: f32) -> Option<AnyElement> {
    row.progress.clone().map(|figures| {
        let words = text(figures)
            .flex_shrink_0()
            .number_of_lines(1u32)
            .text_color(color(Token::Ink3));
        if app.form == Form::Terminal {
            words.into_element()
        } else {
            words
                .font_family(FONT_MONO)
                .tabular_nums()
                .text_size(px(size))
                .into_element()
        }
    })
}

/// The sub line: the member strip, then the model's sentence.
fn sub_line(app: AppState, row: &PlanRow) -> Option<AnyElement> {
    if row.members.is_empty() && row.sub.is_empty() {
        return None;
    }
    Some(
        view()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .min_w(px(0.0))
            .when(!row.members.is_empty(), |line| {
                line.child(squares(app, &row.members))
            })
            .when(!row.sub.is_empty(), |line| {
                line.child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(rich_small(app, &row.sub)),
                )
            })
            .into_element(),
    )
}

/// The desktop row (`.row` in the list's `14px 54px 1fr auto` grid): two lines, at least
/// 40 pt high, padding 6 14 6 16; selected the sel fill with the 1 pt ink bar at the edge.
/// The kit's list row is one fixed 30 pt line, so the list draws its own two-line row.
fn desktop_row(app: AppState, row: &PlanRow) -> AnyElement {
    let action = row.action;
    let selected = row.selected;
    view()
        .relative()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(px(8.0))
        .min_h(px(40.0))
        .pl(px(16.0))
        .pr(px(14.0))
        .py(px(6.0))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(row_label(row))
        .accessibility_state(AccessibilityState {
            selected,
            ..AccessibilityState::default()
        })
        .test_id(&target_id(action))
        .on_press(move || press(app, action))
        .when(selected, |r| r.bg(color(Token::Sel)))
        .hover(|s| s.bg(color(Token::Hover)))
        .focus_visible(|s| {
            s.outline(px(RING))
                .outline_color(color(Token::Focus))
                .outline_offset(px(-RING))
        })
        .when(selected, |r| {
            r.child(
                view()
                    .absolute()
                    .left(px(0.0))
                    .top(px(4.0))
                    .bottom(px(4.0))
                    .w(px(1.0))
                    .bg(color(Token::Ink)),
            )
        })
        .child(view().w(px(14.0)).flex_shrink_0().child(dot(app, row.hue)))
        .child(
            view().w(px(54.0)).flex_shrink_0().child(
                text(row.id_label.clone())
                    .font_family(FONT_MONO)
                    .tabular_nums()
                    .text_size(px(11.5))
                    .line_height(px(17.0))
                    .text_color(color(Token::Ink3)),
            ),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .gap(px(1.0))
                .child(title_line(app, row, 13.0, FontWeight::NORMAL))
                .children(sub_line(app, row)),
        )
        .children(progress(app, row, 11.0))
        .into_element()
}

/// The phone row (`.prow`): the dot, then `id · title` and the progress, and under them the
/// strip and the sub line.
fn phone_cells(app: AppState, row: &PlanRow) -> Vec<AnyElement> {
    let first = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_baseline()
                .gap(px(8.0))
                .child(
                    text(row.id_label.clone())
                        .flex_shrink_0()
                        .number_of_lines(1u32)
                        .font_family(FONT_MONO)
                        .tabular_nums()
                        .text_size(px(13.0))
                        .text_color(color(Token::Ink3)),
                )
                .child(view().flex_1().min_w(px(0.0)).child(title_line(
                    app,
                    row,
                    16.0,
                    FontWeight::SEMIBOLD,
                ))),
        )
        .children(progress(app, row, 12.0));
    vec![
        view()
            .w(px(12.0))
            .flex_shrink_0()
            .self_start()
            .pt(px(5.0))
            .child(dot(app, row.hue))
            .into_element(),
        view()
            .flex_1()
            .min_w(px(0.0))
            .gap(px(2.0))
            .child(first)
            .children(sub_line(app, row))
            .into_element(),
    ]
}

/// One line of Cells: dot, id, title, strip, progress. The sub line is left to the detail,
/// which states the unmapped criteria in full.
fn terminal_cells(app: AppState, row: &PlanRow) -> Vec<AnyElement> {
    let mut cells = vec![
        dot(app, row.hue),
        view()
            .w(term::cells(8.0))
            .flex_shrink_0()
            .child(
                text(row.id_label.clone())
                    .truncate()
                    .text_color(color(Token::Ink3)),
            )
            .into_element(),
        view()
            .flex_1()
            .min_w(px(0.0))
            .child(title_line(app, row, 13.0, FontWeight::NORMAL))
            .into_element(),
    ];
    if !row.members.is_empty() {
        cells.push(squares(app, &row.members));
    }
    cells.extend(progress(app, row, 13.0));
    cells
}

/// The colour of words in a state hue; neutral words stay in the quiet ink.
fn status_ink(hue: Hue) -> AdaptiveColor {
    match hue {
        Hue::Neutral => color(Token::Ink3),
        _ => hue_ink(hue),
    }
}
