//! Background: the asking task's goal and place, with the position-in-plan graph. A question in
//! a plan draws that plan with the Plans destination's own graph, the asking task selected.

use nsh_core::prelude::*;

use super::{rich_at, words};
use crate::model::inbox::Background;
use crate::model::{Form, Rich};
use crate::screens::plans::plan_graph;
use crate::state::AppState;
use crate::ui::kit::notice::loading;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

/// Background's placeholder bars while task detail loads (`.skel` 90 %, 70 %, 80 % × 60).
const PLACE_BARS: [(f32, f32); 3] = [(0.9, 10.0), (0.7, 10.0), (0.8, 60.0)];

/// The task's goal and place, with the plan's graph under them when the question is in a plan;
/// a plan needs the band's width. Loading: placeholder bars with the lazy-detail note as the
/// region's caption. Absent: what is not recorded.
pub(super) fn background(app: AppState, part: Background) -> AnyElement {
    let form = app.form;
    match part {
        Background::Loading {
            skeleton_count,
            note,
        } => loading(
            app,
            note,
            &PLACE_BARS[..usize::from(skeleton_count).min(PLACE_BARS.len())],
        ),
        Background::Absent { note } => words(
            app,
            note,
            if form == Form::Phone { 14.0 } else { 12.5 },
            Token::Ink3,
        )
        .into_element(),
        Background::Ready {
            goal, plan_line, ..
        } => goal_and_place(app, &goal, plan_line).into_element(),
        Background::Plan {
            goal,
            plan_line,
            graph,
            selected,
        } => {
            let words_part = goal_and_place(app, &goal, plan_line);
            let drawn = plan_graph(app, move || (graph.clone(), selected.clone()));
            under(app, words_part, drawn)
        }
    }
}

/// The goal over the plan line, which reads quiet.
fn goal_and_place(app: AppState, goal: &Rich, plan_line: Rich) -> View {
    let form = app.form;
    let plan_line: Rich = plan_line.into_iter().map(|run| run.dim()).collect();
    let goal_size = if form == Form::Phone { 14.5 } else { 12.5 };
    view()
        .gap(px(8.0))
        .when(!goal.is_empty(), |v| {
            v.child(rich_at(app, goal, goal_size, color(Token::Ink2)).max_w(px(620.0)))
        })
        .when(!plan_line.is_empty(), |v| {
            v.child(rich_at(
                app,
                &plan_line,
                if form == Form::Phone { 13.0 } else { 12.0 },
                color(Token::Ink3),
            ))
        })
}

/// The words, then the graph under them.
fn under(app: AppState, words_part: View, drawn: AnyElement) -> AnyElement {
    view()
        .child(words_part)
        .child(
            view()
                .mt(px(12.0))
                .when(app.form == Form::Terminal, |v| v.child(term::blank()))
                .child(drawn),
        )
        .into_element()
}
