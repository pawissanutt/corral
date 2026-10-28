//! Bounded task facets (plan T-253 section 4, "`TaskRef` and `TaskMut`"): what a helper may read
//! or change of a task, implemented by the full [`Task`] over its vectors and by
//! [`TaskHead`] / [`TaskHeadMut`] over the rows of the view or mutation they belong to.
//!
//! Neither trait has an iterator over a history vector, and no projection they return holds a
//! nested history collection: older rows are reached only by the keyed reads, pending and open
//! pages, run-scoped pages and emitted readers below, each counted on a head. A helper written
//! against these traits therefore cannot scan history at either level.
//!
//! ```
//! use corral_core::{TaskRef, Task, TaskHead};
//! fn reachable(task: &impl TaskRef) -> anyhow::Result<()> {
//!     let _ = (task.id(), task.status(), task.runs_len()?);
//!     let _ = task.current_run_head()?.map(|run| run.facts.children);
//!     let _ = task.run_head("R1")?;
//!     let _ = task.review_child("R1", "R1-review1")?;
//!     let _ = task.artifacts_of("R1")?;
//!     Ok(())
//! }
//! fn both(task: &Task, head: &TaskHead) {
//!     let _ = reachable(task);
//!     let _ = reachable(head);
//! }
//! ```
//!
//! ```compile_fail
//! fn leak(task: &impl corral_core::TaskRef) { let _ = task.runs(); }
//! ```
//!
//! ```compile_fail
//! fn leak(task: &impl corral_core::TaskRef) { let _ = &task.runs; }
//! ```
//!
//! ```compile_fail
//! fn leak(task: &impl corral_core::TaskRef) { let _ = task.plans(); }
//! ```
//!
//! ```compile_fail
//! fn leak(task: &impl corral_core::TaskRef) { let _ = task.comments(); }
//! ```
//!
//! ```compile_fail
//! fn leak(task: &impl corral_core::TaskRef) { let _ = task.artifacts(); }
//! ```
//!
//! ```compile_fail
//! fn leak(task: &impl corral_core::TaskRef) { let _ = task.aggregate_attempts(); }
//! ```
//!
//! ```compile_fail
//! fn leak(task: &impl corral_core::TaskRef) { let _ = task.approved_scope_history(); }
//! ```
//!
//! ```compile_fail
//! fn leak(task: &impl corral_core::TaskRef) {
//!     let _ = task.current_run_head().unwrap().unwrap().review_runs;
//! }
//! ```
//!
//! ```compile_fail
//! fn leak(task: &impl corral_core::TaskRef) {
//!     let _ = task.last_plan_head().unwrap().unwrap().reviews;
//! }
//! ```

use std::borrow::Cow;

use anyhow::Result;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::agent::{AgentGeneration, AgentTurn, ContainerRef};
use crate::inbox::{Recipient, SessionBinding, SessionCursor};
use crate::{
    AggregateAttempt, AggregateReview, ApprovedScopeRevision, Artifact, AttemptHead, Checkpoint,
    CheckpointMeta, CheckpointRecord, DecisionRecord, HistorySummary, InboxHead, PlanHead,
    PlanRecord, PlanReview, PlanReviewHead, PlanRun, PlanRunHead, ProfileSwitch, ResumeReceipt,
    Run, RunHead, RunObligations, TaskComment, TaskHeadFields, Usage, UsageEntry, Verdict,
    WaitRecord,
};

use crate::{Task, TaskHead, TaskHeadMut};

/// Which prompt section a finding row feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSource {
    /// A coding run's review child (the review packet's `previous_findings`).
    Review,
    /// A plan review (the `Previous plan findings` section).
    Plan,
}

/// One emitted finding, the stored document of a `findings` row. A review finding is a review
/// child whose review has notes and is not an approval; a plan finding is a plan review with a
/// verdict other than approve, notes or not, exactly as the two prompt builders select them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmittedFinding {
    pub source: FindingSource,
    /// The coding run (review findings) or plan (plan findings) the review belongs to.
    pub owner: String,
    /// The review child or plan review id.
    pub attempt: String,
    pub reviewer: String,
    pub verdict: Option<Verdict>,
    /// Review findings only: the child was superseded.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub superseded: bool,
    /// Review findings only: the reviewed head.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sha: String,
    pub notes: Option<String>,
}

