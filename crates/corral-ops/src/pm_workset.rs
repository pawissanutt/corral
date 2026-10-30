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
    /// Open and claimed PM inbox items, oldest first.
    pub inbox: Vec<PmAction>,
    pub assessments: Vec<crate::attention::TaskAssessment>,
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
        | PmActionKind::Wait
        | PmActionKind::AgentQuestion
        | PmActionKind::OwnerQuestion
        | PmActionKind::Inbox => None,
    }
}

fn option_authority(id: &str, kind: PmActionKind) -> Option<OperatingActionKind> {
    match id {
        "independent_review" => Some(OperatingActionKind::Review),
        "review_plan" => Some(OperatingActionKind::PlanReview),
        "approve_plan" => Some(OperatingActionKind::AcceptPlan),
        "ask_owner" | "inspect_plan" | "request_plan_changes" | "file_flake" | "wait" => None,
        _ => authority(kind),
    }
}

fn option_target<'a>(
    action: PmActionKind,
    task: Option<&'a str>,
    option: &'a str,
) -> Option<&'a str> {
    match action {
        PmActionKind::IdeaPromotion => option.strip_prefix("promote_"),
        PmActionKind::LessonsDistill
        | PmActionKind::GateLog
        | PmActionKind::StaleVerdict
        | PmActionKind::Question
        | PmActionKind::EvidenceOnly
        | PmActionKind::LoadFailure
        | PmActionKind::ReviewInFlight
        | PmActionKind::Gate
        | PmActionKind::Review
        | PmActionKind::Land
        | PmActionKind::Dispatch
        | PmActionKind::Owner
        | PmActionKind::Wait
        | PmActionKind::AgentQuestion
        | PmActionKind::OwnerQuestion
        | PmActionKind::Inbox => task,
    }
}

