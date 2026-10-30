//! Read-only, deterministic PM policy. Observations contain facts, never model recommendations.
pub(crate) mod inbox;
pub(crate) mod observe;
mod plan;
mod questions;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::gate_script::q;

/// Facts observed from the board, or supplied by a frozen replay case.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PmObservation {
    GateUntrusted {
        task: String,
        log: String,
        problem: String,
    },
    StaleReview {
        task: String,
        reason: String,
        repair: String,
        /// The reduced tier the repair re-gates on; None keeps today's full-gate plan.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tier: Option<String>,
    },
    Question {
        task: String,
        question: String,
        source: String,
        agent_state: String,
    },
    EvidenceOnly {
        task: String,
        range_diff: String,
        finding: String,
    },
    LoadFailure {
        task: String,
        command: String,
        alone_passed: Option<bool>,
        evidence: String,
    },
    IdleIdeas {
        ideas: Vec<String>,
        used: usize,
        limit: usize,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        shortlist: Vec<crate::promotion::PromotionCandidate>,
    },
    ReviewInFlight {
        task: String,
        reviews: Vec<String>,
    },
    AutomaticWait {
        task: String,
        condition: String,
    },
    Blocked {
        task: String,
        condition: Box<corral_core::attention::Condition>,
    },
    GateNeeded {
        task: String,
    },
    ReviewNeeded {
        task: String,
    },
    ReadyToLand {
        task: String,
    },
    Dispatch {
        task: String,
    },
    Owner {
        task: String,
        reason: String,
    },
    PlanDecision {
        task: String,
        plan: String,
        reason: String,
        evidence: Vec<String>,
        review_ready: bool,
        approval_ready: bool,
        notes_file: String,
    },
    /// An open Pi-native question for the PM to answer with a basis, or escalate.
    AgentQuestion {
        task: String,
        key: String,
        revision: u32,
        question_kind: String,
        blocking: bool,
        text: String,
    },
    /// A Pi-native question that waits on the owner: escalated, or unattended.
    OwnerQuestion {
        task: String,
        key: String,
        revision: u32,
        question_kind: String,
        text: String,
        reason: String,
    },
}