impl EmittedFinding {
    /// The finding of one review child of coding run `run`, if it emits one.
    pub fn of_review_child(run: &str, child: &Run) -> Option<EmittedFinding> {
        let review = child.review.as_ref()?;
        let notes = review.notes.clone()?;
        (review.verdict != Some(Verdict::Approve)).then(|| EmittedFinding {
            source: FindingSource::Review,
            owner: run.to_owned(),
            attempt: child.id.clone(),
            reviewer: review.reviewer.clone(),
            verdict: review.verdict,
            superseded: child.outcome == Some(crate::RunOutcome::Superseded),
            sha: review.sha.clone(),
            notes: Some(notes),
        })
    }

    /// [`Self::of_review_child`] from a review child's projection.
    pub fn of_review_head(run: &str, child: &RunHead) -> Option<EmittedFinding> {
        let review = child.review.as_ref()?;
        let notes = review.notes.clone()?;
        (review.verdict != Some(Verdict::Approve)).then(|| EmittedFinding {
            source: FindingSource::Review,
            owner: run.to_owned(),
            attempt: child.id.clone(),
            reviewer: review.reviewer.clone(),
            verdict: review.verdict,
            superseded: child.outcome == Some(crate::RunOutcome::Superseded),
            sha: review.sha.clone(),
            notes: Some(notes),
        })
    }

    /// The finding of one review of plan `plan`, if it emits one.
    pub fn of_plan_review(
        plan: &str,
        id: &str,
        reviewer: &str,
        verdict: Option<Verdict>,
        notes: Option<&str>,
    ) -> Option<EmittedFinding> {
        verdict
            .is_some_and(|v| v != Verdict::Approve)
            .then(|| EmittedFinding {
                source: FindingSource::Plan,
                owner: plan.to_owned(),
                attempt: id.to_owned(),
                reviewer: reviewer.to_owned(),
                verdict,
                superseded: false,
                sha: String::new(),
                notes: notes.map(str::to_owned),
            })
    }
}

/// Every finding a full task emits, in prompt order, with its row key.
pub fn task_findings(task: &Task) -> Vec<(String, EmittedFinding)> {
    let mut out = Vec::new();
    for (i, run) in task.runs.iter().enumerate() {
        for (c, child) in run.review_runs.iter().enumerate() {
            if let Some(finding) = EmittedFinding::of_review_child(&run.id, child) {
                out.push((finding_key(FindingSource::Review, i + 1, c + 1), finding));
            }
        }
    }
    for (i, plan) in task.plans.iter().enumerate() {
        for (r, review) in plan.reviews.iter().enumerate() {
            if let Some(finding) = EmittedFinding::of_plan_review(
                &plan.id,
                &review.id,
                &review.reviewer,
                review.verdict,
                review.notes.as_deref(),
            ) {
                out.push((finding_key(FindingSource::Plan, i + 1, r + 1), finding));
            }
        }
    }
    out
}

/// The `findings` row key of child `child` of container `container` (both 1-based).
pub fn finding_key(source: FindingSource, container: usize, child: usize) -> String {
    let prefix = match source {
        FindingSource::Review => "run",
        FindingSource::Plan => "plan",
    };
    format!("{prefix}:{container:06}/{child:06}")
}

