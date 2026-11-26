//! Comparing a recipe diagram with a later commit: the choice of commit, what corrald pinned and
//! found, and the selected element at that commit beside its pinned evidence
//! (`model::artifacts::diagram_compare`). The artifact itself is never changed.
//!
//! test ids: "diagram-compare" (the bar), "seg-diagram-compare-<n>", "diagram-compare-status",
//! "diagram-compare-summary", "diagram-compare-side", "diagram-compare-source",
//! "diagram-compare-link-<n>".

use std::time::Duration;

use nsh_core::prelude::*;

use crate::model::artifacts::Action;
use crate::model::artifacts::LOADING_CONTENT;
use crate::model::artifacts::diagram::Graph;
use crate::model::artifacts::diagram_compare::{
    COMPARE_CHOICES, COMPARE_HEAD, COMPARE_NOTE, Link, Side, retry,
};
use crate::model::nav::TaskTab;
use crate::model::review::impact::expected::Expected;
use crate::model::review::impact::{Action as ImpactAction, View as ChangesView};
use crate::model::{Hue, Live};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button, seg};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::text::{dim, h3, plain};

const RETRY: Duration = Duration::from_millis(1500);

/// The comparison's choice, its pinned commit and what changed, under the graph.
pub(super) fn bar(app: AppState, graph: Memo<Option<Graph>>) -> AnyElement {
    pin_and_retry(app, graph);
    let shown = computed(move || {
        graph.with(|g| {
            g.as_ref().filter(|g| g.can_compare).map(|g| {
                (
                    g.reference.clone(),
                    g.compare
                        .as_ref()
                        .map(|c| (c.status.clone(), c.summary.clone())),
                )
            })
        })
    });
    let reference = computed(move || graph.with(|g| g.as_ref().map(|g| g.reference.clone())));
    let chosen = move || {
        graph.with(|g| {
            g.as_ref()
                .and_then(|g| g.compare.as_ref())
                .map(|c| c.origin)
        })
    };
    dynamic(move || {
        let (_, compare) = shown.get()?;
        Some(untracked(|| {
            let picker = seg(
                app,
                &COMPARE_CHOICES,
                chosen,
                move |origin| {
                    if let Some(reference) = reference.get_untracked() {
                        app.artifacts_act(Action::Compare { reference, origin });
                    }
                },
                "seg-diagram-compare",
            );
            let mut bar = view().test_id("diagram-compare").gap(px(4.0)).child(
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .child(plain(app, COMPARE_HEAD).font_weight(FontWeight::BOLD))
                    .child(picker),
            );
            match compare {
                None => bar = bar.child(dim(app, COMPARE_NOTE)),
                Some((status, summary)) => {
                    bar = bar
                        .child(plain(app, status).test_id("diagram-compare-status"))
                        .children(summary.map(|summary| {
                            plain(app, summary)
                                .test_id("diagram-compare-summary")
                                .into_element()
                        }))
                        .child(dim(app, COMPARE_NOTE));
                }
            }
            bar.into_element()
        }))
    })
    .into_element()
}

/// Pins the commit corrald resolved, once, and asks again while an answer is pending.
fn pin_and_retry(app: AppState, graph: Memo<Option<Graph>>) {
    let pin = computed(move || {
        graph.with(|g| {
            g.as_ref().and_then(|g| {
                let commit = g.compare.as_ref()?.pin.clone()?;
                Some((g.reference.clone(), commit))
            })
        })
    });
    effect(move || {
        if let Some((reference, commit)) = pin.get() {
            app.artifacts_act(Action::Pin { reference, commit });
        }
    });
    let wanted = computed(move || {
        graph.with(|g| {
            g.as_ref()
                .and_then(|g| g.compare.as_ref())
                .map(retry)
                .unwrap_or_default()
        })
    });
    let asked = signal(0u32);
    effect(move || {
        asked.get();
        let targets = wanted.get();
        if !targets.is_empty() {
            set_timeout(RETRY, move || {
                if wanted.get_untracked() == targets {
                    for target in &targets {
                        app.refresh(target.clone());
                    }
                    asked.update(|n| *n = n.wrapping_add(1));
                }
            });
        }
    });
}

/// The selected element at the comparison commit, under its pinned evidence.
pub(super) fn side(app: AppState, side: Side) -> AnyElement {
    let gap = if app.form == crate::model::Form::Terminal {
        0.0
    } else {
        4.0
    };
    let body: Vec<AnyElement> = match side {
        Side::Loading => vec![dim(app, LOADING_CONTENT).into_element()],
        Side::Failed(why) => vec![dim(app, why).text_color(hue_ink(Hue::Fail)).into_element()],
        Side::Ready {
            change,
            source,
            derived,
            links,
            pending: _,
        } => {
            let mut out = vec![
                plain(app, change).into_element(),
                super::diagram::source_block(app, source, "diagram-compare-source"),
            ];
            out.extend(
                derived
                    .into_iter()
                    .map(|line| plain(app, line).into_element()),
            );
            out.extend(links.into_iter().enumerate().map(|(n, link)| {
                let label = link.label();
                button(
                    app,
                    label,
                    ButtonKind::Ghost,
                    format!("diagram-compare-link-{n}"),
                    move || open(app, &link),
                )
            }));
            out
        }
    };
    view()
        .test_id("diagram-compare-side")
        .gap(px(gap))
        .child(h3(app, "At the comparison commit", None))
        .children(body)
        .into_element()
}

/// A candidate link opens its task's Changes at the diff or Impact, expecting the submission
/// it was served for: Changes show that one (the diff's file expanded once the task loads) or
/// say they hold another (`model::review::impact::expected`).
fn open(app: AppState, link: &Link) {
    let (task, view, expect) = match link {
        Link::Diff { task, head, path } => (
            task,
            ChangesView::Diff,
            Expected {
                head: head.clone(),
                path: Some(path.clone()),
            },
        ),
        Link::Impact { task, head } => (
            task,
            ChangesView::Impact,
            Expected {
                head: head.clone(),
                path: None,
            },
        ),
    };
    app.open_live(Live::Task(task.as_str().into()));
    app.set_task_tab(TaskTab::Changes);
    app.impact_act(task, ImpactAction::View(view));
    app.impact_act(task, ImpactAction::Expect(Some(expect)));
}
