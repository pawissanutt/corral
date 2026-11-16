//! Plans: the grouped plan list beside the selected plan's detail (desktop and terminal), the
//! Plans tab and the pushed plan page (phone), and the dependency graph Now's panel shows too.
//!
//! Everything shown comes from the served feed (corrald's, or the explicitly chosen synthetic
//! preview) through `model::plans`: `live_list_filtered` in the screen's own project scope for
//! the rows, `live_detail` for the review document, tracker, approved state, document or diff,
//! `live_phone_page` for the pushed page and `layout` for the graph. Presses run
//! `plans::State::reduce_live` through
//! `AppState::reduce`, whose effects the root executes. `live.rs` keeps the shown plan's
//! detail loaded and current.

mod decide;
mod detail;
mod graph;
mod live;
mod story;
mod tracker;
mod verbs;

use nsh_core::prelude::*;

use crate::model::hue::Hue;
use crate::model::plans::{
    self, Action, ButtonKind as Weight, ListNotice, ListSection, LiveAction, MemberSquare,
    PlanList, PlanRow, SquareState,
};
use crate::model::projects::FilterScreen;
use crate::model::text::{flat, span};
use crate::model::{Form, Id};
use crate::screens::{PhoneBar, PhonePage};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::{chip, illus};
use crate::ui::kit::marks::{dot, hue_ink};
use crate::ui::kit::notice::{empty, error, loading, notice};
use crate::ui::kit::page::{Width, columns, group_head, head, pane, search};
use crate::ui::kit::project_filter::project_filter;
use crate::ui::kit::rows::{list_row, measured, project_chip};
use crate::ui::kit::text::rich_small;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// The plan graph renderer, for other destinations that draw a plan's members (Inbox's question
/// sheet); presses go through the Plans actions.
pub(crate) use graph::drawing as plan_graph;
pub use live::{live_graph, live_plan_phone};
pub(crate) use verbs::menu_trigger;

const RING: f32 = 2.0;
/// The most member glyphs a terminal row shows; a longer plan ends its strip in "+n", so the id
/// and title keep their room.
const TERM_SQUARES: usize = 8;

/// The list column's width beside the detail, and on a desktop screen narrower than
/// `LIST_BESIDE` (about 420 pt of detail beside the full list), its compact width.
const LIST: f32 = 380.0;
const LIST_COMPACT: f32 = 280.0;
const LIST_BESIDE: f32 = 800.0;

/// The Plans destination. Desktop: the 380 pt list beside the detail, 280 pt when the screen
/// is narrow. Terminal: the same two panes, the detail alone on a narrow grid (rows select in
/// place, so the detail is what a key press changes). Phone: the large title over the
/// scrolling list; a row pushes its plan.
pub fn screen(app: AppState) -> AnyElement {
    if untracked(|| app.daemon()) {
        live::keep_rows_loaded(app);
        if app.form != Form::Phone {
            live::keep_selected_loaded(app);
        }
    }
    match app.form {
        Form::Phone => view()
            .flex_1()
            .flex_col()
            .min_h(px(0.0))
            .child(dynamic(move || {
                let title = content(app).title;
                head(app, title, None, vec![search(app)])
            }))
            .child(pane(app, "plans-list", list(app)))
            .into_element(),
        Form::Terminal => columns(
            app,
            vec![
                (Width::Fixed(LIST), pane(app, "plans-list", list(app))),
                (Width::Fill, detail::pane(app)),
            ],
            1,
        ),
        Form::Desktop => {
            // Measured, not assumed from the form: a small desktop window beside the sidebar
            // keeps the detail usable by narrowing the list.
            let room = signal(None::<f32>);
            let compact = computed(move || room.get().is_some_and(|width| width < LIST_BESIDE));
            view()
                .flex_1()
                .flex_row()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .on_layout(move |frame: &Bounds<Points>| room.set(Some(frame.size.width.0)))
                .child(dynamic(move || {
                    let list_width = if compact.get() { LIST_COMPACT } else { LIST };
                    untracked(move || {
                        columns(
                            app,
                            vec![
                                (Width::Fixed(list_width), pane(app, "plans-list", list(app))),
                                (Width::Fill, detail::pane(app)),
                            ],
                            1,
                        )
                    })
                }))
                .into_element()
        }
    }
}

