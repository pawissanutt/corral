use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use corral_core::{GateResult, PromoteIdeas, Reviewer, Run, Status, Task, Verdict};
use serde::Deserialize;

use super::PmObservation;
use crate::{Ctx, verification};

/// Structured reviewer evidence; arbitrary review prose never authorizes acceptance.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceFinding {
    scope: String,
    base: String,
    head: String,
    finding: String,
}

/// An attributed test observation is evidence, not a replacement for the gate.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadCheck {
    version: u32,
    task: String,
    run: String,
    head: String,
    command: String,
    under_load_exit: i32,
    alone_exit: Option<i32>,
    log: String,
}

pub(crate) fn observations(ctx: &Ctx) -> Result<Vec<PmObservation>> {
    let mut tasks = ctx.store.list()?;
    tasks.sort_by(|a, b| a.created.cmp(&b.created).then(a.id.cmp(&b.id)));
    let events = ctx.store.events_shared()?;
    let mut out = Vec::new();
    let used = crate::SlotUsage::from_tasks(&tasks, &ctx.store.config.limits).total();
    let limit = ctx.store.config.limits.running;
    let operating = ctx.store.effective_operating_policy()?;
    for task in &tasks {
        if task.status == Status::Idea {
            continue;
        }
        if task.status == Status::Dropped {
            if operating.tasks.contains_key(&task.id) {
                out.push(PmObservation::Owner {
                    task: task.id.clone(),
                    reason: "Dropped task remains a scoped obligation; owner disposition or a scope amendment is required.".into(),
                });
            }
            continue;
        }
        if task.dispatch_hold {
            out.push(PmObservation::Owner {
                task: task.id.clone(),
                reason: "Task has an explicit dispatch hold; owner clearance is required.".into(),
            });
            continue;
        }
        if task.status != Status::Done
            && let Some(observation) = super::plan::observation(ctx, task)?
        {
            out.push(observation);
            continue;
        }
        if let Some(run) = task.current_run() {
            match crate::pm_questions::pending(ctx, run) {
                Ok(Some(question)) => {
                    out.push(PmObservation::Question {
                        task: task.id.clone(),
                        question: question.question,
                        source: question.source,
                        agent_state: run.agent_state.clone().unwrap_or_else(|| "unknown".into()),
                    });
                    continue;
                }
                Ok(None) => {}
                Err(error) => {
                    crate::messages::report(Some(&task.id), "pm-next", format!("{error:#}"));
                    out.push(PmObservation::Owner {
                        task: task.id.clone(),
                        reason: format!("Harness question state unavailable: {error:#}"),
                    });
                    continue;
                }
            }
            if let Some(decision) = run.obligations.pending_decision() {
                out.push(PmObservation::Question {
                    task: task.id.clone(),
                    question: decision.request.question.clone(),
                    source: format!(
                        "decision {} ({:?})",
                        decision.request.id, decision.request.source
                    ),
                    agent_state: run.agent_state.clone().unwrap_or_else(|| "unknown".into()),
                });
                continue;
            }
            if task.status == Status::Done {
                continue;
            }
            if run.pending_reply.is_some() || run.stuck.is_some() {
                out.push(PmObservation::Owner {
                    task: task.id.clone(),
                    reason: "Unresolved delivery or process recovery requires owner disposition."
                        .into(),
                });
                continue;
            }
            if task.status == Status::NeedsMe {
                out.push(PmObservation::Owner {
                    task: task.id.clone(),
                    reason: run.blocked_excerpt.clone().unwrap_or_else(|| {
                        "Owner disposition required before further work.".into()
                    }),
                });
                continue;
            }
            if let Some(reason) = crate::automation_blocker(ctx, task)? {
                out.push(PmObservation::Owner {
                    task: task.id.clone(),
                    reason,
                });
                continue;
            }
            if task.awaiting_owner() {
                match review_observation(ctx, task, run, &tasks, &events) {
                    Ok(observation) => out.push(observation),
                    Err(error) => {
                        crate::messages::report(Some(&task.id), "pm-next", format!("{error:#}"));
                        out.push(PmObservation::Owner {
                            task: task.id.clone(),
                            reason: format!("Evidence unavailable: {error:#}"),
                        });
                    }
                }
            }
        }
        if task.status == Status::NeedsMe && task.current_run().is_none() {
            out.push(PmObservation::Owner {
                task: task.id.clone(),
                reason: "Task needs owner disposition; no current worker run is available.".into(),
            });
            continue;
        }
        if (task.status == Status::Ready
            || (task.status == Status::Backlog
                && (operating.allows(&task.id, corral_core::OperatingActionKind::Dispatch)
                    || operating.allows(&task.id, corral_core::OperatingActionKind::Plan))))
            && used < limit
            && task.project.is_some()
        {
            match crate::queue::blocked_reason(ctx, &ctx.store, task, false) {
                Ok(None) => out.push(PmObservation::Dispatch {
                    task: task.id.clone(),
                }),
                Ok(Some(condition)) => out.push(PmObservation::AutomaticWait {
                    task: task.id.clone(),
                    condition,
                }),
                Err(error) => out.push(PmObservation::Owner {
                    task: task.id.clone(),
                    reason: format!("Dependency or admission unavailable: {error:#}"),
                }),
            }
        }
    }
    if crate::dispatch_policy::free_slots(ctx)? > 0 {
        let mut shortlist = Vec::new();
        for candidate in crate::promotion::promotion_shortlist(ctx)? {
            let mode = ctx
                .store
                .config
                .dispatch_policy(Some(&candidate.project))
                .promote_ideas;
            if (mode == PromoteIdeas::Suggest
                || (mode == PromoteIdeas::Auto
                    && operating.allows(&candidate.id, corral_core::OperatingActionKind::Dispatch)))
                && crate::dispatch_policy::automatic_capacity_reason(
                    &ctx.store,
                    &ctx.store.get(&candidate.id)?,
                )?
                .is_none()
            {
                shortlist.push(candidate);
            }
        }
        if !shortlist.is_empty() {
            out.push(PmObservation::IdleIdeas {
                ideas: shortlist.iter().map(|c| c.id.clone()).collect(),
                used,
                limit,
                shortlist,
            });
        }
    }
    Ok(out)
}

