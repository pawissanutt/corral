//! Route discovery never runs on input or paint paths. Cache entries belong to a task/config revision.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};

use corral_core::{Config, ProfileChoice, Status, Task};
use corral_ops::{Ctx, RouteExplanation};
use jiff::Timestamp;

#[cfg(test)]
pub(crate) mod plan_review_tests;
#[cfg(test)]
pub(crate) mod tests;

#[derive(Debug)]
pub(crate) struct CardRoute {
    pub label: String,
    pub warning: bool,
}

pub(crate) struct TaskRoute {
    pub next: Result<RouteExplanation, String>,
    pub recorded: Option<Result<RouteExplanation, String>>,
    pub card: CardRoute,
}

type Revision = (Timestamp, u64);
struct Request {
    ctx: Ctx,
    tasks: Vec<(Task, Revision)>,
}
struct Response {
    id: String,
    revision: Revision,
    route: TaskRoute,
}

pub(crate) struct RouteCache {
    config: Option<Config>,
    generation: u64,
    revisions: HashMap<String, Revision>,
    values: HashMap<String, TaskRoute>,
    tx: Sender<Request>,
    rx: Receiver<Response>,
}

impl Default for RouteCache {
    fn default() -> Self {
        let (tx, requests) = mpsc::channel::<Request>();
        let (results, rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(request) = requests.recv() {
                for (task, revision) in request.tasks {
                    let route = resolve(&request.ctx, &task);
                    if results
                        .send(Response {
                            id: task.id,
                            revision,
                            route,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }
        });
        Self {
            config: None,
            generation: 0,
            revisions: HashMap::new(),
            values: HashMap::new(),
            tx,
            rx,
        }
    }
}

impl RouteCache {
    /// Called on store refresh, not each frame. Unchanged entries, including errors, are retained.
    pub fn refresh(&mut self, ctx: &Ctx, tasks: &[Task]) {
        if self.config.as_ref() != Some(&ctx.store.config) {
            self.generation = self.generation.wrapping_add(1);
            self.config = Some(ctx.store.config.clone());
        }
        self.revisions
            .retain(|id, _| tasks.iter().any(|t| &t.id == id));
        self.values.retain(|id, _| self.revisions.contains_key(id));
        let tasks: Vec<_> = tasks
            .iter()
            .filter_map(|task| {
                let revision = (task.updated, self.generation);
                if self.revisions.get(&task.id) == Some(&revision) {
                    return None;
                }
                self.revisions.insert(task.id.clone(), revision);
                self.values.remove(&task.id);
                Some((task.clone(), revision))
            })
            .collect();
        if !tasks.is_empty()
            && self
                .tx
                .send(Request {
                    ctx: ctx.clone(),
                    tasks,
                })
                .is_err()
        {
            corral_ops::report_message(None, "routes", "route worker exited".into());
        }
        self.poll();
    }

    pub fn poll(&mut self) {
        while let Ok(response) = self.rx.try_recv() {
            if self.revisions.get(&response.id) == Some(&response.revision) {
                self.values.insert(response.id, response.route);
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<&TaskRoute> {
        self.values.get(id)
    }

    #[cfg(test)]
    pub fn receive(&mut self) {
        while self.values.len() < self.revisions.len() {
            let response = self
                .rx
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("route worker");
            if self.revisions.get(&response.id) == Some(&response.revision) {
                self.values.insert(response.id, response.route);
            }
        }
    }
}

fn resolve(ctx: &Ctx, task: &Task) -> TaskRoute {
    let next = corral_ops::route_snapshot(ctx, task, &ProfileChoice::default(), false)
        .map_err(|e| format!("{e:#}"));
    let recorded = (task.current_run().is_some() || !task.plan_runs.is_empty())
        .then(|| corral_ops::recorded_route(ctx, task).map_err(|e| format!("{e:#}")));
    let planning = crate::plan::stage(task, Some(&ctx.store.config)).is_some_and(|s| {
        !matches!(
            s,
            crate::plan::PlanStage::Approved | crate::plan::PlanStage::Decomposed(_)
        )
    });
    let displayed = if planning && !task.plan_runs.is_empty()
        || matches!(
            task.status,
            Status::Running | Status::NeedsMe | Status::Review | Status::Done
        ) {
        recorded.as_ref().unwrap_or(&next)
    } else {
        &next
    };
    let card = match displayed {
        Ok(route) => {
            let (implementer, reviewer) = if planning {
                (
                    route
                        .review
                        .planner
                        .as_ref()
                        .map_or(route.implementer.profile.as_str(), |p| p.profile.as_str()),
                    route
                        .review
                        .plan_reviewer
                        .as_ref()
                        .unwrap_or(&route.review.reviewer),
                )
            } else {
                (route.implementer.profile.as_str(), &route.review.reviewer)
            };
            CardRoute {
                label: format!(
                    "{implementer} -> {}",
                    reviewer.profile.as_deref().unwrap_or(&reviewer.selection)
                ),
                warning: !route.implementer.warnings.is_empty()
                    || !reviewer.warnings.is_empty()
                    || planning
                        && route
                            .review
                            .planner
                            .as_ref()
                            .is_some_and(|p| !p.warnings.is_empty()),
            }
        }
        Err(_) => CardRoute {
            label: "route unavailable".into(),
            warning: true,
        },
    };
    TaskRoute {
        next,
        recorded,
        card,
    }
}
