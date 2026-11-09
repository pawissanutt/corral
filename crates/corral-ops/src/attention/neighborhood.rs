//! Frozen neighborhood facts over either cold full tasks or typed task heads.
use crate::slots::SlotUsage;
use anyhow::Result;
use corral_core::agent::QuestionState;
use corral_core::inbox::OwnerEntry;
use corral_core::{
    AvailabilityEpisode, Config, EffectivePolicy, OperatingActionKind as Action, Status, Task,
    TaskFieldsRef, TaskHead, TaskRef,
};
use jiff::Timestamp;
use std::collections::{BTreeMap, BTreeSet};

/// Everything assessment may inspect. No constructor probes Git or pane inventory.
pub struct Neighborhood<'a> {
    pub tasks: Vec<&'a dyn TaskRef>,
    pub config: &'a Config,
    pub policy: &'a EffectivePolicy,
    pub deliveries: BTreeMap<String, Vec<OwnerEntry>>,
    /// Unknown inventory is never evidence that a pane disappeared.
    pub inventory_known: bool,
    pub resource_wait: Option<String>,
    pub availability: BTreeMap<String, AvailabilityEpisode>,
    pub missing_reviewers: BTreeMap<String, Timestamp>,
    pub prepared_refusals: BTreeMap<String, (Timestamp, String)>,
    pub queue_refusals: BTreeMap<String, (Timestamp, String)>,
    by_id: BTreeMap<&'a str, &'a dyn TaskRef>,
    // Only the explicit cold compatibility constructor retains full-task sources.
    cold_tasks: Option<&'a [Task]>,
    pub(super) slots: usize,
    pub(super) review_slots: usize,
    pub(super) independent: crate::capacity::CapacityUsage,
    independent_tasks: BTreeMap<&'a str, crate::capacity::CapacityUsage>,
    pub(super) project_slots: BTreeMap<Option<&'a str>, usize>,
    landed: BTreeMap<&'a str, bool>,
    answers: BTreeMap<String, (String, bool)>,
    decisions: BTreeMap<String, bool>,
}

fn landed(task: &(impl TaskFieldsRef + ?Sized), run: Option<&str>, auto_done: bool) -> bool {
    *task.status() == Status::Done
        && run.is_some_and(|run| {
            task.landed()
                .as_ref()
                .is_some_and(|receipt| receipt.run == run && !receipt.sha.is_empty())
                || auto_done
        })
}

impl<'a> Neighborhood<'a> {
    /// Cold complete-snapshot constructor, using the selected scheduler model's counters.
    pub fn new(tasks: &'a [Task], config: &'a Config, policy: &'a EffectivePolicy) -> Self {
        let mut n = Self::from_views(
            tasks.iter().map(|task| {
                (
                    task as &dyn TaskRef,
                    SlotUsage::from_tasks(std::slice::from_ref(task), &config.limits),
                )
            }),
            config,
            policy,
        );
        for task in tasks {
            let run = task.current_run();
            n.landed.insert(
                &task.id,
                landed(
                    task,
                    run.map(|run| run.id.as_str()),
                    run.and_then(|run| run.autopilot.as_ref())
                        .is_some_and(|state| {
                            state.stage == corral_core::AutoStage::Done && state.sha.is_some()
                        }),
                ),
            );
        }
        n.cold_tasks = Some(tasks);
        n
    }

    /// Hot constructor over the caller's active/keyed neighborhood. Named selectors must
    /// have their owner head included, even when that owner is terminal. All keyed reads
    /// use these same pinned heads; errors cannot masquerade as an unsatisfied check.
    pub fn new_heads(
        tasks: &'a [TaskHead],
        config: &'a Config,
        policy: &'a EffectivePolicy,
    ) -> Result<Self> {
        let mut n = Self::from_views(
            tasks.iter().map(|task| {
                (
                    task as &dyn TaskRef,
                    SlotUsage::from_heads(std::slice::from_ref(task), &config.limits),
                )
            }),
            config,
            policy,
        );
        for task in tasks {
            let run = task.current_run.as_deref();
            n.landed.insert(
                task.id.as_str(),
                landed(
                    task,
                    run.map(|run| run.id.as_str()),
                    run.and_then(|run| run.autopilot.as_ref())
                        .is_some_and(|state| {
                            state.stage == corral_core::AutoStage::Done && state.sha.is_some()
                        }),
                ),
            );
        }
        let (questions, decisions) = n.selectors();
        for target in questions {
            for task in &n.tasks {
                if let Some(question) = task.question_by_key(&target)? {
                    n.answers.insert(
                        target,
                        (
                            question.revision.to_string(),
                            question.state == QuestionState::Answered && question.answer.is_some(),
                        ),
                    );
                    break;
                }
            }
        }
        for target in decisions {
            for task in &n.tasks {
                if let Some(decision) = task.decision_by_id(&target)? {
                    n.decisions.insert(
                        target,
                        decision.receipt.as_ref().is_some_and(|receipt| {
                            receipt.outcome == corral_core::ReceiptOutcome::Answered
                        }),
                    );
                    break;
                }
            }
        }
        Ok(n)
    }