macro_rules! head_fields {
    ($($name:ident: $ty:ty),* $(,)?) => {
        /// One accessor per non-history task field, served by the full task and by heads.
        pub trait TaskFieldsRef {
            $(fn $name(&self) -> &$ty;)*
        }

        /// Copies every non-history field of `task` into head fields.
        pub fn task_head_fields(task: &Task) -> TaskHeadFields {
            TaskHeadFields {
                $($name: task.$name.clone(),)*
                member_gates: task
                    .member_gates
                    .iter()
                    .map(|(member, gate)| (member.clone(), crate::MemberGateHead {
                        revision: gate.revision,
                        gate: gate.gate.clone(),
                        reason: gate.reason.clone(),
                        by: gate.by.clone(),
                        at: gate.at,
                    }))
                    .collect(),
            }
        }

        /// Writes head fields back into `task`; member gates change only through amendments.
        pub(crate) fn set_task_head_fields(task: &mut Task, fields: TaskHeadFields) -> Result<()> {
            anyhow::ensure!(
                fields.member_gates == task_head_fields(task).member_gates,
                "member gates change only through a member-gate amendment"
            );
            $(task.$name = fields.$name;)*
            Ok(())
        }

        impl TaskFieldsRef for Task {
            $(fn $name(&self) -> &$ty { &self.$name })*
        }

        impl TaskFieldsRef for TaskHead {
            $(fn $name(&self) -> &$ty { &self.task.$name })*
        }

        impl TaskFieldsRef for TaskHeadMut {
            $(fn $name(&self) -> &$ty { &self.head().task.$name })*
        }
    };
}

head_fields! {
    dashboard: Option<crate::DashboardSchedule>,
    origin: Option<crate::Origin>,
    idea_decision: Option<crate::IdeaDecision>,
    completion: Option<crate::CriteriaManifest>,
    result: Option<crate::TaskResult>,
    id: String,
    title: String,
    status: crate::Status,
    project: Option<String>,
    kind: Option<String>,
    size: Option<crate::TaskSize>,
    priority: String,
    depends_on: Vec<String>,
    labels: Vec<String>,
    write_scope: Vec<String>,
    profile: Option<String>,
    reviewer: Option<String>,
    queued: bool,
    dispatch_plan: Option<crate::DispatchPlan>,
    launch: Option<crate::LaunchReservation>,
    dispatch_hold: bool,
    review_reservation: Option<crate::ReviewWork>,
    review_wait: Option<crate::ReviewWork>,
    review: Option<crate::Accept>,
    plan_required: Option<bool>,
    design: bool,
    plan: Option<String>,
    tracker: bool,
    approved_scope: Option<crate::ApprovedScope>,
    approved_scope_revision: u64,
    parent: Option<String>,
    source_digest: Option<String>,
    children: Vec<String>,
    rounds: u32,
    created: Timestamp,
    updated: Timestamp,
    agent_origin: Option<crate::agent::AgentOrigin>,
    body: String,
}

/// Read access to a task without its history: one accessor per non-history field, the
/// current-container projections, summary facts, keyed rows and counted pages.
pub trait TaskRef: TaskFieldsRef {
    /// The current coding run as a projection, with its current review child.
    fn current_run_head(&self) -> Result<Option<Cow<'_, RunHead>>>;
    fn last_plan_run_head(&self) -> Result<Option<Cow<'_, PlanRunHead>>>;
    /// The last plan record with its current review.
    fn last_plan_head(&self) -> Result<Option<Cow<'_, PlanHead>>>;
    /// The last aggregate attempt with its current review.
    fn last_aggregate_attempt_head(&self) -> Result<Option<Cow<'_, AttemptHead>>>;
    /// The history summary: facts over every history row.
    fn history_summary(&self) -> Result<Cow<'_, HistorySummary>>;
    fn runs_len(&self) -> Result<usize>;
    fn plan_runs_len(&self) -> Result<usize>;
    fn plans_len(&self) -> Result<usize>;
    fn aggregate_attempts_len(&self) -> Result<usize>;
    fn comments_len(&self) -> Result<usize>;
    /// The operating-contract revision, whose digest covers the scope-amendment history.
    fn contract_revision(&self) -> Result<String>;
    /// Every non-history field, member gates without their superseded revisions.
    fn head_fields(&self) -> Result<Cow<'_, TaskHeadFields>>;
    /// The head fields with each current container as its vector's only element and nested
    /// history empty but the current child, for checks that read nothing older.
    fn shell_task(&self) -> Result<Task> {
        crate::store::shell_of(self)
    }

    /// The coding run `id` (the current one with its current child, an earlier one
    /// without), at most one history row.
    fn run_head(&self, id: &str) -> Result<Option<RunHead>>;
    fn plan_run_head(&self, id: &str) -> Result<Option<PlanRunHead>>;
    fn plan_head(&self, id: &str) -> Result<Option<PlanHead>>;
    fn aggregate_attempt_head(&self, id: &str) -> Result<Option<AttemptHead>>;
    fn has_plan(&self, id: &str) -> Result<bool>;
    /// The coding run before the current one (`runs.iter().rev().nth(1)`).
    fn previous_run_head(&self) -> Result<Option<RunHead>>;