fn review_observation(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    tasks: &[Task],
    events: &[corral_core::Event],
) -> Result<PmObservation> {
    let id = &task.id;
    let head = corral_git::rev_sha(&run.worktree, "HEAD")?;
    let base = corral_git::rev_sha(&run.worktree, &run.base)?;
    let policy = verification::policy(ctx, task)?;
    let repair = format!(
        "Rebase onto current {} (observed {base}), resolve conflicts, commit, run the full configured gate, and obtain a fresh independent review before landing. Do not reuse stale verdicts or gate receipts.",
        run.base
    );
    let failure = run.blocked_excerpt.as_deref().unwrap_or("");
    let stale_event = events.iter().rev().find(|e| {
        e.task.as_deref() == Some(id)
            && e.detail["run"].as_str() == Some(run.id.as_str())
            && matches!(e.kind.as_str(), "landing_stale" | "gate_invalidated")
    });
    let stale_review = run.review.as_ref().is_some_and(|r| {
        r.sha != head
            || r.identity
                .as_ref()
                .is_some_and(|i| i.integration_base != base)
    });
    if stale_review || failure.contains("stale review:") || failure.contains("stale_retry:") {
        return Ok(PmObservation::StaleReview {
            task: id.clone(),
            reason: format!(
                "HEAD {head}; base {base}; {}; latest refusal: {:?}",
                failure,
                stale_event.map(|e| &e.detail)
            ),
            repair,
        });
    }
    if let Some(check) = load_check(ctx, task, run, &head)? {
        return Ok(check);
    }
    if !policy.gate.is_empty() {
        match &run.gate {
            Some(g) if g.result == GateResult::Pass => {
                if !verification::gate_passed(ctx, task, run)? {
                    return Ok(PmObservation::GateUntrusted {
                        task: id.clone(),
                        log: crate::gate_paths(ctx, id, &run.id).log.display().to_string(),
                        problem: "Pass record lacks a fresh, complete, untampered per-attempt log and receipts.".into(),
                    });
                }
            }
            Some(g) if g.result == GateResult::Running => {
                return Ok(PmObservation::AutomaticWait {
                    task: id.clone(),
                    condition:
                        "Await the running gate's collected result; do not launch another attempt."
                            .into(),
                });
            }
            Some(g) if g.result == GateResult::Fail => {
                // Delegated bounded repair is routine work, not an owner question.
                if task.rounds < policy.max_rounds
                    && ctx
                        .store
                        .effective_operating_policy()?
                        .allows(id, corral_core::OperatingActionKind::Repair)
                {
                    return Ok(PmObservation::AutomaticWait {
                        task: id.clone(),
                        condition: format!(
                            "Autopilot sends the failed gate back as repair round {} of {}.",
                            task.rounds + 1,
                            policy.max_rounds
                        ),
                    });
                }
                return Ok(PmObservation::Owner {
                    task: id.clone(),
                    reason: format!(
                        "Gate failed: {}. Send the factual failure back; never approve a failed gate.",
                        g.failed_cmd.as_deref().unwrap_or("see attempt log")
                    ),
                });
            }
            _ => return Ok(PmObservation::GateNeeded { task: id.clone() }),
        }
    }
    let pending: Vec<String> = tasks
        .iter()
        .filter(|t| t.id != *id && t.project == task.project && t.status == Status::Review)
        .filter(|t| {
            t.current_run()
                .and_then(|r| r.review.as_ref())
                .is_some_and(|r| r.verdict.is_none())
                && !t
                    .current_run()
                    .is_some_and(corral_core::Run::review_interrupted)
        })
        .map(|t| t.id.clone())
        .collect();
    if run.review.as_ref().is_some_and(|r| r.verdict.is_none()) && !run.review_interrupted() {
        return Ok(PmObservation::ReviewInFlight {
            task: id.clone(),
            reviews: vec![id.clone()],
        });
    }
    if (policy.reviewer != Reviewer::None || run.review.is_some())
        && !verification::review_fresh(ctx, task, run)?
    {
        return Ok(PmObservation::ReviewNeeded { task: id.clone() });
    }
    if let Some(review) = &run.review {
        if review.verdict == Some(Verdict::Changes)
            && let Some(notes) = &review.notes
            && let Ok(finding) = serde_json::from_str::<EvidenceFinding>(notes)
            && finding.scope == "evidence_only"
        {
            let diff = range_diff(run, &finding.base, &finding.head, &base, &head)?;
            if identical_range_diff(&diff) {
                if !pending.is_empty() {
                    return Ok(PmObservation::ReviewInFlight {
                        task: id.clone(),
                        reviews: pending,
                    });
                }
                if let Some(blocker) = landing_blocker(ctx, task, run, &policy, &repair)? {
                    return Ok(blocker);
                }
                return Ok(PmObservation::EvidenceOnly {
                    task: id.clone(),
                    range_diff: diff,
                    finding: format!("{}: {}", review.reviewer, finding.finding),
                });
            }
        }
        if review.verdict != Some(Verdict::Approve) {
            return Ok(PmObservation::Owner {
                task: id.clone(),
                reason: format!(
                    "Reviewer {}: {:?}: {}",
                    review.reviewer,
                    review.verdict,
                    review.notes.as_deref().unwrap_or("")
                ),
            });
        }
    }
    if !pending.is_empty() {
        return Ok(PmObservation::ReviewInFlight {
            task: id.clone(),
            reviews: pending,
        });
    }
    if let Some(blocker) = landing_blocker(ctx, task, run, &policy, &repair)? {
        return Ok(blocker);
    }
    Ok(PmObservation::ReadyToLand { task: id.clone() })
}

