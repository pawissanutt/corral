//! The history summary of a task (plan T-253 section 4): constant-size facts over its history
//! rows only, the earlier elements of the growing vectors. Head mutations fold demoted or
//! appended rows into it; full mutations recompute it with [`HistorySummary::of`].

use std::collections::BTreeMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::container_facts::{
    CostTotals, changes_unsuperseded, herdr_panes, plan_run_panes, run_panes,
};
use crate::{PlanRecord, PlanRun, Run, Task, Verdict};

/// The id, outcome and end of the most recent ended run of one family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndedRun {
    pub id: String,
    pub outcome: Option<crate::RunOutcome>,
    pub ended: Timestamp,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenTotals {
    #[serde(with = "crate::container_facts::fixed")]
    pub input: u64,
    #[serde(with = "crate::container_facts::fixed")]
    pub output: u64,
    #[serde(with = "crate::container_facts::fixed")]
    pub cache_read: u64,
    #[serde(with = "crate::container_facts::fixed")]
    pub cache_write: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistorySummary {
    /// History coding runs (every run but the current one).
    #[serde(with = "crate::container_facts::fixed")]
    pub runs: u64,
    #[serde(with = "crate::container_facts::fixed")]
    pub plan_runs: u64,
    #[serde(with = "crate::container_facts::fixed")]
    pub plans: u64,
    #[serde(with = "crate::container_facts::fixed")]
    pub aggregate_attempts: u64,
    #[serde(with = "crate::container_facts::fixed")]
    pub comments: u64,
    #[serde(with = "crate::container_facts::fixed")]
    pub artifacts: u64,
    #[serde(with = "crate::container_facts::fixed")]
    pub kept_artifacts: u64,
    /// Artifacts registered to the current coding run, not its accumulated row collection.
    pub current_artifacts: crate::container_facts::ArtifactFacts,
    #[serde(with = "crate::container_facts::fixed")]
    pub scope_revisions: u64,
    #[serde(with = "crate::container_facts::fixed")]
    pub pm_receipts: u64,
    #[serde(with = "crate::container_facts::fixed")]
    pub member_gate_revisions: u64,
    /// History coding runs by outcome (`active` for runs without one).
    #[serde(with = "crate::container_facts::fixed::map")]
    pub runs_by_outcome: BTreeMap<String, u64>,
    #[serde(with = "crate::container_facts::fixed")]
    pub failures: u64,
    pub last_ended_run: Option<EndedRun>,
    pub last_ended_plan_run: Option<EndedRun>,
    pub tokens: TokenTotals,
    /// Earlier runs with usage, including a present but zero-token usage record.
    #[serde(with = "crate::container_facts::fixed")]
    pub usage_runs: u64,
    #[serde(with = "crate::container_facts::fixed")]
    pub held_gates: u64,
    #[serde(with = "crate::container_facts::fixed")]
    pub held_reviews: u64,
    #[serde(with = "crate::container_facts::fixed")]
    pub pending_plan_reviews: u64,
    /// Per-run cost subtotals, summed run by run.
    pub cost: CostTotals,
    /// Pane references of history containers per socket.
    #[serde(with = "crate::container_facts::fixed::map")]
    pub pane_refs: BTreeMap<String, u64>,
    #[serde(with = "crate::container_facts::fixed")]
    pub review_changes_unsuperseded: u64,
    #[serde(with = "crate::container_facts::fixed")]
    pub plan_review_changes: u64,
    /// The task's operating-contract revision, kept here because its digest covers the whole
    /// scope-amendment history: hot reads take it without reading that history.
    pub operating_contract_revision: String,
    /// [`crate::operating::scope_history_chain`] of the scope-amendment history, extended as
    /// revisions are appended, so the contract revision is recomputed without reading it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope_history_chain: String,
}

fn n(len: usize) -> u64 {
    u64::try_from(len).unwrap_or(u64::MAX)
}

fn history<T>(items: &[T]) -> &[T] {
    &items[..items.len().saturating_sub(1)]
}

fn outcome_name(run: &Run) -> String {
    run.outcome
        .and_then(|o| serde_json::to_value(o).ok())
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "active".to_owned())
}