/// A plan's pushed phone page from the model's page: its title, Ask in the navigation bar
/// (which then stands for the floating Ask), the detail as the body, and the decide bar
/// whenever the detail shown awaits acceptance, so a plan opened while its detail was loading
/// gains Accept and Changes once it arrives.
fn phone_page(app: AppState, page: plans::PhonePage, plan: Id) -> PhonePage {
    let trailing = page
        .right
        .zip(page.right_label)
        .map(|(button, label)| icon_command(app, &button, label));
    let shown = {
        let plan = plan.clone();
        computed(move || {
            let env = app.env();
            app.plans.with(|state| {
                app.feed
                    .with(|feed| decides(&plans::live_detail(state, Some(&plan), feed, env)))
            })
        })
    };
    PhonePage {
        title: page.title,
        body: detail::phone_body(app, Some(plan.clone())),
        ask: trailing.is_none(),
        trailing,
        bar: Some(PhoneBar {
            shown,
            view: detail::phone_bar(app, Some(plan)),
        }),
        scroll: true,
    }
}

/// Whether a plan's detail offers Accept and Changes: a revision awaiting acceptance, or its
/// diff while it still does.
fn decides(detail: &plans::Detail) -> bool {
    match detail {
        plans::Detail::Awaiting(_) => true,
        plans::Detail::Diff(diff) => diff.decide.is_some(),
        _ => false,
    }
}

/// The list content from the served feed, in the Plans scope.
fn content(app: AppState) -> PlanList {
    let env = app.env();
    let selection = app.selection();
    let filter = app.project_filter(FilterScreen::Plans);
    app.feed
        .with(|feed| plans::live_list_filtered(&selection, feed, env, &filter))
}

/// Runs one Plans press with the served feed.
fn press(app: AppState, action: Action) {
    let Action::Live(live) = action;
    app.reduce(app.plans, move |state, feed, _, env| {
        state.reduce_live(live, feed, env)
    });
}

/// The test id of a press target, from its action.
fn target_id(action: &Action) -> String {
    let Action::Live(live) = action;
    match live {
        LiveAction::Plan(id) => format!("plan-{id}"),
        LiveAction::Member { member: id, .. } | LiveAction::Task(id) => format!("task-{id}"),
        LiveAction::Question(_) => "btn-answer".into(),
        LiveAction::Session(id) => format!("btn-open-session-{id}"),
        LiveAction::Review(_) => "btn-review-candidate".into(),
        LiveAction::Ask(record, _) => format!("btn-ask-{}", record.id()),
        LiveAction::Accept(_) => "btn-accept".into(),
        LiveAction::Changes(_) => "btn-request-changes".into(),
        LiveAction::Draft(..) => "plan-changes-input".into(),
        LiveAction::SendChanges(_) => "btn-send-changes".into(),
        LiveAction::CancelChanges(_) => "btn-cancel-changes".into(),
        LiveAction::Diff(_) => "btn-diff".into(),
        LiveAction::DiffFrom { from, .. } => format!("btn-diff-from-{from}"),
        LiveAction::Resolve { .. } => "plan-decision-input".into(),
        LiveAction::Document(_) => "btn-open-document".into(),
        LiveAction::Overview(_) => "btn-close-document".into(),
        LiveAction::Retry(_) => "btn-retry".into(),
        LiveAction::Recover(_) => "btn-check-result".into(),
    }
}

/// A model button as a kit button: standard is the filled main action, ghost outlined, quiet
/// bare; a refused command is disabled. Ask and session commands lead with the chat icon.
fn command(app: AppState, button: &plans::Button, wide: bool) -> AnyElement {
    let action = button.action.clone();
    let kind = match button.kind {
        Weight::Standard => ButtonKind::Primary,
        Weight::Ghost => ButtonKind::Ghost,
        Weight::Quiet => ButtonKind::Quiet,
    };
    let chat = matches!(
        action,
        Action::Live(LiveAction::Ask(..) | LiveAction::Session(_))
    );
    button_with(
        app,
        button.label.clone(),
        kind,
        ButtonOptions {
            icon: chat.then_some(Icon::Ask),
            disabled: !button.enabled,
            wide,
            ..ButtonOptions::default()
        },
        target_id(&action),
        move || press(app, action.clone()),
    )
}