fn landing_blocker(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    policy: &corral_core::ReviewPolicy,
    repair: &str,
) -> Result<Option<PmObservation>> {
    let id = &task.id;
    if !corral_git::is_clean(&run.worktree)? {
        return Ok(Some(PmObservation::Owner {
            task: id.clone(),
            reason: "Candidate has uncommitted changes.".into(),
        }));
    }
    if let Some(project) = task.project.as_deref()
        && let Some(reason) = crate::hooks::operational_blocker(ctx, project)?
    {
        return Ok(Some(PmObservation::Owner {
            task: id.clone(),
            reason,
        }));
    }
    let key = crate::integration::RepairKey::new(task, run, "pm-next")?;
    let packet =
        crate::integration::IntegrationPacket::build(ctx, task, run, policy, key, vec![], None)?;
    if !packet.conflict_paths.is_empty() || !packet.missing_from_candidate.is_empty() {
        return Ok(Some(PmObservation::StaleReview {
            task: id.clone(),
            reason: packet.compact(),
            repair: repair.into(),
        }));
    }
    if packet.file_sizes.iter().any(|f| {
        f.candidate_sloc
            .zip(f.ceiling)
            .is_some_and(|(n, cap)| n > cap)
    }) {
        return Ok(Some(PmObservation::Owner {
            task: id.clone(),
            reason: format!(
                "Source size ceiling breached: {}",
                serde_json::to_string(&packet.file_sizes)?
            ),
        }));
    }
    if !crate::completion::evidence_failures(ctx, task, run)?.is_empty() {
        return Ok(Some(PmObservation::Owner {
            task: id.clone(),
            reason: "Required acceptance evidence is missing or stale.".into(),
        }));
    }
    Ok(None)
}

