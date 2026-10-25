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
            comments: n(task.comments.len()),
            artifacts: n(task.artifacts.len()),
            scope_revisions: n(task.approved_scope_history.len()),
            pm_receipts: n(task.agent_pm_receipts.len()),
            member_gate_revisions: task.member_gates.values().map(|g| n(g.history.len())).sum(),
            ..HistorySummary::default()
        };
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
            self.tokens.input += usage.input;
            self.tokens.output += usage.output;
            self.tokens.cache_read += usage.cache_read;
            self.tokens.cache_write += usage.cache_write;
        }
        self.cost.add(&CostTotals::of_run(run));
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
            self.tokens.input += usage.input;
            self.tokens.output += usage.output;
            self.tokens.cache_read += usage.cache_read;
            self.tokens.cache_write += usage.cache_write;
        }
        self.cost.add(&run.facts.cost);
        self.add_sockets(&run.facts.pane_refs);
        self.review_changes_unsuperseded += run.facts.changes_unsuperseded;
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