    fn review_child(&self, run: &str, id: &str) -> Result<Option<RunHead>>;
    fn review_child_by_review_at(&self, run: &str, at: Timestamp) -> Result<Option<RunHead>>;
    fn stronger_source(
        &self,
        run: &str,
        sha: &str,
        exclude: Option<&str>,
    ) -> Result<Option<String>>;
    fn plan_review(&self, plan: &str, id: &str) -> Result<Option<PlanReviewHead>>;
    fn plan_review_at(&self, plan: &str, n: usize) -> Result<Option<PlanReviewHead>>;
    fn plan_review_approved_by(&self, plan: &str, reviewer: &str)
    -> Result<Option<PlanReviewHead>>;
    fn aggregate_review(&self, attempt: &str, id: &str) -> Result<Option<AggregateReview>>;
    fn wait(&self, run: &str, id: &str) -> Result<Option<WaitRecord>>;
    fn decision(&self, run: &str, id: &str) -> Result<Option<DecisionRecord>>;
    fn receipt(&self, run: &str, id: &str) -> Result<Option<ResumeReceipt>>;
    fn checkpoint_by_artifact(&self, run: &str, artifact: &str) -> Result<Option<CheckpointMeta>>;
    fn effect_applied(&self, recipient: &Recipient, seq: u64) -> Result<bool>;
    fn observe_cursor(
        &self,
        recipient: &Recipient,
        adapter: &str,
        binding: &SessionBinding,
    ) -> Result<Option<SessionCursor>>;
    /// The inbox scalars of `recipient`'s container on this task.
    fn inbox_head(&self, recipient: &Recipient) -> Result<Option<InboxHead>>;
    /// Generation `n` of the agent bound to `container` on this task.
    fn agent_generation(&self, container: &ContainerRef, n: u32)
    -> Result<Option<AgentGeneration>>;
    /// Turn `seq` of generation `generation` of the agent bound to `container`.
    fn agent_turn(
        &self,
        container: &ContainerRef,
        generation: u32,
        seq: u64,
    ) -> Result<Option<AgentTurn>>;

    fn open_review_children(&self, run: &str) -> Result<Vec<RunHead>>;
    fn pending_plan_reviews(&self, plan: &str) -> Result<Vec<PlanReviewHead>>;
    fn pending_aggregate_reviews(&self, attempt: &str) -> Result<Vec<AggregateReview>>;
    fn open_obligations(&self, run: &str) -> Result<RunObligations>;
    /// Checkpoint metadata of `run`, newest first, read as the caller consumes it.
    fn checkpoint_metas_newest_first(
        &self,
        run: &str,
    ) -> Result<Box<dyn Iterator<Item = Result<CheckpointMeta>> + '_>>;
    fn checkpoint_payload(&self, run: &str, ordinal: u64) -> Result<Option<Checkpoint>>;
    /// Artifacts registered to `run`, in registration order.
    fn artifacts_of(&self, run: &str) -> Result<Vec<Artifact>>;

    /// Every owner comment, for the Owner comments section only.
    fn emitted_comments(&self) -> Result<Vec<TaskComment>>;
    /// Every finding of `source` except the review child or plan review `exclude`, for
    /// `previous_findings` and Previous plan findings only.
    fn emitted_findings(
        &self,
        source: FindingSource,
        exclude: Option<&str>,
    ) -> Result<Vec<EmittedFinding>>;
    /// Every wait and decision of `run`, for the resume packet only.
    fn emitted_obligations(&self, run: &str) -> Result<RunObligations>;
}