    fn selectors(&self) -> (BTreeSet<String>, BTreeSet<String>) {
        let mut questions = BTreeSet::new();
        let mut decisions = BTreeSet::new();
        for task in &self.tasks {
            if let Some(park) = task.park() {
                for check in &park.resume_when {
                    if let Some(target) = &check.target {
                        match check.kind.as_str() {
                            "answered" => {
                                questions.insert(target.clone());
                            }
                            "decision" => {
                                decisions.insert(target.clone());
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        (questions, decisions)
    }

    fn from_views(
        views: impl IntoIterator<Item = (&'a dyn TaskRef, SlotUsage)>,
        config: &'a Config,
        policy: &'a EffectivePolicy,
    ) -> Self {
        let mut tasks = Vec::new();
        let mut by_id = BTreeMap::new();
        let mut slots = 0;
        let mut review_slots = 0;
        let mut independent = crate::capacity::CapacityUsage::default();
        let mut independent_tasks = BTreeMap::new();
        let mut project_slots = BTreeMap::new();
        for (task, usage) in views {
            slots += usage.total();
            review_slots += usage.reviews + usage.gates;
            if let Some(used) = &usage.independent {
                independent.merge(used);
                independent_tasks.insert(task.id().as_str(), used.clone());
            }
            *project_slots.entry(task.project().as_deref()).or_default() += usage.total();
            by_id.insert(task.id().as_str(), task);
            tasks.push(task);
        }
        Self {
            tasks,
            config,
            policy,
            deliveries: BTreeMap::new(),
            inventory_known: false,
            resource_wait: None,
            availability: BTreeMap::new(),
            missing_reviewers: BTreeMap::new(),
            prepared_refusals: BTreeMap::new(),
            queue_refusals: BTreeMap::new(),
            by_id,
            cold_tasks: None,
            slots,
            review_slots,
            independent,
            independent_tasks,
            project_slots,
            landed: BTreeMap::new(),
            answers: BTreeMap::new(),
            decisions: BTreeMap::new(),
        }
    }

    pub(super) fn plan_review_wait(
        &self,
        task: &corral_core::TaskHeadFields,
        pending: bool,
    ) -> Option<String> {
        if self.config.limits.capacity.is_some() {
            self.agent_wait(task, usize::from(pending))
        } else {
            (self.review_slots >= self.config.limits.review)
                .then(|| "Waiting for a plan-review slot".into())
        }
    }

    pub(super) fn agent_wait(
        &self,
        task: &corral_core::TaskHeadFields,
        credit: usize,
    ) -> Option<String> {
        let limits = self.config.limits.capacity.as_ref()?;
        let own = self.independent_tasks.get(task.id.as_str());
        let mut used = self.independent.clone();
        used.agents = used.agents.credited(
            task.capacity_lane,
            credit.min(own.map_or(0, |u| u.agents.total())),
        );
        used.agent_reason(limits, task.capacity_lane)
    }

    pub(super) fn unfinished_wait(&self, task: &corral_core::TaskHeadFields) -> Option<String> {
        let limits = self.config.limits.capacity.as_ref()?;
        if self
            .independent_tasks
            .get(task.id.as_str())
            .is_some_and(|u| u.unfinished.total() > 0)
        {
            return None;
        }
        self.independent
            .unfinished_reason(limits, task.capacity_lane)
    }

    pub fn task(&self, id: &str) -> Option<&dyn TaskRef> {
        self.by_id.get(id).copied()
    }
    pub(crate) fn landed(&self, id: &str) -> Option<bool> {
        self.landed.get(id).copied()
    }
    pub(crate) fn answered(&self, key: &str, revision: Option<&str>) -> Option<bool> {
        if let Some(tasks) = self.cold_tasks {
            return tasks.iter().find_map(|task| {
                crate::questions::containers(task)
                    .into_iter()
                    .find_map(|container| {
                        corral_core::agent::container_state(task, &container)?
                            .questions
                            .iter()
                            .find(|question| {
                                question.key == key
                                    && Some(question.revision.to_string()).as_deref() == revision
                            })
                            .map(|question| {
                                question.state == QuestionState::Answered
                                    && question.answer.is_some()
                            })
                    })
            });
        }
        self.answers
            .get(key)
            .filter(|(found, _)| Some(found.as_str()) == revision)
            .map(|(_, answered)| *answered)
    }
    pub(crate) fn decision_answered(&self, id: &str) -> Option<bool> {
        if let Some(tasks) = self.cold_tasks {
            return tasks
                .iter()
                .flat_map(|task| &task.runs)
                .flat_map(|run| std::iter::once(run).chain(&run.review_runs))
                .flat_map(|run| &run.obligations.decisions)
                .find(|decision| decision.request.id == id)
                .map(|decision| {
                    decision.receipt.as_ref().is_some_and(|receipt| {
                        receipt.outcome == corral_core::ReceiptOutcome::Answered
                    })
                });
        }
        self.decisions.get(id).copied()
    }

    pub fn allows(&self, task: &(impl TaskFieldsRef + ?Sized), action: Action) -> bool {
        self.policy.allows(task.id(), action)
            || task.park().as_ref().is_some_and(|park| {
                self.policy
                    .tasks
                    .get(task.id())
                    .and_then(|policy| policy.refusals.get(&action))
                    .is_some_and(|reasons| {
                        reasons
                            .iter()
                            .all(|reason| *reason == park.refusal(task.id()))
                    })
            })
    }

    pub(super) fn authority_reason(
        &self,
        task: &(impl TaskFieldsRef + ?Sized),
        action: Action,
    ) -> String {
        self.policy
            .tasks
            .get(task.id())
            .and_then(|policy| policy.refusals.get(&action))
            .map(|reasons| {
                let Some(park) = task.park() else {
                    return reasons.join("; ");
                };
                let refusal = park.refusal(task.id());
                reasons
                    .iter()
                    .map(String::as_str)
                    .filter(|reason| *reason != refusal)
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_else(|| {
                if self.policy.reasons.is_empty() {
                    "Task is outside the confirmed operating grant".into()
                } else {
                    self.policy.reasons.join("; ")
                }
            })
    }
}
