//! Clock thresholds of one open task (incremental contract A.1.2): when its per-task sync steps
//! may act without any store change. Each threshold mirrors the comparison the step makes.
use corral_core::backend::TaskNum;
use corral_core::{AgentHostHead, Config, RunHead, TaskHead};
use jiff::{SignedDuration, Timestamp};

use super::incremental::{EVERY_LONG, EVERY_SHORT};

/// A task's clock inputs: absolute thresholds read from its head and a maximum interval
/// between runs for inputs the planner does not observe (files, processes, keyed rows).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TaskDue {
    pub at: Vec<Timestamp>,
    pub every: Option<SignedDuration>,
}

impl TaskDue {
    /// The earliest threshold no run has passed yet, or `last_run + every`. A threshold at or
    /// before the last run was already evaluated by it. Never run: due immediately.
    pub(crate) fn next(&self, last_run: Option<Timestamp>) -> Option<Timestamp> {
        let fixed = self
            .at
            .iter()
            .copied()
            .filter(|at| last_run.is_none_or(|run| *at > run))
            .min();
        let cadence = self.every.map(|every| match last_run {
            Some(run) => add(run, every),
            None => Timestamp::MIN,
        });
        match (fixed, cadence) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn short(&mut self) {
        self.every = Some(EVERY_SHORT);
    }
}

/// What the planner keeps about one open task between passes.
#[derive(Debug, Clone)]
pub(super) struct TaskMemo {
    pub(super) num: TaskNum,
    pub(super) due: TaskDue,
    pub(super) last_run: Option<Timestamp>,
    /// Panes whose absence a per-task step is still counting (pane-liveness misses).
    pub(super) live_panes: Vec<String>,
    /// Any pane reference: `INV` concerns this task.
    pub(super) pane_ref: bool,
    /// Unresolved waits on another task's status.
    pub(super) dependency_waits: bool,
}

impl TaskMemo {
    pub(super) fn of(head: &TaskHead, config: &Config, last_run: Option<Timestamp>) -> Self {
        let mut live = Vec::new();
        let mut pane_ref = false;
        let mut dependency_waits = false;
        if let Some(run) = head.current_run.as_deref() {
            pane_ref |= run.herdr.is_some()
                || !run.facts.pane_refs.is_empty()
                || run
                    .current_child
                    .as_ref()
                    .is_some_and(|c| c.herdr.is_some());
            if run.outcome.is_none()
                && head.status.holds_slot()
                && run.agent_state.as_deref() != Some("gone")
            {
                live.extend(run.herdr.as_ref().map(|h| h.pane_id.clone()));
            }
            if let Some(review) = &run.review {
                pane_ref |= review.pane.is_some();
                if review.holds_slot() {
                    live.extend(review.pane.clone());
                }
            }
            if let Some(gate) = &run.gate {
                pane_ref |= gate.pane.is_some();
                if gate.holds_slot() {
                    live.extend(gate.pane.clone());
                }
            }
            if let Some(child) = run.current_child.as_deref().filter(|c| c.outcome.is_none()) {
                live.extend(child.herdr.as_ref().map(|h| h.pane_id.clone()));
            }
            dependency_waits = run.facts.dependency_waits > 0;
        }
        if let Some(plan_run) = head.last_plan_run.as_deref() {
            pane_ref |= plan_run.herdr.is_some() || !plan_run.facts.pane_refs.is_empty();
            if plan_run.holds_slot() {
                live.extend(plan_run.herdr.as_ref().map(|h| h.pane_id.clone()));
            }
        }
        if let Some(plan) = head.last_plan.as_deref() {
            pane_ref |= !plan.facts.pane_refs.is_empty();
            if let Some(review) = &plan.current_review {
                pane_ref |= review.herdr.is_some();
                if review.verdict.is_none() && !review.cancelled {
                    live.extend(review.herdr.as_ref().map(|h| h.pane_id.clone()));
                }
            }
        }
        if let Some(attempt) = head.last_aggregate_attempt.as_deref() {
            pane_ref |= !attempt.facts.pane_refs.is_empty();
            if let Some(review) = &attempt.current_review {
                pane_ref |= review.herdr.is_some();
                if review.pending() {
                    live.extend(review.herdr.as_ref().map(|h| h.pane_id.clone()));
                }
            }
        }
        Self {
            num: head.num,
            due: task_due(head, config),
            last_run,
            live_panes: live,
            pane_ref,
            dependency_waits,
        }
    }
}

/// Thresholds of `head` under `config`. Verified against the code (sync.rs `reconcile`,
/// `pane_liveness::gone`, `reviewer::watchdog`, `plan_pilot`, `obligations_sync`,
/// `autopilot::infra`, `provider_stall`, `agent::gate_queue`).
pub(crate) fn task_due(head: &TaskHead, config: &Config) -> TaskDue {
    let sync = &config.sync;
    let mut due = TaskDue::default();
    if let Some(run) = head.current_run.as_deref() {
        let open = run.outcome.is_none();
        if open && head.status.holds_slot() {
            worker_run(&mut due, run, config);
        }
        if let Some(review) = run.review.as_ref().filter(|r| r.holds_slot()) {
            // Reviewer watchdog: review idle bound and unseen reviewer startup.
            due.at.push(secs(review.at, sync.review_idle_after_secs));
            due.at.push(secs(review.at, sync.startup_grace_secs));
            due.short();
        }
        if let Some(child) = run.current_child.as_deref().filter(|c| c.outcome.is_none()) {
            due.at.push(secs(child.started, sync.startup_grace_secs));
            if let Some(since) = child.idle_since {
                due.at.push(secs(since, sync.review_idle_after_secs));
                // A child's OmO activity window is `idle_after_secs`.
                due.at.push(secs(since, sync.idle_after_secs));
            }
            due.short();
        }
        if run.gate.as_ref().is_some_and(|g| g.holds_slot())
            || (open && run.agent_host.is_some())
            || native_pending(run.agent_host.as_deref())
        {
            // Gate result files and native receipts (gate_start_uncertain_secs, question
            // deadlines, owed work of leftover native heads) live outside the head.
            due.short();
        }
    }
    if native_pending(
        head.last_plan_run
            .as_deref()
            .and_then(|r| r.agent_host.as_deref()),
    ) {
        due.short();
    }
    if let Some(plan_run) = head.last_plan_run.as_deref().filter(|r| r.holds_slot()) {
        due.at.push(secs(plan_run.started, sync.startup_grace_secs));
        due.short();
    }
    let plan_review = head
        .last_plan
        .as_deref()
        .and_then(|plan| plan.current_review.as_ref());
    if native_pending(plan_review.and_then(|r| r.agent_host.as_deref())) {
        due.short();
    }
    if let Some(review) = plan_review.filter(|r| r.verdict.is_none() && !r.cancelled) {
        if review.herdr.is_some() {
            due.at.push(secs(review.at, sync.startup_grace_secs));
        }
        due.short();
    }
    if head
        .last_aggregate_attempt
        .as_deref()
        .and_then(|attempt| attempt.current_review.as_ref())
        .is_some_and(|review| review.pending())
    {
        due.short();
    }
    if let Some(wait) = head.infra_wait.as_ref().filter(|w| w.held_at.is_none()) {
        // The workspace bound is what `infra_guard` reads; a shorter project override is
        // taken too, which can only run the step earlier.
        let project = head
            .project
            .as_deref()
            .and_then(|p| config.projects.get(p))
            .and_then(|p| p.review.infra_wait_secs);
        let bound = project.map_or(config.review_infra_wait_secs(), |p| {
            p.min(config.review_infra_wait_secs())
        });
        due.at.push(secs(wait.first_seen, bound));
    }
    if due.every.is_none() && head.status.holds_slot() {
        due.every = Some(EVERY_LONG);
    }
    due
}

fn native_pending(host: Option<&AgentHostHead>) -> bool {
    host.is_some_and(|host| host.records.pending > 0)
}

/// `reconcile` thresholds of a slot-holding worker run.
fn worker_run(due: &mut TaskDue, run: &RunHead, config: &Config) {
    let sync = &config.sync;
    if !run.seen_working {
        // Unseen pane: gone after startup grace (`pane_liveness::gone`).
        due.at.push(secs(run.started, sync.startup_grace_secs));
    }
    if let Some(round) = &run.changes_round {
        // A reopened conversation that idles after startup is a report gap.
        due.at.push(secs(round.at, sync.startup_grace_secs));
    }
    if let Some(since) = run.idle_since.filter(|_| !run.idle_no_report) {
        due.at.push(secs(since, sync.idle_after_secs));
    }
    if let Some(since) = run.herdr_idle_since {
        // Stuck inspection compares strictly (`>`), so one second later.
        due.at.push(add(
            secs(since, sync.stuck_after_secs),
            SignedDuration::from_secs(1),
        ));
    }
    if let Some(deadline) = run
        .facts
        .pending_decision
        .as_ref()
        .and_then(|d| d.request.deadline)
    {
        due.at.push(deadline);
    }
    // Wait deadlines (keyed rows), launch-exit receipts, OmO activity, provider-stall
    // session logs and native questions are not on the head.
    due.short();
}

fn secs(at: Timestamp, secs: u64) -> Timestamp {
    add(
        at,
        SignedDuration::from_secs(i64::try_from(secs).unwrap_or(i64::MAX)),
    )
}

fn add(at: Timestamp, by: SignedDuration) -> Timestamp {
    at.saturating_add(by).unwrap_or(Timestamp::MAX)
}