impl HistorySummary {
    /// # Errors
    /// Returns a serialization failure of the operating-contract digest.
    pub fn of(task: &Task) -> anyhow::Result<HistorySummary> {
        let mut summary = HistorySummary {
            operating_contract_revision: crate::operating::operating_contract_revision(task)?,
            scope_history_chain: crate::operating::scope_history_chain(
                &task.approved_scope_history,
            )?,
            comments: n(task.comments.len()),
            artifacts: n(task.artifacts.len()),
            kept_artifacts: n(task
                .artifacts
                .iter()
                .filter(|a| a.tier() == crate::ArtifactTier::Keep && a.purged_at.is_none())
                .count()),
            scope_revisions: n(task.approved_scope_history.len()),
            pm_receipts: n(task.agent_pm_receipts.len()),
            member_gate_revisions: task.member_gates.values().map(|g| n(g.history.len())).sum(),
            ..HistorySummary::default()
        };
        if let Some(run) = task.current_run() {
            for artifact in task.artifacts.iter().filter(|a| a.run == run.id) {
                summary.current_artifacts.fold(artifact);
            }
        }
        for (seq, run) in history(&task.runs).iter().enumerate() {
            summary.fold_run(seq + 1, run);
        }
        for (seq, run) in history(&task.plan_runs).iter().enumerate() {
            summary.fold_plan_run(seq + 1, run);
        }
        for (seq, plan) in history(&task.plans).iter().enumerate() {
            summary.fold_plan(seq + 1, plan);
        }
        for (seq, attempt) in history(&task.aggregate_attempts).iter().enumerate() {
            summary.aggregate_attempts += 1;
            for (r, review) in attempt.reviews.iter().enumerate() {
                summary.add_panes(herdr_panes(
                    &format!("aggregate_review:{:06}/{:06}", seq + 1, r + 1),
                    "aggregate_review",
                    review.herdr.as_ref(),
                ));
            }
        }
        Ok(summary)
    }

    fn add_panes(&mut self, panes: Vec<crate::PaneRef>) {
        for pane in panes {
            *self.pane_refs.entry(pane.socket).or_default() += 1;
        }
    }

    /// Folds the coding run at 1-based `seq` as it becomes a history row.
    pub fn fold_run(&mut self, seq: usize, run: &Run) {
        self.runs += 1;
        *self.runs_by_outcome.entry(outcome_name(run)).or_default() += 1;
        self.failures += u64::from(run.failures);
        if let Some(ended) = run.ended
            && self
                .last_ended_run
                .as_ref()
                .is_none_or(|last| last.ended <= ended)
        {
            self.last_ended_run = Some(EndedRun {
                id: run.id.clone(),
                outcome: run.outcome,
                ended,
            });
        }
        if let Some(usage) = run.total_usage() {
            self.usage_runs += 1;
            self.tokens.input += usage.input;
            self.tokens.output += usage.output;
            self.tokens.cache_read += usage.cache_read;
            self.tokens.cache_write += usage.cache_write;
        }
        self.cost.add(&CostTotals::of_run(run));
        self.held_gates += u64::from(run.gate.as_ref().is_some_and(crate::GateRecord::holds_slot));
        self.held_reviews += u64::from(
            run.review
                .as_ref()
                .is_some_and(crate::ReviewRecord::holds_slot),
        );
        let owner = format!("{seq:06}/000000");
        self.add_panes(run_panes(&format!("run:{owner}"), run));
        for (c, child) in run.review_runs.iter().enumerate() {
            self.add_panes(run_panes(&format!("run:{seq:06}/{:06}", c + 1), child));
        }
        self.review_changes_unsuperseded += n(run
            .review_runs
            .iter()
            .filter(|c| changes_unsuperseded(c))
            .count());
    }

    pub fn fold_plan_run(&mut self, seq: usize, run: &PlanRun) {
        self.plan_runs += 1;
        if let Some(ended) = run.ended
            && self
                .last_ended_plan_run
                .as_ref()
                .is_none_or(|last| last.ended <= ended)
        {
            self.last_ended_plan_run = Some(EndedRun {
                id: run.id.clone(),
                outcome: None,
                ended,
            });
        }
        self.add_panes(plan_run_panes(&format!("plan_run:{seq:06}"), run));
    }

