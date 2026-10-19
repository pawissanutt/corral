//! A distillation is a bounded spike, never reasoning performed in the PM's own context.
use std::collections::BTreeSet;
use std::io::Write;

use anyhow::{Context, Result, bail};
use corral_core::{Lesson, Status, Task, TaskSize};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{Ctx, LessonSignal};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LessonDue {
    pub project: String,
    pub reasons: Vec<String>,
    pub profile: Option<String>,
    pub pending: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LessonRequest {
    pub version: u32,
    pub project: String,
    pub task: String,
    pub source_digest: String,
    pub existing: Vec<Lesson>,
    pub signals: Vec<LessonSignal>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LessonProposal {
    pub version: u32,
    pub project: String,
    pub task: String,
    pub source_digest: String,
    pub lessons: Vec<Lesson>,
}

/// Read-only schedule evaluation. Active distillation tasks suppress duplicate filing.
pub fn lessons_due(ctx: &Ctx, now: Timestamp) -> Result<Vec<LessonDue>> {
    let events = ctx.store.events_shared()?;
    let tasks = ctx.store.list()?;
    let config = &ctx.store.config.lessons;
    let mut due = Vec::new();
    for project in ctx.store.config.projects.keys() {
        let requested_index = events.iter().rposition(|e| {
            e.kind == "lessons_distill_requested"
                && e.detail["project"].as_str() == Some(project.as_str())
        });
        let requested = requested_index.map(|index| &events[index]);
        let pending = requested
            .and_then(|e| e.task.as_deref())
            .and_then(|id| tasks.iter().find(|t| t.id == id))
            .filter(|t| !matches!(t.status, Status::Done | Status::Dropped))
            .map(|t| t.id.clone());
        let baseline = requested.map(|e| e.ts);
        // Count by durable append order: wall time can tie or move backwards.
        let relevant: Vec<_> = events
            .iter()
            .skip(requested_index.map_or(0, |index| index + 1))
            .filter(|e| {
                e.task.as_ref().is_some_and(|id| {
                    tasks
                        .iter()
                        .any(|t| &t.id == id && t.project.as_deref() == Some(project.as_str()))
                })
            })
            .collect();
        // Missing or unknown historical strategies do not prove a landing.
        let landings = relevant
            .iter()
            .filter(|e| {
                e.kind == "approved"
                    && matches!(
                        e.detail["strategy"].as_str(),
                        Some("fast_forward" | "rebase")
                    )
            })
            .count();
        let failures = relevant
            .iter()
            .filter(|e| {
                (e.kind == "gate" && e.detail["result"] == "fail")
                    || matches!(e.kind.as_str(), "changes_requested" | "discarded")
                    || corral_core::actionable_kind(e) == Some("escalated")
            })
            .count();
        let has_history = tasks.iter().any(|t| {
            t.project.as_deref() == Some(project.as_str())
                && (matches!(t.status, Status::Done | Status::Dropped) || !t.runs.is_empty())
        });
        let mut reasons = Vec::new();
        if let Some(every) = &config.every {
            let duration = corral_core::parse_age(every)?;
            if has_history && baseline.is_none_or(|at| now.duration_since(at) >= duration) {
                reasons.push(format!("interval {every} elapsed"));
            }
        }
        if config
            .after_landings
            .is_some_and(|n| landings >= usize::try_from(n).unwrap_or(usize::MAX))
        {
            reasons.push(format!("{landings} landings since last request"));
        }
        if config
            .after_failures
            .is_some_and(|n| failures >= usize::try_from(n).unwrap_or(usize::MAX))
        {
            reasons.push(format!("{failures} failure signals since last request"));
        }
        if !reasons.is_empty() || pending.is_some() {
            due.push(LessonDue {
                project: project.clone(),
                reasons,
                profile: config.profile.clone(),
                pending,
            });
        }
    }
    Ok(due)
}

/// One actionable wake per project/request generation, shared by all sync callers.
pub(crate) fn announce_due(ctx: &Ctx, now: Timestamp) -> Result<()> {
    let config = &ctx.store.config.lessons;
    if config.every.is_none() && config.after_landings.is_none() && config.after_failures.is_none()
    {
        return Ok(());
    }
    let Some(_lock) = ctx.store.try_lock("lessons-schedule")? else {
        return Ok(());
    };
    let events = ctx.store.events_shared()?;
    for due in lessons_due(ctx, now)?
        .into_iter()
        .filter(|due| due.pending.is_none() && !due.reasons.is_empty())
    {
        let request = events
            .iter()
            .rev()
            .find(|e| {
                e.kind == "lessons_distill_requested"
                    && e.detail["project"].as_str() == Some(due.project.as_str())
            })
            .and_then(|e| e.task.as_deref());
        if events.iter().any(|e| {
            e.kind == "lessons_due"
                && e.detail["project"].as_str() == Some(due.project.as_str())
                && e.detail["request"].as_str() == request
        }) {
            continue;
        }
        ctx.store.log_event(
            None,
            "lessons_due",
            json!({
                "project":due.project,"request":request,"profile":due.profile,
                "reason":format!("Project {} lessons are due: {}",due.project,due.reasons.join(", ")),
            }),
        )?;
    }
    Ok(())
}

fn request_path(ctx: &Ctx, task: &str) -> Result<std::path::PathBuf> {
    Ok(ctx
        .store
        .state_dir()
        .join("lessons")
        .join(format!("{}.json", corral_core::Store::normalize_id(task)?)))
}

/// File one spike on an explicit strong profile. This never launches an agent.
pub fn lessons_distill(ctx: &Ctx, project: &str, profile: Option<&str>) -> Result<Task> {
    ctx.store.check_runtime()?;
    ctx.store.project_path(project)?;
    let _lock = ctx
        .store
        .try_lock("lessons-distill")?
        .context("another lesson distillation operation is active")?;
    if let Some(pending) = lessons_due(ctx, Timestamp::now())?
        .into_iter()
        .find(|d| d.project == project)
        .and_then(|d| d.pending)
    {
        return ctx.store.get(&pending);
    }
    let profile = profile
        .or(ctx.store.config.lessons.profile.as_deref())
        .context("set [lessons].profile to a strong profile or pass --profile")?;
    let choice = corral_core::ProfileChoice {
        profile: Some(profile.to_string()),
        ..Default::default()
    };
    ctx.store.config.resolve_profile(&choice, None, None)?;
    let signals = crate::lesson_signals(ctx, project)?;
    if signals.is_empty() {
        bail!("no historical lesson signals for project {project}");
    }
    let existing = ctx.store.lessons(project)?;
    let source_digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&signals)?));
    let body = format!(
        "Distill project {project} history in one bounded strong-model synthesis pass. \
         This is a spike, not a coding task. Do not edit project code or dispatch other agents. \
         The explicit profile {profile} is the strong-model exception for this task.\n\n\
         Read the request artifact, including existing lessons and deterministic signal clusters. \
         Produce a JSON proposal with version=1, project, task, source_digest (copied exactly), \
         and lessons (the full desired collection). Each lesson has id, rule (one line), why, \
         evidence (task, ts, source copied from signals), scope (files/globs, kinds, profiles, \
         harnesses, tags), confidence (0..1), pinned, contradicted, last_seen. \
         Scope narrowly. Preserve IDs, pinned lessons and evidence; flag contradicted lessons. \
         Do not invent evidence or turn one anecdote into a high-confidence rule. \
         Old unpinned lessons decay automatically during matching; do not refresh last_seen \
         without recurring evidence. Return at most 100 lessons.\n\n\
         Apply with `corral --root {} lessons apply --project {project} --file <proposal.json>`. \
         Attach the proposal and a short report, then complete the normal run contract. \
         A stale owner edit must be reconciled, never overwritten.\n\n\
         Kind: spike. Size: small; one evidence synthesis and validated document update.",
        crate::gate_script::q(&ctx.store.root().display().to_string())
    );
    let task = crate::new_task(
        ctx,
        &format!("Distill project lessons: {project}"),
        Some(project),
        ctx.store.root(),
        Status::Backlog,
        &body,
    )?;
    let request = LessonRequest {
        version: 1,
        project: project.to_string(),
        task: task.id.clone(),
        source_digest,
        existing,
        signals,
    };
    let path = request_path(ctx, &task.id)?;
    std::fs::create_dir_all(path.parent().context("request has no directory")?)
        .context("creating lesson request directory")?;
    write_json(&path, &request)?;
    crate::add_artifact(
        ctx,
        &task.id,
        &path,
        Some(corral_core::ArtifactKind::Evidence),
        Some("Deterministic lesson distillation request"),
    )?;
    let task = ctx.store.update(&task.id, |t| {
        t.profile = Some(profile.to_string());
        t.kind = Some("spike".to_string());
        t.size = Some(TaskSize::S);
        t.body
            .push_str(&format!("\n\nRequest: {}\n", path.display()));
        Ok(())
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "lessons_distill_requested",
        json!({"project":project,"profile":profile,"source_digest":request.source_digest}),
    )?;
    Ok(task)
}