/// An icon-only control in the phone's navigation bar (`.pnav .rt button`): 44 by 44 pt, the
/// phone's touch-target floor, the chat icon in ink-2, named by `label`.
fn icon_command(app: AppState, button: &plans::Button, label: &'static str) -> AnyElement {
    let action = button.action.clone();
    view()
        .flex_row()
        .items_center()
        .justify_center()
        .min_w(px(44.0))
        .min_h(px(44.0))
        .rounded(px(10.0))
        .role(AccessibilityRole::Button)
        .label(label)
        .test_id(&target_id(&action))
        .cursor_pointer()
        .active(|s| s.opacity(0.6))
        .focus_visible(|s| {
            s.outline(px(RING))
                .outline_color(color(Token::Focus))
                .outline_offset(px(-RING))
        })
        .on_press(move || press(app, action.clone()))
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
/// a terminal shows one glyph per member up to `TERM_SQUARES`, then how many more. Hidden from
/// screen readers: the row says it.
fn squares(app: AppState, members: &[MemberSquare]) -> AnyElement {
    if app.form == Form::Terminal {
        let drawn = if members.len() > TERM_SQUARES {
            TERM_SQUARES - 2
        } else {
            members.len()
        };
        let mut line = text("")
            .flex_shrink_0()
            .number_of_lines(1u32)
            .accessibility_elements_hidden(true);
        for square in members.iter().take(drawn) {
            let (glyph, ink) = square_glyph(square.state, square.hue);
            line = line.child(text(glyph).text_color(ink));
        }
        if drawn < members.len() {
            line = line
                .child(text(format!("+{}", members.len() - drawn)).text_color(color(Token::Ink3)));
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

/// The grouped list (`.list`), the destination's keyboard list (`AppState::list_keys`): the
/// project scope, a loading, empty, error or stale notice where corrald says so, then each
/// section's head in its hue with its count, then its rows.
fn list(app: AppState) -> AnyElement {
    // The plan last scrolled into view: a refresh that keeps the selection leaves the scroll
    // where the reader put it.
    let revealed = stored(None::<Id>);
    let terminal = app.form == Form::Terminal;
    let rows = dynamic(move || {
        let content = content(app);
        let (lead, sections) = match &content.notice {
            Some(ListNotice::Loading) => (
                Some(
                    view()
                        .px(px(if terminal { 0.0 } else { 16.0 }))
                        .py(px(8.0))
                        .child(loading(
                            app,
                            plans::LIST_LOADING,
                            &[(0.7, 12.0), (0.5, 10.0), (0.6, 12.0)],
                        ))
                        .into_element(),
                ),
                vec![],
            ),
            Some(ListNotice::Empty { heading, body }) => {
                (Some(empty(app, *heading, *body, None)), vec![])
            }
            Some(ListNotice::Error(line)) => {
                (Some(error(app, &vec![span(line.as_str())], None)), vec![])
            }
            Some(ListNotice::Stale(line)) => (
                Some(
                    view()
                        .px(px(if terminal { 0.0 } else { 12.0 }))
                        .pb(px(6.0))
                        .child(notice(app, Hue::Held, &vec![span(line.as_str())], None))
                        .into_element(),
                ),
                content.sections.clone(),
            ),
            None => (None, content.sections.clone()),
        };
        view()
            .children(lead)
            .children(
                sections
                    .iter()
                    .map(|section| group(app, section, revealed))
                    .collect::<Vec<_>>(),
            )
            .into_element()
    });
    view()
        .element_ref(&app.list_keys)
        .pt(px(if app.form == Form::Desktop { 6.0 } else { 0.0 }))
        .pb(px(if terminal { 0.0 } else { 14.0 }))
        .role(AccessibilityRole::List)
        .label(untracked(|| content(app).title))
        .child(project_filter(app, FilterScreen::Plans))
        .child(rows)
        .into_element()
}

fn group(app: AppState, section: &ListSection, revealed: Stored<Option<Id>>) -> AnyElement {
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
                .map(|row| plan_row(app, row, revealed))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

/// What a screen reader hears for a plan row: id, title, project, the sub line and the progress.
fn row_label(row: &PlanRow) -> String {
    let mut parts = vec![format!("{} {}", row.id_label, row.title)];
    parts.extend(row.project_chip.clone());
    let sub = flat(&row.sub);
    if !sub.is_empty() {
        parts.push(sub);
    }
    parts.extend(row.progress.clone());
    parts.extend(row.illustrative.map(str::to_owned));
    parts.join(", ")
}

fn plan_row(app: AppState, row: &PlanRow, revealed: Stored<Option<Id>>) -> AnyElement {
    let action = row.action.clone();
    let selected = row.selected;
    let id = target_id(&action);
    let drawn = match app.form {
        Form::Desktop => desktop_row(app, row),
        Form::Phone => list_row(
            app,
            row_label(row),
            id,
            move || selected,
            move || press(app, action.clone()),
            phone_cells(app, row),
        ),
        Form::Terminal => list_row(
            app,
            row_label(row),
            id,
            move || selected,
            move || press(app, action.clone()),
            terminal_cells(app, row),
        ),
    };
    // A plan's row carries its commands in its context menu (RC-03).
    let drawn = verbs::row_menu(app, row.id.clone(), drawn);
    if selected {
        revealing(row.id.clone(), revealed, drawn)
    } else {
        drawn
    }
}

/// The selected row, scrolled into the list's view when the selection has just moved to it, so
/// a key press never leaves it off screen; a row already in view stays put. The plan counts as
/// revealed only once the scroll has run: a rebuild of the rows before then (the selected plan's
/// detail arriving, the feed's clock) drops the pending scroll with the row it was for, and the
/// rebuilt row asks again.
fn revealing(id: Id, revealed: Stored<Option<Id>>, row: AnyElement) -> AnyElement {
    let handle = element_ref();
    if revealed.with(|shown| shown.as_ref() != Some(&id)) {
        after_answer(move || {
            revealed.replace(Some(id));
            handle.reveal(RevealOptions::default().vertical(ScrollAlignment::Nearest));
        });
    }
    view().element_ref(&handle).child(row).into_element()
}

/// The title line of a row: the title cut to fit, its project chip and illustrative tag after it.
fn title_line(app: AppState, row: &PlanRow, size: f32, weight: FontWeight) -> AnyElement {
    let words = text(row.title.clone())
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
    let room = signal(0.0_f32);
    // The illustrative tag beside the project's takes its own room from the title.
    let reserved = row
        .illustrative
        .map_or(0.0, |label| label.chars().count() as f32 + 3.0);
    let tag = row
        .project_chip
        .clone()
        .map(|project| project_chip(app, project, room, reserved));
    measured(
        app,
        view()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .min_w(px(0.0))
            // A terminal row measures this line, so it takes its width from the cell around it.
            .when(app.form == Form::Terminal, |line| line.w_full()),
        room,
    )
    .child(words)
    .children(tag)
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
    let action = row.action.clone();
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
        .test_id(&target_id(&action))
        .on_press(move || press(app, action.clone()))
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

/// The phone row (`.prow`): the dot, then `id · title`, the title free to take two lines (at
/// 16 pt one line holds too few characters to tell like plans apart), and under them the strip,
/// the sub line and the progress.
fn phone_cells(app: AppState, row: &PlanRow) -> Vec<AnyElement> {
    let title = view()
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
        .child(
            text(row.title.clone())
                .flex_1()
                .min_w(px(0.0))
                .number_of_lines(2u32)
                .font_family(FONT_TEXT)
                .text_size(px(16.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(Token::Ink)),
        )
        .children(row.illustrative.map(|label| illus(app, label)));
    let figures = progress(app, row, 12.0);
    let project = row
        .project_chip
        .clone()
        .map(|project| chip(app, project, Hue::Neutral));
    let meta =
        (project.is_some() || !row.members.is_empty() || !row.sub.is_empty() || figures.is_some())
            .then(|| {
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .min_w(px(0.0))
                    .children(project)
                    .when(!row.members.is_empty(), |line| {
                        line.child(squares(app, &row.members))
                    })
                    // The sentence takes the room between the strip and the figures, so the figures
                    // keep the trailing edge even when there is no sentence.
                    .child(
                        view()
                            .flex_1()
                            .min_w(px(0.0))
                            .when(!row.sub.is_empty(), |cell| {
                                cell.child(rich_small(app, &row.sub))
                            }),
                    )
                    .children(figures)
                    .into_element()
            });
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
            .gap(px(3.0))
            .child(title)
            .children(meta)
            .into_element(),
    ]
}

/// One line of Cells: dot, the id whole, title, strip, progress in a column of its own. The sub
/// line is left to the detail, which states the unmapped criteria in full.
fn terminal_cells(app: AppState, row: &PlanRow) -> Vec<AnyElement> {
    let mut cells = vec![
        dot(app, row.hue),
        view()
            .min_w(term::cells(8.0))
            .flex_shrink_0()
            .child(
                text(row.id_label.clone())
                    .number_of_lines(1u32)
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
    cells.extend(progress(app, row, 13.0).map(|figures| {
        view()
            .flex_row()
            .justify_end()
            .flex_shrink_0()
            .min_w(term::cells(5.0))
            .child(figures)
            .into_element()
    }));
    cells
}

#[cfg(test)]
#[path = "plans_tests.rs"]
mod tests;