fn project_permissions(action: &mut PmAction, policy: &EffectivePolicy) {
    for option in &mut action.options {
        if option.id == "wait" {
            option.requires_owner = false;
        } else {
            option.requires_owner = option_authority(&option.id, action.kind)
                .zip(option_target(
                    action.kind,
                    action.task.as_deref(),
                    &option.id,
                ))
                .is_none_or(|(kind, task)| !policy.allows(task, kind));
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
    pm_workset_with_policy(ctx).map(|(workset, _)| workset)
}

/// Attention bookkeeping does not change executable authority. Every other field still does.
fn task_identity(task: &Task) -> Result<Vec<u8>> {
    let mut canonical = task.clone();
    canonical.attention = corral_core::AttentionLedger::default();
    Ok(serde_json::to_vec(&canonical)?)
}

/// The workset and the effective policy it was computed from, both from one read of the board.
pub fn pm_workset_with_policy(ctx: &Ctx) -> Result<(PmWorkset, EffectivePolicy)> {
    project(ctx, None)
}

/// Reuse the current refresh's assessment; executable authority still comes from fresh policy.
pub fn pm_workset_with_attention(
    ctx: &Ctx,
    attention: &crate::attention::AttentionBoard,
) -> Result<PmWorkset> {
    project(ctx, Some(attention)).map(|(workset, _)| workset)
}

fn project(
    ctx: &Ctx,
    attention: Option<&crate::attention::AttentionBoard>,
) -> Result<(PmWorkset, EffectivePolicy)> {
    let frozen;
    let ctx = if ctx.store.is_snapshot() {
        ctx
    } else {
        frozen = Ctx {
            store: ctx.store.read_only_snapshot()?,
            ..ctx.clone()
        };
        &frozen
    };
    let policy = ctx.store.effective_operating_policy()?;
    let tasks = ctx.store.list()?;
    let fresh;
    let board = if let Some(board) = attention {
        board
    } else {
        fresh = crate::attention::board(ctx, jiff::Timestamp::now())?;
        &fresh
    };
    let assessments: Vec<_> = if policy.tasks.is_empty() {
        Vec::new()
    } else {
        board
            .tasks
            .iter()
            .filter(|row| policy.tasks.contains_key(&row.task))
            .cloned()
            .collect()
    };
    let bin = format!(
        "{} --root {}",
        crate::gate_script::q(&crate::prompt::corral_bin()?.display().to_string()),
        crate::gate_script::q(&ctx.store.root().display().to_string())
    );
    let mut observations = crate::pm_next::observe::with_assessments(ctx, &board.tasks)?;
    observations.sort_by_key(crate::pm_next::rank);
    let mut actions = Vec::new();
    let mut decisions = Vec::new();
    let mut waits = Vec::new();
    for mut observation in observations {
        if let crate::PmObservation::AutomaticWait { task, condition } = &mut observation
            && let Some(primary) = assessments
                .iter()
                .find(|row| row.task == *task)
                .and_then(|row| row.assessment.primary.as_ref())
        {
            *condition = primary.reason.clone();
        }
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
        if !policy.tasks.is_empty()
            && !policy.tasks.contains_key(&task.id)
            && action.kind != PmActionKind::IdeaPromotion
        {
            continue;
        }
        project_permissions(&mut action, &policy);
        if matches!(
            action.kind,
            PmActionKind::ReviewInFlight | PmActionKind::Wait
        ) {
            waits.push(action);
            continue;
        }
        let mut executable_options = Vec::new();
        for option in &action.options {
            if option.requires_owner {
                continue;
            }
            let Some(kind) = option_authority(&option.id, action.kind) else {
                continue;
            };
            let target = option_target(action.kind, action.task.as_deref(), &option.id)
                .and_then(|id| tasks.iter().find(|task| task.id == id))
                .context("automatic option has no task")?;
            // Repair identity follows the candidate and proof, not usage refreshes,
            // task timestamps, comments or historical worker telemetry.
            let target_identity = if action.kind == PmActionKind::StaleVerdict {
                let run = target.current_run().context("repair has no current run")?;
                serde_json::to_vec(&(
                    corral_core::operating_contract_revision(target)?,
                    target.status,
                    target.rounds,
                    (
                        &run.id,
                        &run.worktree,
                        &run.branch,
                        &run.base,
                        &run.base_sha,
                    ),
                    (
                        corral_git::rev_sha(&run.worktree, "HEAD")?,
                        corral_git::rev_sha(&run.worktree, &run.base)?,
                    ),
                    (&run.gate, &run.review),
                ))?
            } else {
                task_identity(target)?
            };
            let id = crate::verification::digest(&serde_json::to_vec(&(
                &policy.revision,
                &target_identity,
                &action,
                &option.id,
            ))?);
            executable_options.push((option.id.clone(), target.id.clone(), kind, id));
        }
        for option in &mut action.options {
            if let Some((_, _, _, id)) = executable_options
                .iter()
                .find(|(option_id, _, _, _)| option_id == &option.id)
            {
                option.command = format!("{bin} pm act {id} --json");
            }
            if option.id == action.recommended {
                action.command = option.command.clone();
            }
        }
        for (option, target, kind, id) in executable_options {
            let mut selected = action.clone();
            selected.task = Some(target);
            selected.recommended = option;
            selected.command = format!("{bin} pm act {id} --json");
            selected.requires_owner = false;
            actions.push(PmExecutable {
                id,
                authority: kind,
                action: selected,
            });
        }
        if action.requires_owner {
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
                    let condition = assessments
                        .iter()
                        .find(|row| row.task == *id)
                        .and_then(|row| {
                            row.assessment
                                .primary
                                .as_ref()
                                .map(|condition| condition.reason.clone())
                                .or_else(|| {
                                    row.assessment
                                        .progress
                                        .as_ref()
                                        .map(|progress| progress.reason.clone())
                                })
                        })
                        .unwrap_or_else(|| {
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
    for row in &assessments {
        if policy.tasks.contains_key(&row.task)
            && !waits
                .iter()
                .any(|wait| wait.task.as_ref() == Some(&row.task))
            && let Some(primary) = &row.assessment.primary
        {
            waits.push(pm_card(
                &crate::PmObservation::AutomaticWait {
                    task: row.task.clone(),
                    condition: primary.reason.clone(),
                },
                &bin,
            ));
        }
    }
    let independent_work_continues = !actions.is_empty()
        || tasks.iter().any(|task| {
            policy.tasks.contains_key(&task.id)
                && task.holds_dispatch_slot()
                && !decisions.iter().any(|d| d.task == task.id)
                && !assessments
                    .iter()
                    .any(|row| row.task == task.id && row.blocked.is_some())
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
    let inbox = crate::pm_next::inbox::cards(ctx, &bin)?;
    let next = actions
        .first()
        .map(|a| a.action.clone())
        .or_else(|| {
            decisions
                .iter()
                .find(|d| d.presentation.is_none())
                .map(|d| d.action.clone())
        })
        .or_else(|| inbox.first().cloned())
        .or_else(|| waits.first().cloned())
        .unwrap_or_else(|| crate::pm_next::pm_select(&[], &bin));
    let mut next = crate::pm_next::enrich(ctx, next, actions.is_empty())?;
    if let Some(condition) = board
        .tasks
        .iter()
        .find(|row| next.task.as_deref() == Some(&row.task))
        .and_then(|row| row.blocked.as_ref())
    {
        next.why = format!("Blocked: {}", condition.reason);
        let unblocker = format!("unblocks: {}", condition.unblocker());
        if !next.evidence.contains(&unblocker) {
            next.evidence.push(unblocker);
            next.evidence.push(format!(
                "blocked since: {}",
                condition
                    .since
                    .map_or_else(|| "unknown".into(), |at| at.to_string())
            ));
        }
    }
    let workset = PmWorkset {
        policy_revision: policy.revision.clone(),
        mode: policy.mode,
        paused: policy.paused,
        status,
        progress,
        actions,
        decisions,
        waits,
        inbox,
        assessments,
        blockers,
        independent_work_continues,
        next,
    };
    Ok((workset, policy))
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
    let before = task_identity(&ctx.store.get(task)?)?;
    crate::sync(ctx)?;
    let after = task_identity(&ctx.store.get(task)?)?;
    Ok(PmActionResult {
        outcome: if before == after {
            PmActionOutcome::Waiting
        } else {
            PmActionOutcome::Progressed
        },
        workset: pm_workset(ctx)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_pm_scope_does_not_request_whole_board_attention() {
        let tmp = tempfile::tempdir().unwrap();
        let store = corral_core::Store::init(tmp.path()).unwrap();
        let task = store.create("Unscoped", None, Status::Ready, "").unwrap();
        let ctx = crate::resources::FixedResourceProbe::ctx(store);
        let (workset, policy) = pm_workset_with_policy(&ctx).unwrap();
        assert!(policy.tasks.is_empty());
        assert!(workset.assessments.is_empty());
        // Whole-board attention remains available through its explicit read surface.
        let board = crate::attention::board(&ctx, jiff::Timestamp::now()).unwrap();
        assert_eq!(board.tasks.len(), 1);
        assert_eq!(board.tasks[0].task, task.id);
    }

    #[test]
    fn canonical_needs_me_loss_survives_an_inventory_free_board_read() {
        let tmp = tempfile::tempdir().unwrap();
        let store = corral_core::Store::init(tmp.path()).unwrap();
        let task = store
            .create_initialized("Lost", None, Status::NeedsMe, "", |task| {
                task.runs.push(serde_json::from_value(serde_json::json!({
                "id":"R1","agent":"fixture","worktree":"/not-read","branch":"unused",
                "base":"main","base_sha":"abc","started":"2026-10-25T20:00:00Z","agent_state":"gone"
            }))?);
                Ok(())
            })
            .unwrap();
        let view = crate::attention::board(
            &crate::Ctx::from_store(store),
            "2026-10-25T23:00:00Z".parse().unwrap(),
        )
        .unwrap();
        let row = view.tasks.iter().find(|row| row.task == task.id).unwrap();
        let primary = row.assessment.primary.as_ref().unwrap();
        assert_eq!(primary.kind, "agent_lost");
        assert_eq!(primary.responsible, corral_core::Responsible::Pm);
        assert_eq!(primary.since, task.status_since);
        assert!(row.assessment.progress.is_none());
    }

    #[test]
    fn legacy_lost_agent_uses_canonical_idle_time_without_inventing_status_time() {
        use crate::attention::{Neighborhood, assess};
        let tmp = tempfile::tempdir().unwrap();
        let store = corral_core::Store::init(tmp.path()).unwrap();
        let mut task = store.create("Lost", None, Status::NeedsMe, "").unwrap();
        task.status_since = None;
        task.runs.push(
            serde_json::from_value(serde_json::json!({
                "id":"R1","agent":"fixture","worktree":"/not-read","branch":"unused",
                "base":"main","base_sha":"abc","started":"2026-10-25T20:00:00Z",
                "agent_state":"gone","idle_since":"2026-10-25T21:00:00Z"
            }))
            .unwrap(),
        );
        let tasks = std::slice::from_ref(&task);
        let policy = store.effective_operating_policy().unwrap();
        let mut n = Neighborhood::new(tasks, &store.config, &policy);
        n.inventory_known = true;
        let a = assess(&task, &n, "2026-10-25T23:00:00Z".parse().unwrap());
        let condition = a
            .conditions
            .iter()
            .find(|row| row.kind == "agent_lost")
            .unwrap();
        assert_eq!(
            condition.since,
            Some("2026-10-25T21:00:00Z".parse().unwrap())
        );
        assert_eq!(condition.since_source, corral_core::SinceSource::Canonical);
        assert_eq!(condition.responsible, corral_core::Responsible::Pm);
    }

    #[test]
    fn provider_reset_wait_keeps_worker_progress_and_has_no_ladder_key() {
        use crate::attention::{Neighborhood, assess};
        let tmp = tempfile::tempdir().unwrap();
        let store = corral_core::Store::init(tmp.path()).unwrap();
        let mut task = store.create("Provider", None, Status::Running, "").unwrap();
        task.runs.push(
            serde_json::from_value(serde_json::json!({
                "id":"R1","agent":"fixture","worktree":"/not-read","branch":"unused",
                "base":"main","base_sha":"abc","started":"2026-10-25T20:00:00Z"
            }))
            .unwrap(),
        );
        let policy = store.effective_operating_policy().unwrap();
        let mut n = Neighborhood::new(std::slice::from_ref(&task), &store.config, &policy);
        let since = "2026-10-25T21:00:00Z".parse().unwrap();
        n.availability.insert(
            task.id.clone(),
            corral_core::AvailabilityEpisode {
                version: 1,
                id: "episode".into(),
                task: task.id.clone(),
                role: corral_core::ProfileRole::Implementation,
                profile: "fixture".into(),
                revision: "1".into(),
                started: since,
                attempts: Vec::new(),
                notice: None,
                exhausted: Some(corral_core::Exhaustion {
                    reason: "provider reset".into(),
                    at: since,
                    waiting_until: Some("2026-10-26T00:00:00Z".parse().unwrap()),
                }),
            },
        );
        let a = assess(&task, &n, "2026-10-25T23:00:00Z".parse().unwrap());
        assert_eq!(a.progress.unwrap().executor, "worker");
        let condition = a
            .conditions
            .iter()
            .find(|row| row.kind == "provider_wait")
            .unwrap();
        assert_eq!(condition.since, Some(since));
        assert!(condition.key.is_none());
        assert_eq!(condition.responsible, corral_core::Responsible::External);
    }
    #[test]
    fn generated_board_accounts_for_two_hundred_lifecycle_combinations() {
        use crate::attention::{Neighborhood, assess};
        use corral_core::Store;
        use serde_json::json;
        let now = "2026-10-25T23:00:00Z".parse().unwrap();
        // Given 200 tasks varying status, run closure, liveness and queue membership.
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::init(tmp.path()).unwrap();
        let seed = store.create("Seed", None, Status::Ready, "").unwrap();
        let statuses = [
            Status::Idea,
            Status::Backlog,
            Status::Ready,
            Status::Running,
            Status::NeedsMe,
            Status::Review,
            Status::Done,
            Status::Dropped,
        ];
        let tasks: Vec<_> = (0..200)
            .map(|index| {
                let mut task = seed.clone();
                task.id = format!("T-{index:03}");
                task.status = statuses[index % 8];
                task.queued = index % 3 == 0;
                if index % 4 != 0 {
                    let mut worker: corral_core::Run = serde_json::from_value(json!({
                    "id":"R1", "agent":"fixture", "worktree":"/not-a-repository",
                    "branch":"corral/T", "base":"main", "base_sha":"abc",
                    "started":"2026-10-25T20:00:00Z", "agent_state":"working"
                    }))
                    .unwrap();
                    if index % 5 == 0 {
                        worker.outcome = Some(corral_core::RunOutcome::Done);
                    }
                    if index % 7 == 0 {
                        worker.idle_no_report = true;
                    }
                    task.runs.push(worker);
                }
                task
            })
            .collect();
        let policy = EffectivePolicy {
            mode: OperatingMode::Assist,
            paused: false,
            revision: "fixture".into(),
            authorization: None,
            tasks: Default::default(),
            reasons: Vec::new(),
        };
        let n = Neighborhood::new(&tasks, &store.config, &policy);
        // When the generated board is classified.
        for task in &tasks {
            let a = assess(task, &n, now);
            // Then every scheduled unfinished task is accounted for, with evidence on any stall.
            if !matches!(task.status, Status::Done | Status::Dropped) {
                assert!(
                    a.progress.is_some() || !a.conditions.is_empty(),
                    "{}",
                    task.id
                );
            }
            if task.status == Status::Idea || (task.status == Status::Backlog && !task.queued) {
                assert_eq!(a.primary.as_ref().unwrap().kind, "not_scheduled");
                assert!(a.primary.as_ref().unwrap().key.is_none());
            }
            for condition in a.conditions.iter().filter(|c| c.kind == "unexpected_stall") {
                assert!(
                    condition
                        .reason
                        .contains(&format!("status={}", task.status.as_str()))
                );
                assert!(condition.reason.contains("stage="));
            }
        }
    }
}