    pub fn fold_plan(&mut self, seq: usize, plan: &PlanRecord) {
        self.plans += 1;
        self.pending_plan_reviews += n(plan.reviews.iter().filter(|r| r.verdict.is_none()).count());
        self.plan_review_changes += n(plan
            .reviews
            .iter()
            .filter(|r| r.verdict == Some(Verdict::Changes))
            .count());
        for (r, review) in plan.reviews.iter().enumerate() {
            self.add_panes(herdr_panes(
                &format!("plan_review:{seq:06}/{:06}", r + 1),
                "plan_review",
                review.herdr.as_ref(),
            ));
        }
    }

    /// [`Self::fold_run`] from a projection and its facts, as a head mutation demotes a run.
    pub fn fold_run_head(&mut self, run: &crate::RunHead) {
        self.runs += 1;
        let outcome = run
            .outcome
            .and_then(|o| serde_json::to_value(o).ok())
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "active".to_owned());
        *self.runs_by_outcome.entry(outcome).or_default() += 1;
        self.failures += u64::from(run.failures);
        if let Some(ended) = run.ended
            && self
                .last_ended_run
                .as_ref()
                .is_none_or(|last| last.ended <= ended)
        {
            self.last_ended_run = Some(EndedRun {
                id: run.id.clone(),
                outcome: run.outcome,
                ended,
            });
        }
        if let Some(usage) = &run.facts.usage_total {
            self.usage_runs += 1;
            self.tokens.input += usage.input;
            self.tokens.output += usage.output;
            self.tokens.cache_read += usage.cache_read;
            self.tokens.cache_write += usage.cache_write;
        }
        self.cost.add(&run.facts.cost);
        self.held_gates += u64::from(run.gate.as_ref().is_some_and(crate::GateRecord::holds_slot));
        self.held_reviews += u64::from(
            run.review
                .as_ref()
                .is_some_and(crate::ReviewRecord::holds_slot),
        );
        self.add_sockets(&run.facts.pane_refs);
        self.review_changes_unsuperseded += run.facts.changes_unsuperseded;
    }

    /// Replaces a history run's contribution after a keyed edit that kept its id, ended,
    /// usage and children; pane references are adjusted by the caller with the pane rows.
    pub fn adjust_run_head(&mut self, before: &crate::RunHead, after: &crate::RunHead) {
        let outcome = |run: &crate::RunHead| {
            run.outcome
                .and_then(|o| serde_json::to_value(o).ok())
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "active".to_owned())
        };
        let old = outcome(before);
        if let Some(n) = self.runs_by_outcome.get_mut(&old) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.runs_by_outcome.remove(&old);
            }
        }
        *self.runs_by_outcome.entry(outcome(after)).or_default() += 1;
        self.failures = self.failures - u64::from(before.failures) + u64::from(after.failures);
        let held_gate = |run: &crate::RunHead| {
            u64::from(run.gate.as_ref().is_some_and(crate::GateRecord::holds_slot))
        };
        let held_review = |run: &crate::RunHead| {
            u64::from(
                run.review
                    .as_ref()
                    .is_some_and(crate::ReviewRecord::holds_slot),
            )
        };
        self.held_gates = self.held_gates - held_gate(before) + held_gate(after);
        self.held_reviews = self.held_reviews - held_review(before) + held_review(after);
        if let Some(last) = self.last_ended_run.as_mut().filter(|l| l.id == after.id) {
            last.outcome = after.outcome;
        }
    }

    pub fn fold_plan_run_head(&mut self, run: &crate::PlanRunHead) {
        self.plan_runs += 1;
        if let Some(ended) = run.ended
            && self
                .last_ended_plan_run
                .as_ref()
                .is_none_or(|last| last.ended <= ended)
        {
            self.last_ended_plan_run = Some(EndedRun {
                id: run.id.clone(),
                outcome: None,
                ended,
            });
        }
        self.add_sockets(&run.facts.pane_refs);
    }

    pub fn fold_plan_head(&mut self, plan: &crate::PlanHead) {
        self.plans += 1;
        self.pending_plan_reviews += plan.facts.pending_children;
        self.plan_review_changes += plan.facts.changes;
        self.add_sockets(&plan.facts.pane_refs);
    }

    pub fn fold_attempt_head(&mut self, attempt: &crate::AttemptHead) {
        self.aggregate_attempts += 1;
        self.add_sockets(&attempt.facts.pane_refs);
    }

    fn add_sockets(&mut self, sockets: &BTreeMap<String, u64>) {
        for (socket, n) in sockets {
            *self.pane_refs.entry(socket.clone()).or_default() += n;
        }
    }
}