impl PmObservation {
    pub(crate) fn task(&self) -> Option<&str> {
        match self {
            Self::GateUntrusted { task, .. }
            | Self::StaleReview { task, .. }
            | Self::Question { task, .. }
            | Self::EvidenceOnly { task, .. }
            | Self::LoadFailure { task, .. }
            | Self::ReviewInFlight { task, .. }
            | Self::AutomaticWait { task, .. }
            | Self::Blocked { task, .. }
            | Self::GateNeeded { task }
            | Self::ReviewNeeded { task }
            | Self::ReadyToLand { task }
            | Self::Dispatch { task }
            | Self::PlanDecision { task, .. }
            | Self::AgentQuestion { task, .. }
            | Self::OwnerQuestion { task, .. }
            | Self::Owner { task, .. } => Some(task),
            Self::IdleIdeas { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PmActionKind {
    LessonsDistill,
    GateLog,
    StaleVerdict,
    Question,
    EvidenceOnly,
    LoadFailure,
    #[serde(rename = "promote", alias = "idea_promotion")]
    IdeaPromotion,
    ReviewInFlight,
    Gate,
    Review,
    Land,
    Dispatch,
    Owner,
    Wait,
    AgentQuestion,
    OwnerQuestion,
    /// An open or claimed PM inbox item.
    Inbox,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PmOption {
    pub id: String,
    pub label: String,
    pub command: String,
    pub requires_owner: bool,
}

/// Exactly one action. `command` is the recommended option, not permission to act as owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PmAction {
    pub kind: PmActionKind,
    pub task: Option<String>,
    pub why: String,
    pub command: String,
    pub verify: String,
    pub evidence: Vec<String>,
    pub options: Vec<PmOption>,
    pub recommended: String,
    pub requires_owner: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shortlist: Vec<crate::promotion::PromotionCandidate>,
}

fn option(id: &str, label: &str, command: String, requires_owner: bool) -> PmOption {
    PmOption {
        id: id.into(),
        label: label.into(),
        command,
        requires_owner,
    }
}

/// Closed choices and defaults are computed entirely in code.
pub fn pm_card(observation: &PmObservation, bin: &str) -> PmAction {
    use PmObservation::*;
    let (kind, task, why, verify, evidence, options) = match observation {
        GateUntrusted { task, log, problem } => (
            PmActionKind::GateLog,
            Some(task.clone()),
            "A pass event is not a passing per-attempt log.",
            "The new attempt ends in exactly one pass result after the final configured command.",
            vec![format!("log: {log}"), problem.clone()],
            vec![option(
                "regate",
                "Run a fresh gate",
                format!("{bin} gate {} --wait", q(task)),
                false,
            )],
        ),
        StaleReview {
            task,
            reason,
            repair,
            tier,
        } => (
            PmActionKind::StaleVerdict,
            Some(task.clone()),
            "The verdict does not bind the current integrated candidate.",
            if tier.is_some() {
                "Agent rebases, runs its one receipted tier gate as the repair names, and obtains a fresh independent review."
            } else {
                "Agent rebases, runs the full gate, and obtains a fresh independent review."
            },
            vec![reason.clone(), repair.clone()],
            vec![option(
                "send_back",
                "Send the rebase and re-gate plan",
                format!("{bin} changes {} {}", q(task), q(repair)),
                false,
            )],
        ),
        Question {
            task,
            question,
            source,
            agent_state,
        } => (
            PmActionKind::Question,
            Some(task.clone()),
            "Blocked: an unanswered harness question outranks a working pane.",
            "The owner answers the exact question; a decision receipt acknowledges delivery.",
            vec![
                format!("source: {source}"),
                format!("herdr: {agent_state}"),
                question.clone(),
            ],
            vec![option(
                "ask_owner",
                "Show the question to the owner",
                format!("{bin} show {} --json", q(task)),
                true,
            )],
        ),
        EvidenceOnly {
            task,
            range_diff,
            finding,
        } => (
            PmActionKind::EvidenceOnly,
            Some(task.clone()),
            "The finding concerns evidence only; range-diff shows identical commits.",
            "Acting owner explicitly accepts the evidence; fresh gate and review identities still apply.",
            vec![finding.clone(), range_diff.clone()],
            vec![
                option(
                    "accept_owner",
                    "Ask the acting owner to accept",
                    format!("{bin} approve {} --merge ff", q(task)),
                    true,
                ),
                option(
                    "independent_review",
                    "Obtain another independent review",
                    format!("{bin} review-run {}", q(task)),
                    false,
                ),
            ],
        ),
        LoadFailure {
            task,
            command,
            alone_passed,
            evidence,
        } => {
            let choices = if *alone_passed == Some(true) {
                vec![option(
                    "file_flake",
                    "File the reproducible load-only failure",
                    format!(
                        "{bin} new {} --kind bug --size s --body {}",
                        q(&format!("Fix load-sensitive test failure from {task}")),
                        q(&format!(
                            "## Goal\nRemove the load-sensitive failure.\n\n## Design\nInvestigate on a strong implementation profile; preserve the failing assertion.\n\n## Acceptance\nReproduce under load and pass alone and concurrently without sleeps or weakened tests.\n\n## Evidence\n{evidence}\nIsolated command: {command}"
                        ))
                    ),
                    false,
                )]
            } else {
                vec![option(
                    "rerun_alone",
                    "Request an isolated rerun with a recorded result",
                    format!(
                        "{bin} changes {} {}",
                        q(task),
                        q(&format!(
                            "Re-run the failed test alone with bounded event-based synchronization. Record the exact command, exit code, candidate SHA and log. Do not call it a flake without an isolated pass. Failed command: {command}"
                        ))
                    ),
                    false,
                )]
            };
            (
                PmActionKind::LoadFailure,
                Some(task.clone()),
                "A load failure needs an isolated result, not an invented pass.",
                "Keep the failed gate as evidence; file a flake bug only after an isolated pass; never waive the gate.",
                vec![
                    evidence.clone(),
                    format!("isolated pass: {alone_passed:?}"),
                    command.clone(),
                ],
                choices,
            )
        }
        IdleIdeas {
            ideas,
            used,
            limit,
            shortlist,
        } => (
            PmActionKind::IdeaPromotion,
            ideas.first().cloned(),
            "Which eligible Idea should the owner promote to Backlog?",
            "Only the selected Idea moves to Backlog; this readback never moves or dispatches it.",
            std::iter::once(format!("slots: {used}/{limit}; review headroom reserved"))
                .chain(shortlist.iter().map(|candidate| {
                    format!(
                        "rank {} {}: {}; predicted conflicts: {}; overlap checked: {}",
                        candidate.rank,
                        candidate.id,
                        candidate.why,
                        candidate.predicted_conflicts.join(", "),
                        candidate.overlap_checked
                    )
                }))
                .collect(),
            if shortlist.is_empty() {
                // Frozen observations without eligibility evidence cannot authorize
                // a concrete promotion. Keep their owner-scoping decision intact.
                vec![option(
                    "ask_owner",
                    "Ask the owner to scope and qualify an Idea",
                    format!("{bin} list --status idea --json"),
                    true,
                )]
            } else {
                ideas
                    .iter()
                    .map(|id| {
                        option(
                            &format!("promote_{id}"),
                            &format!("Promote {id} to Backlog"),
                            format!("{bin} move {} backlog", q(id)),
                            true,
                        )
                    })
                    .chain(std::iter::once(option(
                        "wait",
                        "Promote none; wait",
                        format!("{bin} wait --json"),
                        false,
                    )))
                    .collect()
            },
        ),
        ReviewInFlight { task, reviews } => (
            PmActionKind::ReviewInFlight,
            Some(task.clone()),
            "Landing would invalidate another review in flight.",
            "All reviews in this project finish before any landing is attempted.",
            vec![format!("pending reviews: {}", reviews.join(", "))],
            vec![option(
                "wait",
                "Wait for the review result",
                format!("{bin} wait --json"),
                false,
            )],
        ),
        AutomaticWait { task, condition } => (
            PmActionKind::Wait,
            Some(task.clone()),
            "An external condition must change before this task can progress.",
            "Recompute from fresh state after the named condition changes.",
            vec![condition.clone()],
            vec![option(
                "wait",
                "Observe the resumption condition",
                format!("{bin} wait --json"),
                false,
            )],
        ),
        Blocked { task, condition } => {
            let command = format!("{bin} show {} --json", q(task));
            let requires_owner = condition.next_action.requires_owner;
            let id = if requires_owner { "ask_owner" } else { "wait" };
            return PmAction {
                kind: if requires_owner {
                    PmActionKind::Owner
                } else {
                    PmActionKind::Wait
                },
                task: Some(task.clone()),
                why: format!("Blocked: {}", condition.reason),
                command: command.clone(),
                verify: condition.next_action.description.clone(),
                evidence: vec![
                    format!("unblocks: {}", condition.unblocker()),
                    format!(
                        "blocked since: {}",
                        condition
                            .since
                            .map_or_else(|| "unknown".into(), |at| at.to_string())
                    ),
                    format!("resume checks: {}", condition.resume.len()),
                ],
                options: vec![option(
                    id,
                    "Inspect the blocker and resume checks",
                    command,
                    requires_owner,
                )],
                recommended: id.into(),
                requires_owner,
                shortlist: vec![],
            };
        }
        GateNeeded { task } => (
            PmActionKind::Gate,
            Some(task.clone()),
            "The candidate needs a passing gate.",
            "Check the per-attempt log, final command and candidate-bound receipts.",
            vec![],
            vec![option(
                "gate",
                "Run the gate",
                format!("{bin} gate {} --wait", q(task)),
                false,
            )],
        ),
        ReviewNeeded { task } => (
            PmActionKind::Review,
            Some(task.clone()),
            "The candidate needs an independent review.",
            "A fresh, candidate-bound verdict is recorded by the configured reviewer.",
            vec![],
            vec![option(
                "review",
                "Start independent review",
                format!("{bin} review-run {}", q(task)),
                false,
            )],
        ),
        ReadyToLand { task } => (
            PmActionKind::Land,
            Some(task.clone()),
            "Fresh checks passed and no review is in flight.",
            "Approval succeeds without changing the candidate; inspect the acceptance and post-land receipts.",
            vec![],
            vec![option(
                "accept_owner",
                "Request owner acceptance",
                format!("{bin} approve {} --merge ff", q(task)),
                true,
            )],
        ),
        Dispatch { task } => (
            PmActionKind::Dispatch,
            Some(task.clone()),
            "Approved ready work fits the slot and dependency limits.",
            "A run starts on the intended profile, or dispatch refuses with a policy reason.",
            vec![],
            vec![option(
                "dispatch_owner",
                "Request dispatch authorization",
                format!("{bin} dispatch {}", q(task)),
                true,
            )],
        ),
        Owner { task, reason } => (
            PmActionKind::Owner,
            Some(task.clone()),
            "This situation needs an owner decision or a strong-model spike.",
            "The owner resolves the question or files bounded work on a strong profile; the PM does not improvise.",
            vec![reason.clone()],
            vec![option(
                "ask_owner",
                "Escalate the facts to the owner",
                format!("{bin} show {} --json", q(task)),
                true,
            )],
        ),
        AgentQuestion { .. } | OwnerQuestion { .. } => {
            return questions::card(observation, bin);
        }
        PlanDecision {
            task,
            plan,
            reason,
            evidence,
            review_ready,
            approval_ready,
            notes_file,
        } => {
            let mut choices = vec![option(
                "inspect_plan",
                "Show the pending plan decision to the owner",
                format!("{bin} plan show {} --json", q(task)),
                true,
            )];
            if *review_ready {
                choices.push(option(
                    "review_plan",
                    "Request authorization for a fresh independent plan review",
                    format!("{bin} plan review {} --json", q(task)),
                    true,
                ));
            }
            if *approval_ready {
                choices.push(option(
                    "approve_plan",
                    &format!("Owner accepts after writing resolutions to {notes_file}"),
                    format!(
                        "{bin} plan verdict {} approve --owner --plan {} --notes-file {} --json",
                        q(task),
                        q(plan),
                        q(notes_file)
                    ),
                    true,
                ));
            }
            // Both verdict commands require owner-authored notes. The path is
            // explicit, not a fabricated decision or an existing reviewer note.
            if *review_ready || *approval_ready {
                choices.push(option(
                    "request_plan_changes",
                    &format!("Owner requests changes after writing notes to {notes_file}"),
                    format!(
                        "{bin} plan verdict {} changes --owner --plan {} --notes-file {} --json",
                        q(task),
                        q(plan),
                        q(notes_file)
                    ),
                    true,
                ));
            }
            return PmAction {
                kind: PmActionKind::Owner,
                task: Some(task.clone()),
                why: reason.clone(),
                command: choices[0].command.clone(),
                verify: format!(
                    "Inspect plan {plan} and its latest review with plan show --json. \
                     Before an owner verdict, write nonempty owner notes to {notes_file}; \
                     resolve every owner decision as Resolved: <decision> => <resolution>. \
                     Acceptance requires fresh independent approval under current policy; \
                     technical assessment prose is not approval. Verify the exact plan's \
                     approval or changes verdict is recorded. No implementation is launched."
                ),
                evidence: evidence.clone(),
                options: choices,
                recommended: "inspect_plan".into(),
                requires_owner: true,
                shortlist: vec![],
            };
        }
    };
    // Frozen observations carry no grant. They are recommendations, never
    // automatic authorization; live worksets project effective permissions.
    let mut options = options;
    for choice in &mut options {
        choice.requires_owner = choice.id != "wait";
    }
    let first = &options[0];
    PmAction {
        kind,
        task,
        why: why.into(),
        command: first.command.clone(),
        verify: verify.into(),
        evidence,
        recommended: first.id.clone(),
        requires_owner: first.requires_owner,
        options,
        shortlist: match observation {
            IdleIdeas { shortlist, .. } => shortlist.clone(),
            _ => vec![],
        },
    }
}

pub(crate) fn rank(observation: &PmObservation) -> u8 {
    match observation {
        PmObservation::Question { .. }
        | PmObservation::OwnerQuestion { .. }
        | PmObservation::AgentQuestion { .. } => 0,
        PmObservation::StaleReview { .. } => 1,
        PmObservation::GateUntrusted { .. } => 2,
        PmObservation::LoadFailure { .. } => 3,
        PmObservation::EvidenceOnly { .. } => 4,
        PmObservation::Owner { .. }
        | PmObservation::PlanDecision { .. }
        | PmObservation::Blocked { .. } => 5,
        PmObservation::GateNeeded { .. } => 6,
        PmObservation::ReviewNeeded { .. } => 7,
        PmObservation::ReviewInFlight { .. } => 8,
        PmObservation::AutomaticWait { .. } => 8,
        PmObservation::ReadyToLand { .. } => 9,
        PmObservation::Dispatch { .. } => 10,
        PmObservation::IdleIdeas { .. } => 11,
    }
}

/// Shared live/replay policy. Input order breaks equal-priority ties deterministically.
pub fn pm_select(observations: &[PmObservation], bin: &str) -> PmAction {
    if let Some(observation) = observations.iter().min_by_key(|o| rank(o)) {
        return pm_card(observation, bin);
    }
    PmAction {
        kind: PmActionKind::Wait,
        task: None,
        why: "No safe PM action is pending.".into(),
        command: format!("{bin} wait --json"),
        verify: "After the next wake, run pm next again.".into(),
        evidence: vec![],
        options: vec![option(
            "wait",
            "Wait for a board event",
            format!("{bin} wait --json"),
            false,
        )],
        recommended: "wait".into(),
        requires_owner: false,
        shortlist: vec![],
    }
}

/// Does not sync, launch panes, write events, or update receipts.
pub fn pm_next(ctx: &Ctx) -> Result<PmAction> {
    Ok(crate::pm_workset(ctx)?.next)
}

pub(crate) fn enrich(ctx: &Ctx, mut action: PmAction, allow_lessons: bool) -> Result<PmAction> {
    let bin = format!(
        "{} --root {}",
        q(&crate::prompt::corral_bin()?.display().to_string()),
        q(&ctx.store.root().display().to_string())
    );
    if allow_lessons
        && matches!(
            action.kind,
            PmActionKind::Wait | PmActionKind::IdeaPromotion | PmActionKind::Dispatch
        )
        && let Some(due) = crate::lessons_due(ctx, jiff::Timestamp::now())?
            .into_iter()
            .find(|d| d.pending.is_none() && !d.reasons.is_empty())
    {
        let command = if due.profile.is_some() {
            format!("{bin} lessons distill --project {}", q(&due.project))
        } else {
            format!("{bin} lessons due --json")
        };
        action = PmAction {
            kind: PmActionKind::LessonsDistill,
            task: None,
            why: format!("Project {} lessons are due: {}.", due.project, due.reasons.join(", ")),
            command: command.clone(),
            verify: "A separate spike is filed on the explicitly configured strong profile; the PM does not synthesize lessons or dispatch it.".into(),
            evidence: vec![serde_json::to_string(&due)?],
            options: vec![option(
                "distill",
                if due.profile.is_some() { "File a lesson distillation spike" } else { "Ask owner to configure [lessons].profile" },
                command,
                due.profile.is_none(),
            )],
            recommended: "distill".into(),
            requires_owner: due.profile.is_none(),
            shortlist: vec![],
        };
    }
    if let Some(id) = &action.task {
        let task = ctx.store.get(id)?;
        let lessons = match task.current_run() {
            Some(run) => match crate::applied_lessons(ctx, &task, run)? {
                Some(lessons) => lessons,
                None => crate::lessons_for(ctx, &task, Some(run))?,
            },
            None => crate::lessons_for(ctx, &task, None)?,
        };
        if !lessons.is_empty() {
            action.evidence.push(format!(
                "project lessons: {}",
                serde_json::to_string(&lessons)?
            ));
        }
        action
            .evidence
            .push(format!("task {}: {}", task.id, task.status.as_str()));
        if let Some(run) = task.current_run() {
            action.evidence.push(format!(
                "run {}; HEAD {:?}; gate {}; review {:?}",
                run.id,
                crate::worktree_head(run),
                serde_json::to_string(&run.gate)?,
                run.review.as_ref().map(|r| (&r.sha, r.verdict))
            ));
        }
        for event in ctx
            .store
            .events_shared()?
            .iter()
            .rev()
            .filter(|e| e.task.as_deref() == Some(id))
            .take(3)
        {
            action.evidence.push(format!(
                "event {} {}: {}",
                event.ts, event.kind, event.detail
            ));
        }
    }
    Ok(action)
}