/// Mutation of a task without access to its history: head fields and current containers,
/// append operations, keyed edits of single rows and the child mutations. Every keyed edit
/// refuses a change of the row's id or ordering fields (`started`, `ended`), and of fields that
/// only their own mutation keeps consistent (usage, nested children, agent records, inbox).
pub trait TaskMut: TaskRef {
    fn edit_head_fields(
        &mut self,
        edit: impl FnOnce(&mut TaskHeadFields) -> Result<()>,
    ) -> Result<()>;
    fn start_run(&mut self, run: Run) -> Result<()>;
    fn start_plan_run(&mut self, run: PlanRun) -> Result<()>;
    fn push_plan(&mut self, plan: PlanRecord) -> Result<()>;
    fn push_aggregate_attempt(&mut self, attempt: AggregateAttempt) -> Result<()>;
    fn add_comment(&mut self, comment: TaskComment) -> Result<()>;
    fn push_scope_revision(&mut self, revision: ApprovedScopeRevision) -> Result<()>;

    /// Edits coding run `id` (current or earlier) as its projection; one row is written.
    fn edit_run(&mut self, id: &str, edit: impl FnOnce(&mut RunHead) -> Result<()>) -> Result<()>;
    fn edit_plan_run(
        &mut self,
        id: &str,
        edit: impl FnOnce(&mut PlanRunHead) -> Result<()>,
    ) -> Result<()>;
    fn edit_plan(&mut self, id: &str, edit: impl FnOnce(&mut PlanHead) -> Result<()>)
    -> Result<()>;
    fn edit_aggregate_attempt(
        &mut self,
        id: &str,
        edit: impl FnOnce(&mut AttemptHead) -> Result<()>,
    ) -> Result<()>;
    /// Edits the inbox scalars of `recipient`'s current container.
    fn inbox_state_mut<R>(
        &mut self,
        recipient: &Recipient,
        edit: impl FnOnce(&mut InboxHead) -> Result<R>,
    ) -> Result<R>;

    fn push_review_child(&mut self, run: &str, child: Run) -> Result<()>;
    fn edit_review_child(
        &mut self,
        run: &str,
        id: &str,
        edit: impl FnOnce(&mut Run) -> Result<()>,
    ) -> Result<()>;
    fn push_plan_review(&mut self, plan: &str, review: PlanReview) -> Result<()>;
    fn edit_plan_review(
        &mut self,
        plan: &str,
        id: &str,
        edit: impl FnOnce(&mut PlanReview) -> Result<()>,
    ) -> Result<()>;
    fn pop_current_plan_review(&mut self, plan: &str) -> Result<()>;
    fn terminalize_pending_plan_reviews(&mut self, plan: &str, reason: &str) -> Result<()>;
    fn push_aggregate_review(&mut self, attempt: &str, review: AggregateReview) -> Result<()>;
    fn edit_aggregate_review(
        &mut self,
        attempt: &str,
        id: &str,
        edit: impl FnOnce(&mut AggregateReview) -> Result<()>,
    ) -> Result<()>;
    fn supersede_pending_aggregate_reviews(&mut self, attempt: &str, by: &str) -> Result<()>;
    fn push_checkpoint(&mut self, run: &str, record: CheckpointRecord) -> Result<()>;
    fn restore_checkpoints(&mut self, run: &str, records: Vec<CheckpointRecord>) -> Result<()>;
    fn push_switch(&mut self, run: &str, switch: ProfileSwitch) -> Result<()>;
    fn push_wait(&mut self, run: &str, wait: WaitRecord) -> Result<()>;
    fn push_decision(&mut self, run: &str, decision: DecisionRecord) -> Result<()>;
    fn edit_wait(
        &mut self,
        run: &str,
        id: &str,
        edit: impl FnOnce(&mut WaitRecord) -> Result<()>,
    ) -> Result<()>;
    fn edit_decision(
        &mut self,
        run: &str,
        id: &str,
        edit: impl FnOnce(&mut DecisionRecord) -> Result<()>,
    ) -> Result<()>;
    fn mark_effect_applied(&mut self, recipient: &Recipient, effect: u64) -> Result<()>;
    fn set_observe_cursor(&mut self, recipient: &Recipient, cursor: SessionCursor) -> Result<()>;
    fn append_usage_entries(
        &mut self,
        run: &str,
        child: Option<&str>,
        entries: Vec<UsageEntry>,
    ) -> Result<()>;
    fn set_usage(&mut self, run: &str, child: Option<&str>, usage: Usage) -> Result<()>;
}