fn load_check(ctx: &Ctx, task: &Task, run: &Run, head: &str) -> Result<Option<PmObservation>> {
    for artifact in task
        .artifacts
        .iter()
        .rev()
        .filter(|a| a.run == run.id && a.title == "PM isolated test result")
    {
        let path = ctx.store.dir().join(&artifact.path);
        let bytes = crate::artifact_read(ctx, artifact)?;
        if verification::digest(&bytes) != artifact.sha256 {
            bail!("test observation artifact hash changed: {}", path.display());
        }
        let check: LoadCheck = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing {}", path.display()))?;
        if check.version != 1 || check.task != task.id || check.run != run.id || check.head != head
        {
            continue;
        }
        if check.under_load_exit != 0 {
            return Ok(Some(PmObservation::LoadFailure {
                task: task.id.clone(),
                command: check.command,
                alone_passed: check.alone_exit.map(|exit| exit == 0),
                evidence: format!(
                    "{}: load exit {}; alone exit {:?}; log {}",
                    path.display(),
                    check.under_load_exit,
                    check.alone_exit,
                    check.log
                ),
            }));
        }
    }
    Ok(None)
}

fn range_diff(run: &Run, old_base: &str, old_head: &str, base: &str, head: &str) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(&run.worktree)
        .args([
            "range-diff",
            "--no-color",
            "--no-ext-diff",
            &format!("{old_base}..{old_head}"),
            &format!("{base}..{head}"),
        ])
        .stdin(Stdio::null())
        .output()
        .context("computing review range-diff")?;
    if !output.status.success() {
        bail!(
            "range-diff failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn identical_range_diff(diff: &str) -> bool {
    !diff.trim().is_empty()
        && diff.lines().filter(|l| !l.trim().is_empty()).all(|l| {
            let fields: Vec<_> = l.split_whitespace().collect();
            fields.len() >= 5
                && fields[2] == "="
                && fields[0].ends_with(':')
                && fields[3].ends_with(':')
        })
}