fn write_json(path: &std::path::Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("JSON destination has no parent")?;
    let mut file = tempfile::NamedTempFile::new_in(parent).context("creating lesson request")?;
    file.write_all(&serde_json::to_vec_pretty(value)?)
        .context("writing lesson request")?;
    file.persist(path)
        .with_context(|| format!("saving {}", path.display()))?;
    Ok(())
}

/// Validate model output against its captured evidence and owner-edit snapshot.
pub fn lessons_apply(ctx: &Ctx, project: &str, proposal: &LessonProposal) -> Result<()> {
    ctx.store.check_runtime()?;
    let _lock = ctx
        .store
        .try_lock("lessons-distill")?
        .context("another lesson distillation operation is active")?;
    if proposal.version != 1 || proposal.project != project || proposal.lessons.len() > 100 {
        bail!("invalid lesson proposal version, project or lesson count");
    }
    let path = request_path(ctx, &proposal.task)?;
    let request: LessonRequest = serde_json::from_str(
        &std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?,
    )
    .context("parsing lesson distillation request")?;
    if request.version != 1
        || request.project != project
        || request.task != proposal.task
        || request.source_digest != proposal.source_digest
    {
        bail!("lesson proposal does not match its request");
    }
    let events = ctx.store.events_shared()?;
    let latest = events.iter().rev().find(|e| {
        e.kind == "lessons_distill_requested" && e.detail["project"].as_str() == Some(project)
    });
    if latest.and_then(|e| e.task.as_deref()) != Some(proposal.task.as_str()) {
        bail!("lesson proposal is not the latest project distillation request");
    }
    if events
        .iter()
        .any(|e| e.kind == "lessons_distilled" && e.task.as_deref() == Some(proposal.task.as_str()))
    {
        bail!("this lesson distillation request was already applied");
    }
    let known: BTreeSet<_> = request
        .signals
        .iter()
        .flat_map(|s| &s.evidence)
        .chain(request.existing.iter().flat_map(|l| &l.evidence))
        .map(|e| (e.task.clone(), e.ts, e.source.clone()))
        .collect();
    for lesson in &proposal.lessons {
        if lesson.rule.trim().is_empty()
            || lesson.rule.lines().count() != 1
            || lesson.why.trim().is_empty()
            || lesson.evidence.is_empty()
            || lesson
                .evidence
                .iter()
                .any(|e| !known.contains(&(e.task.clone(), e.ts, e.source.clone())))
            || lesson.evidence.iter().all(|e| e.ts != lesson.last_seen)
        {
            bail!(
                "lesson {} contains missing or invented evidence/last_seen",
                lesson.id
            );
        }
    }
    for pinned in request.existing.iter().filter(|l| l.pinned) {
        if !proposal.lessons.contains(pinned) {
            bail!("pinned lesson {} must be preserved unchanged", pinned.id);
        }
    }
    ctx.store
        .save_lessons_if(project, &request.existing, &proposal.lessons)?;
    ctx.store.log_event(
        Some(&proposal.task),
        "lessons_distilled",
        json!({"project":project,"source_digest":proposal.source_digest,
            "lessons":proposal.lessons.iter().map(|l| &l.id).collect::<Vec<_>>()}),
    )
}
