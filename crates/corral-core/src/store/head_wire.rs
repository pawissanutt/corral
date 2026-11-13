//! The wire form of a detached [`TaskHead`]: a head that crosses a process boundary (the
//! daemon's classic board frame). Projections serialize as their stored row documents, which
//! leave out the fields the store fills after decoding; this form carries those explicitly.

use serde::{Deserialize, Serialize};

use super::head::{CurrentSeqs, TaskHead};
use super::{AttemptHead, PlanHead, PlanReviewHead, PlanRunHead, RunHead, TaskHeadFields};
use crate::backend::TaskNum;
use crate::{AggregateReview, ContainerFacts, HistorySummary};

/// A detached head that crosses a process boundary. Carries every field `TaskHead`'s
/// `PartialEq` compares, plus each skipped projection field explicitly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeadWire {
    num: TaskNum,
    rev: u64,
    task: TaskHeadFields,
    current_run_id: Option<String>,
    body: String,
    run: Option<RunWire>,
    plan_run: Option<PlanRunWire>,
    plan: Option<PlanWire>,
    attempt: Option<AttemptWire>,
    summary: HistorySummary,
    annotation_summary: crate::annotations::AnnotationSummary,
    seqs: SeqsWire,
}

/// A coding run (or review child) with its facts and its current review child.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RunWire {
    run: RunHead,
    facts: ContainerFacts,
    child: Option<Box<RunWire>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlanRunWire {
    run: PlanRunHead,
    facts: ContainerFacts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PlanWire {
    plan: PlanHead,
    facts: ContainerFacts,
    current_review: Option<PlanReviewHead>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AttemptWire {
    attempt: AttemptHead,
    facts: ContainerFacts,
    current_review: Option<AggregateReview>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SeqsWire {
    run: Option<usize>,
    run_child: Option<usize>,
    plan_run: Option<usize>,
    plan: Option<usize>,
    plan_review: Option<usize>,
    attempt: Option<usize>,
    attempt_review: Option<usize>,
}

impl RunWire {
    fn of(mut run: RunHead) -> Self {
        let child = run
            .current_child
            .take()
            .map(|child| Box::new(Self::of(*child)));
        let facts = std::mem::take(&mut run.facts);
        Self { run, facts, child }
    }

    fn into_head(self) -> RunHead {
        let mut run = self.run;
        run.facts = self.facts;
        run.current_child = self.child.map(|child| Box::new(child.into_head()));
        run
    }
}

impl PlanRunWire {
    fn of(mut run: PlanRunHead) -> Self {
        let facts = std::mem::take(&mut run.facts);
        Self { run, facts }
    }

    fn into_head(self) -> PlanRunHead {
        let mut run = self.run;
        run.facts = self.facts;
        run
    }
}

impl PlanWire {
    fn of(mut plan: PlanHead) -> Self {
        let facts = std::mem::take(&mut plan.facts);
        let current_review = plan.current_review.take();
        Self {
            plan,
            facts,
            current_review,
        }
    }

    fn into_head(self) -> PlanHead {
        let mut plan = self.plan;
        plan.facts = self.facts;
        plan.current_review = self.current_review;
        plan
    }
}

impl AttemptWire {
    fn of(mut attempt: AttemptHead) -> Self {
        let facts = std::mem::take(&mut attempt.facts);
        let current_review = attempt.current_review.take();
        Self {
            attempt,
            facts,
            current_review,
        }
    }

    fn into_head(self) -> AttemptHead {
        let mut attempt = self.attempt;
        attempt.facts = self.facts;
        attempt.current_review = self.current_review;
        attempt
    }
}

impl SeqsWire {
    fn of(seqs: &CurrentSeqs) -> Self {
        Self {
            run: seqs.run,
            run_child: seqs.run_child,
            plan_run: seqs.plan_run,
            plan: seqs.plan,
            plan_review: seqs.plan_review,
            attempt: seqs.attempt,
            attempt_review: seqs.attempt_review,
        }
    }

    fn into_seqs(self) -> CurrentSeqs {
        CurrentSeqs {
            run: self.run,
            run_child: self.run_child,
            plan_run: self.plan_run,
            plan: self.plan,
            plan_review: self.plan_review,
            attempt: self.attempt,
            attempt_review: self.attempt_review,
        }
    }
}

impl TaskHead {
    /// The wire form of this head; its view and overlay stay behind.
    pub fn to_wire(&self) -> HeadWire {
        let mut task = self.task.clone();
        let body = std::mem::take(&mut task.body);
        let current_run_id = task.current_run_id.take();
        HeadWire {
            num: self.num,
            rev: self.rev,
            task,
            current_run_id,
            body,
            run: self.current_run.as_deref().cloned().map(RunWire::of),
            plan_run: self.last_plan_run.as_deref().cloned().map(PlanRunWire::of),
            plan: self.last_plan.as_deref().cloned().map(PlanWire::of),
            attempt: self
                .last_aggregate_attempt
                .as_deref()
                .cloned()
                .map(AttemptWire::of),
            summary: self.summary.clone(),
            annotation_summary: self.annotation_summary.clone(),
            seqs: SeqsWire::of(&self.seqs),
        }
    }

    /// Detached: `view` and `overlay` are None; keyed reads fail as on any detached head.
    pub fn from_wire(wire: HeadWire) -> TaskHead {
        let mut task = wire.task;
        task.body = wire.body;
        task.current_run_id = wire.current_run_id;
        TaskHead {
            num: wire.num,
            rev: wire.rev,
            task,
            current_run: wire.run.map(|run| Box::new(run.into_head())),
            last_plan_run: wire.plan_run.map(|run| Box::new(run.into_head())),
            last_plan: wire.plan.map(|plan| Box::new(plan.into_head())),
            last_aggregate_attempt: wire.attempt.map(|attempt| Box::new(attempt.into_head())),
            summary: wire.summary,
            annotation_summary: wire.annotation_summary,
            seqs: wire.seqs.into_seqs(),
            view: None,
            overlay: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn run(id: &str) -> crate::Run {
        serde_json::from_value(json!({
            "id": id, "agent": "pi", "worktree": "/tmp/w", "branch": "corral/T-001",
            "base": "main", "base_sha": "abc", "started": "2026-10-23T00:00:00Z"
        }))
        .unwrap()
    }

    #[test]
    fn head_wire_round_trip_equals_head() {
        let tmp = tempfile::tempdir().unwrap();
        let store = crate::Store::init(tmp.path()).unwrap();
        let task = store
            .create("Wire", None, crate::Status::Review, "the body")
            .unwrap();
        store
            .update(&task.id, |t| {
                let revision = crate::plan_contract_revision(t)?;
                let mut parent = run("R1");
                parent.review_runs.push(run("R1-review1"));
                t.runs.push(parent);
                t.plan_runs.push(serde_json::from_value(json!({
                    "id": "D1", "profile": "pi", "harness": "corral-pi",
                    "spec_revision": revision, "started": "2026-10-23T00:00:00Z",
                    "output_dir": "/tmp/plan", "rounds": 0, "state": "planning"
                }))?);
                t.plans.push(serde_json::from_value(json!({
                    "id": "P1", "run": "D1", "spec_revision": revision,
                    "artifact": {
                        "run": "D1", "kind": "plan", "title": "Plan",
                        "path": "plans/P1.md", "source": "/tmp/plan.md",
                        "sha256": "abc", "bytes": 1, "added": "2026-10-23T00:00:00Z"
                    },
                    "document": {
                        "goal": "Plan", "non_goals": [], "decisions": [],
                        "affected_paths": ["core"], "contracts": [], "dependencies": [],
                        "acceptance": [], "verification": ["test"], "owner_decisions": []
                    },
                    "reviews": [{
                        "id": "PR1", "reviewer": "pi", "policy_digest": "policy",
                        "at": "2026-10-23T00:00:00Z"
                    }]
                }))?);
                Ok(())
            })
            .unwrap();
        let head = store.head(&task.id).unwrap().detach();
        let run = head.current_run.as_deref().unwrap();
        assert!(run.current_child.is_some() && run.facts.children == 1);
        assert!(head.last_plan.as_ref().unwrap().current_review.is_some());
        assert!(head.last_plan_run.is_some() && !head.task.body.is_empty());
        assert!(head.task.current_run_id.is_some());

        let wire = serde_json::to_vec(&head.to_wire()).unwrap();
        let back = TaskHead::from_wire(serde_json::from_slice(&wire).unwrap());
        assert_eq!(back, head);
        assert_eq!(back.task.body, head.task.body);
        assert_eq!(back.task.current_run_id, head.task.current_run_id);
        assert_eq!(back.seqs, head.seqs);
        assert!(back.view.is_none() && back.overlay.is_none());
    }
}
