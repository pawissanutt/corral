//! Read-only coordination projection. Cards recommend; effective policy authorizes.
use anyhow::{Context, Result};
use corral_core::{EffectivePolicy, OperatingActionKind, OperatingMode, Status, Task};
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::pm_next::{PmAction, PmActionKind, pm_card};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatingStatus {
    Working,
    Waiting,
    NeedsYourDecision,
    Complete,
}

impl OperatingStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Working => "Working",
            Self::Waiting => "Waiting",
            Self::NeedsYourDecision => "Needs your decision",
            Self::Complete => "Complete",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ScopedProgress {
    pub total: usize,
    pub accepted: usize,
    pub running: usize,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PmExecutable {
    pub id: String,
    pub action: PmAction,
    pub authority: OperatingActionKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PmDecision {
    pub id: String,
    pub task: String,
    pub run: Option<String>,
    pub source: String,
    pub request_digest: String,
    pub action: PmAction,
    pub presentation: Option<corral_core::OperatingPresentation>,
}

#[derive(Debug, Serialize)]
pub struct PmWorkset {
    pub policy_revision: String,
    pub mode: OperatingMode,
    pub paused: bool,
    pub status: OperatingStatus,
    pub progress: ScopedProgress,
    pub actions: Vec<PmExecutable>,
    pub decisions: Vec<PmDecision>,
    pub waits: Vec<PmAction>,
    pub blockers: Vec<String>,
    pub independent_work_continues: bool,
    pub next: PmAction,
}

pub(crate) fn authority(kind: PmActionKind) -> Option<OperatingActionKind> {
    match kind {
        PmActionKind::GateLog | PmActionKind::Gate => Some(OperatingActionKind::Gate),
        PmActionKind::StaleVerdict | PmActionKind::LoadFailure => Some(OperatingActionKind::Repair),
        PmActionKind::Review => Some(OperatingActionKind::Review),
        PmActionKind::Land => Some(OperatingActionKind::Accept),
        PmActionKind::Dispatch | PmActionKind::IdeaPromotion => Some(OperatingActionKind::Dispatch),
        PmActionKind::LessonsDistill
        | PmActionKind::Question
        | PmActionKind::EvidenceOnly
        | PmActionKind::ReviewInFlight
        | PmActionKind::Owner
        | PmActionKind::Wait => None,
    }
}

fn option_authority(id: &str, kind: PmActionKind) -> Option<OperatingActionKind> {
    match id {
        "independent_review" => Some(OperatingActionKind::Review),
        "review_plan" => Some(OperatingActionKind::PlanReview),
        "approve_plan" => Some(OperatingActionKind::AcceptPlan),
        "ask_owner" | "inspect_plan" | "request_plan_changes" | "file_flake" => None,
        _ => authority(kind),
    }
}

fn project_permissions(action: &mut PmAction, policy: &EffectivePolicy, task: &Task) {
    for option in &mut action.options {
        if option.id == "wait" {
            option.requires_owner = false;
        } else {
            option.requires_owner = option_authority(&option.id, action.kind)
                .is_none_or(|kind| !policy.allows(&task.id, kind));
            if !option.requires_owner {
                option.label = match option.id.as_str() {
                    "dispatch_owner" => "Start delegated work".into(),
                    "accept_owner" => "Accept verified delegated work".into(),
                    "approve_plan" => "Accept the delegated reviewed plan".into(),
                    "review_plan" => "Start delegated independent plan review".into(),
                    _ => option.label.clone(),
                };
            }
        }
    }
    action.requires_owner = action
        .options
        .iter()
        .find(|option| option.id == action.recommended)
        .is_none_or(|option| option.requires_owner);
    if action.kind == PmActionKind::Owner
        && let Some(option) = ["approve_plan", "review_plan"].iter().find_map(|id| {
            action
                .options
                .iter()
                .find(|o| &o.id == id && !o.requires_owner)
        })
    {
        action.recommended = option.id.clone();
        action.command = option.command.clone();
        action.requires_owner = false;
        action.verify = "Inspect the exact plan's fresh review and acceptance using plan show --json; no owner-authored verdict or fabricated decision response is supplied.".into();
    }
}

fn decision(ctx: &Ctx, task: &Task, action: PmAction) -> Result<PmDecision> {
    let run = task.current_run();
    let (source, request) = if action.kind == PmActionKind::Question {
        if let Some(native) = run
            .map(|run| crate::pm_questions::pending(ctx, run))
            .transpose()?
            .flatten()
        {
            (native.source, native.question)
        } else if let Some(portable) = run.and_then(|r| r.obligations.pending_decision()) {
            (
                format!("portable:{}", portable.request.id),
                serde_json::to_string(&portable.request)?,
            )
        } else {
            ("question".into(), serde_json::to_string(&action.evidence)?)
        }
    } else {
        (
            format!("{:?}", action.kind),
            serde_json::to_string(&(
                &action.why,
                &action.evidence,
                &action.recommended,
                task.dispatch_hold,
            ))?,
        )
    };
    let run = run
        .map(|r| r.id.clone())
        .or_else(|| task.plan_runs.last().map(|r| r.id.clone()));
    let request_digest =
        crate::verification::digest(&serde_json::to_vec(&(&task.id, &run, &source, &request))?);
    let id = format!("decision-{request_digest}");
    let presentation = ctx.store.operating_state()?.presentations.get(&id).cloned();
    Ok(PmDecision {
        id,
        task: task.id.clone(),
        run,
        source,
        request_digest,
        action,
        presentation,
    })
}

/// One shared, side-effect-free view for PM next, brief and operating surfaces.
pub fn pm_workset(ctx: &Ctx) -> Result<PmWorkset> {
    let policy = ctx.store.effective_operating_policy()?;
    let tasks = ctx.store.list()?;
    let bin = format!(
        "{} --root {}",
        crate::gate_script::q(&crate::prompt::corral_bin()?.display().to_string()),
        crate::gate_script::q(&ctx.store.root().display().to_string())
    );
    let mut observations = crate::pm_next::observe::observations(ctx)?;
    observations.sort_by_key(crate::pm_next::rank);
    let mut actions = Vec::new();
    let mut decisions = Vec::new();
    let mut waits = Vec::new();
    for observation in observations {
        let mut action = pm_card(&observation, &bin);
        let task = action
            .task
            .as_deref()
            .and_then(|id| tasks.iter().find(|task| task.id == id));
        let Some(task) = task else {
            // No task-bound authority can arise from a workspace recommendation.
            action.requires_owner = true;
            decisions.extend(crate::pm_decisions::workspace_decision(ctx, action)?);
            continue;
        };
        if !policy.tasks.is_empty() && !policy.tasks.contains_key(&task.id) {
            continue;
        }
        project_permissions(&mut action, &policy, task);
        if matches!(
            action.kind,
            PmActionKind::ReviewInFlight | PmActionKind::Wait
        ) {
            waits.push(action);
            continue;
        }
        let selected = action.options.iter().find(|o| o.id == action.recommended);
        let kind = selected.and_then(|o| option_authority(&o.id, action.kind));
        if let Some(kind) = kind.filter(|_| !action.requires_owner) {
            let id = crate::verification::digest(&serde_json::to_vec(&(
                &policy.revision,
                task,
                &action,
            ))?);
            action.command = format!("{bin} pm act {id} --json");
            if let Some(option) = action
                .options
                .iter_mut()
                .find(|o| o.id == action.recommended)
            {
                option.command = action.command.clone();
            }
            actions.push(PmExecutable {
                id,
                authority: kind,
                action,
            });
        } else {
            decisions.push(decision(ctx, task, action)?);
        }
    }
    crate::pm_decisions::merge_pending(ctx, &mut decisions)?;
    if !policy.tasks.is_empty() {
        decisions.retain(|decision| policy.tasks.contains_key(&decision.task));
    }
    let mut progress = ScopedProgress {
        total: policy.tasks.len(),
        ..Default::default()
    };
    let mut blockers = policy.reasons.clone();
    for id in policy.tasks.keys() {
        match tasks.iter().find(|task| &task.id == id) {
            Some(task) if task.status == Status::Done => {
                if !task.dispatch_hold
                    && !decisions.iter().any(|d| d.task == task.id)
                    && task.children.iter().all(|child| {
                        policy.tasks.contains_key(child)
                            && tasks
                                .iter()
                                .any(|t| &t.id == child && t.status == Status::Done)
                    })
                    && task.current_run().is_none_or(|run| run.outcome.is_some())
                    && task
                        .project
                        .as_deref()
                        .map(|project| crate::hooks::operational_blocker(ctx, project))
                        .transpose()?
                        .flatten()
                        .is_none()
                {
                    progress.accepted += 1;
                } else {
                    blockers.push(format!(
                        "{id}: implementation or operational completion pending"
                    ));
                }
            }
            Some(task) => {
                if task.holds_dispatch_slot() {
                    progress.running += 1;
                }
                if !actions.iter().any(|a| a.action.task.as_ref() == Some(id))
                    && !decisions.iter().any(|d| &d.task == id)
                {
                    let reason = if matches!(task.status, Status::Ready | Status::Backlog) {
                        crate::queue::blocked_reason(ctx, &ctx.store, task, false)
                            .unwrap_or_else(|error| {
                                Some(format!("dependency or admission unavailable: {error:#}"))
                            })
                            .or(crate::dispatch_policy::automatic_capacity_reason(
                                &ctx.store, task,
                            )?)
                            .or(crate::resources::read_admission(ctx)?)
                    } else {
                        None
                    };
                    let condition = reason.unwrap_or_else(|| {
                        format!(
                            "{}; await an existing executor or worker receipt",
                            task.status.as_str()
                        )
                    });
                    blockers.push(format!("{id}: {condition}"));
                    if !waits.iter().any(|wait| wait.task.as_ref() == Some(id)) {
                        waits.push(pm_card(
                            &crate::PmObservation::AutomaticWait {
                                task: id.clone(),
                                condition,
                            },
                            &bin,
                        ));
                    }
                }
            }
            None => {
                progress.missing.push(id.clone());
                blockers.push(format!("{id}: scoped task missing"));
            }
        }
    }
    let independent_work_continues = !actions.is_empty()
        || tasks.iter().any(|task| {
            policy.tasks.contains_key(&task.id)
                && task.holds_dispatch_slot()
                && !decisions.iter().any(|d| d.task == task.id)
        });
    let status = if !decisions.is_empty() || policy.paused || !progress.missing.is_empty() {
        OperatingStatus::NeedsYourDecision
    } else if progress.total > 0 && progress.accepted == progress.total && blockers.is_empty() {
        OperatingStatus::Complete
    } else if independent_work_continues {
        OperatingStatus::Working
    } else {
        OperatingStatus::Waiting
    };
    let next = actions
        .first()
        .map(|a| a.action.clone())
        .or_else(|| {
            decisions
                .iter()
                .find(|d| d.presentation.is_none())
                .map(|d| d.action.clone())
        })
        .or_else(|| waits.first().cloned())
        .unwrap_or_else(|| crate::pm_next::pm_select(&[], &bin));
    let next = crate::pm_next::enrich(ctx, next, actions.is_empty())?;
    Ok(PmWorkset {
        policy_revision: policy.revision,
        mode: policy.mode,
        paused: policy.paused,
        status,
        progress,
        actions,
        decisions,
        waits,
        blockers,
        independent_work_continues,
        next,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PmActionOutcome {
    Stale,
    Progressed,
    Waiting,
}

#[derive(Debug, Serialize)]
pub struct PmActionResult {
    pub outcome: PmActionOutcome,
    pub workset: PmWorkset,
}

/// Recompute a revision-bound action, then use the existing authorized sync executors.
/// Arbitrary card command text is never executed and owner operations are unreachable.
pub fn pm_act(ctx: &Ctx, id: &str) -> Result<PmActionResult> {
    let current = pm_workset(ctx)?;
    let Some(selected) = current.actions.iter().find(|action| action.id == id) else {
        return Ok(PmActionResult {
            outcome: PmActionOutcome::Stale,
            workset: current,
        });
    };
    let task = selected
        .action
        .task
        .as_deref()
        .context("automatic action has no task")?;
    let before = serde_json::to_vec(&ctx.store.get(task)?)?;
    crate::sync(ctx)?;
    let after = serde_json::to_vec(&ctx.store.get(task)?)?;
    Ok(PmActionResult {
        outcome: if before == after {
            PmActionOutcome::Waiting
        } else {
            PmActionOutcome::Progressed
        },
        workset: pm_workset(ctx)?,
    })
}
