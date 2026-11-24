//! Impact's inspector: what the picked card or edge is, led by "This task" (the lines the
//! change added and removed there, the files they are in and the dependents held there, each
//! of which selects its file on the map), then Activity › System's own inspector for the same
//! node, drawn by the System page's code on this map.
//!
//! Test ids: "impact-this-task", "impact-file-<n>", "impact-dependent-<n>", and those of
//! `screens::system::inspected`.

use corral_types::SystemAt;
use nsh_core::prelude::*;

use crate::model::Id;
use crate::model::review::impact::Action;
use crate::model::review::impact::this_task::{self, Line, ThisTask};
use crate::model::system::inspect::inspect;
use crate::model::system::marks::{Marks, drawn_pick};
use crate::model::system::scene::Scene;
use crate::screens::system::{Host, inspected};
use crate::state::AppState;
use crate::ui::kit::rows::list_row;
use crate::ui::kit::text::{h3, plain};
use crate::ui::tokens::{Token, color};

use super::parts::quiet;

pub(super) fn inspector(
    app: AppState,
    task: Memo<Id>,
    trail: Memo<Vec<SystemAt>>,
    scene: Memo<Option<Scene>>,
    marks: Memo<Marks>,
) -> AnyElement {
    let shown = computed(move || {
        let task = task.get();
        let picked = app.impact.with(|state| {
            (state.task.as_deref() == Some(task.as_str()))
                .then(|| state.map.picked.clone())
                .flatten()
        })?;
        scene.with(|scene| {
            let scene = scene.as_ref()?;
            // The map draws a file folded into "N more" on that card; so does the inspector.
            let pick = marks.with(|marks| drawn_pick(scene, marks, Some(picked)))?;
            let at = trail.with(|trail| trail.last().cloned());
            let inspector = inspect(scene, &pick, at.as_ref());
            let this = app
                .feed
                .with(|feed| this_task::this_task(feed, task.as_str(), &pick));
            (this.is_some() || inspector.is_some()).then_some((this, inspector))
        })
    });
    let host = Host::Impact(marks, task, scene);
    dynamic(move || {
        let Some((this, inspector)) = shown.get() else {
            return untracked(|| quiet(app, this_task::NOTHING));
        };
        untracked(|| {
            view()
                .gap(px(10.0))
                .children(this.map(|this| this_task(app, task, this)))
                .children(inspector.map(|inspector| inspected(app, inspector, host)))
                .into_element()
        })
    })
    .into_element()
}

fn this_task(app: AppState, task: Memo<Id>, this: ThisTask) -> AnyElement {
    let ThisTask {
        lines,
        files,
        held,
        dependents,
    } = this;
    view()
        .test_id("impact-this-task")
        .gap(px(5.0))
        .child(h3(app, this_task::TITLE, None))
        .child(plain(app, lines))
        .children(
            files
                .into_iter()
                .enumerate()
                .map(|(n, line)| row(app, task, line, format!("impact-file-{n}"), false)),
        )
        .children(held.map(|held| quiet(app, held)))
        .children(
            dependents
                .into_iter()
                .enumerate()
                .map(|(n, line)| row(app, task, line, format!("impact-dependent-{n}"), true)),
        )
        .into_element()
}

/// A file or dependent; selecting it shows its file on the map where the head has it.
fn row(app: AppState, task: Memo<Id>, line: Line, id: String, stacked: bool) -> AnyElement {
    let Line {
        main,
        right,
        target,
    } = line;
    // A file's churn is short and sits beside its path; a dependent's place is long and goes
    // under its name.
    let words = if stacked {
        vec![
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(plain(app, main.clone()).number_of_lines(1u32))
                .child(quiet(app, right.clone()))
                .into_element(),
        ]
    } else {
        vec![
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(plain(app, main.clone()).number_of_lines(1u32))
                .into_element(),
            view()
                .flex_shrink_0()
                .child(text(right.clone()).text_color(color(Token::Ink3)))
                .into_element(),
        ]
    };
    match target {
        Some(target) => list_row(
            app,
            format!("{main} {right}"),
            id,
            || false,
            move || {
                let task = untracked(|| task.get());
                app.impact_act(task.as_str(), Action::Focus(target.clone()));
            },
            words,
        ),
        None => view()
            .test_id(&id)
            .flex_row()
            .gap(px(6.0))
            .children(words)
            .into_element(),
    }
}
