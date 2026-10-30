use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use corral_core::{GateResult, PromoteIdeas, Reviewer, Run, Status, Task, Verdict};
use serde::Deserialize;

use super::PmObservation;
use crate::{Ctx, verification};

#[path = "observe_load_checks.rs"]
pub(crate) mod load_checks;
pub(crate) use load_checks::prepare;

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
    let board = crate::attention::board(ctx, jiff::Timestamp::now())?;
    with_assessments(ctx, &board.tasks)
}

pub(crate) fn with_assessments(
    ctx: &Ctx,
    assessments: &[crate::attention::TaskAssessment],
) -> Result<Vec<PmObservation>> {
    let heads = super::heads::inputs(ctx)?;
    let mut tasks = heads.iter().map(|head| super::heads::observed_task(ctx, head))
        .collect::<Result<Vec<_>>>()?;
    tasks.sort_by(|a, b| a.created.cmp(&b.created).then(a.id.cmp(&b.id)));
    let mut out = Vec::new();
    let used =
        crate::SlotUsage::from_heads(&super::heads::open(&ctx.store)?, &ctx.store.config.limits)
            .total();
    let limit = ctx.store.config.limits.running;
    let operating = ctx.store.effective_operating_policy()?;
    for task in &tasks {
        if matches!(task.status, Status::Done | Status::Dropped)
            && !operating.tasks.contains_key(&task.id)
        {
            continue;
        }
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
        if task.tracker && task.status != Status::Done {
            match crate::plan_acceptance::checkpoint::red_card(ctx, task) {
                Ok(Some(reason)) => out.push(PmObservation::Owner {
                    task: task.id.clone(),
                    reason,
                }),
                Ok(None) => {}
                Err(error) => out.push(PmObservation::Owner {
                    task: task.id.clone(),
                    reason: format!("Plan checkpoint state unavailable: {error:#}"),
                }),
            }
        }
        if task.status != Status::Done
            && let Some(observation) = super::plan::observation(ctx, task)?
        {
            out.push(observation);
            continue;
        }
        out.extend(super::questions::observations(task));
        if let Some(run) = task.current_run() {
            // A finished task's worker is gone, so a leftover native question is stale. An
            // explicitly registered portable decision stays visible below.
            let native = if task.status == Status::Done {
                Ok(None)
            } else {
                crate::pm_questions::pending(ctx, run)
            };
            match native {
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
            let task_head = heads
                .iter()
                .find(|head| head.id == task.id)
                .context("observed task head missing")?;
            if let Some(reason) = crate::automation_status::automation_blocker_head(ctx, task_head)?
            {
                out.push(PmObservation::Owner {
                    task: task.id.clone(),
                    reason,
                });
                continue;
            }
            if task.awaiting_owner() {
                match review_observation(ctx, task, run, &tasks) {
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
    project_blocked(assessments, &mut out);
    // Each candidate's own soft/hard cap is checked below; P0 and red-main Ideas may use headroom.
    if crate::dispatch_policy::headroom_slots(ctx)? > 0 {
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
                    &ctx.store.head(&candidate.id)?.to_shell_task()?,
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

fn project_blocked(
    assessments: &[crate::attention::TaskAssessment],
    observations: &mut Vec<PmObservation>,
) {
    for task in assessments {
        let Some(condition) = &task.blocked else {
            continue;
        };
        let mut observed = false;
        for observation in observations.iter_mut() {
            if observation.task() != Some(task.task.as_str()) {
                continue;
            }
            observed = true;
            if matches!(
                observation,
                PmObservation::ReviewInFlight { .. } | PmObservation::AutomaticWait { .. }
            ) {
                *observation = PmObservation::Blocked {
                    task: task.task.clone(),
                    condition: Box::new(condition.clone()),
                };
            }
        }
        if !observed {
            observations.push(PmObservation::Blocked {
                task: task.task.clone(),
                condition: Box::new(condition.clone()),
            });
        }
    }
}

fn review_observation(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    tasks: &[Task],
) -> Result<PmObservation> {
    let id = &task.id;
    let head = corral_git::rev_sha(&run.worktree, "HEAD")?;
    let checked = super::heads::review_task(&ctx.store.head(&task.id)?, task, &head)?;
    let task = &checked;
    let run = task.current_run().context("review run missing")?;
    let id = &task.id;
    let base = corral_git::rev_sha(&run.worktree, &run.base)?;
    let policy = verification::policy(ctx, task)?;
    let gate = format!("corral gate {id} --wait");
    let repair = match crate::prompt::reduced_send_back(
        ctx,
        task,
        &corral_core::RunHead::of(run)?,
        &gate,
    )? {
        Some((tier, text)) => (
            format!(
                "Rebase onto current {} (observed {base}), resolve conflicts and commit. {text} Obtain a fresh independent review before landing. Do not reuse stale verdicts or gate receipts.",
                run.base
            ),
            Some(tier),
        ),
        None => (
            format!(
                "Rebase onto current {} (observed {base}), resolve conflicts, commit, run the full configured gate, and obtain a fresh independent review before landing. Do not reuse stale verdicts or gate receipts.",
                run.base
            ),
            None,
        ),
    };
    let failure = run.blocked_excerpt.as_deref().unwrap_or("");
    let events = super::heads::events(
        ctx,
        Some(id),
        &["landing_stale", "gate_invalidated"],
        Some(&run.id),
        1,
    )?;
    let stale_event = events.iter().rev().find(|e| {
        e.task.as_deref() == Some(id)
            && e.detail["run"].as_str() == Some(run.id.as_str())
            && matches!(e.kind.as_str(), "landing_stale" | "gate_invalidated")
    });
    // A quick approval whose full landing receipt passed is carried at approval, not re-reviewed.
    let promotable = crate::verdict_carry::promotable(ctx, task)?;
    let stale_review = !promotable
        && run.review.as_ref().is_some_and(|r| {
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
            repair: repair.0,
            tier: repair.1,
        });
    }
    if let Some(check) = load_check(ctx, task, run, &head)? {
        return Ok(check);
    }
    if !policy.gate.is_empty() {
        match &run.gate {
            Some(g) if g.result == GateResult::Pass => {
                if !verification::gate_passed(ctx, task, &corral_core::RunHead::of(run)?)? {
                    // An approved review-round receipt still needs the full landing gate.
                    let review = crate::gate_tier::GatePhase::Review;
                    let review = crate::gate_tier::requirement_for(
                        ctx,
                        task,
                        &corral_core::RunHead::of(run)?,
                        review,
                    )?;
                    if crate::gate::gate_log_passed(ctx, task, &corral_core::RunHead::of(run)?)?
                        && verification::gate_fresh_for(
                            ctx,
                            task,
                            &corral_core::RunHead::of(run)?,
                            g,
                            &review,
                        )?
                    {
                        return Ok(PmObservation::GateNeeded { task: id.clone() });
                    }
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
            Some(g) if g.result == GateResult::Stale => {
                return Ok(PmObservation::GateNeeded { task: id.clone() });
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
        && !promotable
        && !verification::review_fresh(ctx, task, &corral_core::RunHead::of(run)?)?
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
                if let Some(blocker) = landing_blocker(ctx, task, run, &policy, &repair, false)? {
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
    if let Some(blocker) = landing_blocker(ctx, task, run, &policy, &repair, promotable)? {
        return Ok(blocker);
    }
    Ok(PmObservation::ReadyToLand { task: id.clone() })
}

/// `promotable`: promotion already validated the evidence against the approval it carries.
fn landing_blocker(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    policy: &corral_core::ReviewPolicy,
    repair: &(String, Option<String>),
    promotable: bool,
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
    let key = crate::integration::RepairKey::new(task, &corral_core::RunHead::of(run)?, "pm-next")?;
    let packet = crate::integration::IntegrationPacket::build(
        ctx,
        task,
        &corral_core::RunHead::of(run)?,
        policy,
        key,
        vec![],
        None,
    )?;
    if !packet.conflict_paths.is_empty() || !packet.missing_from_candidate.is_empty() {
        return Ok(Some(PmObservation::StaleReview {
            task: id.clone(),
            reason: packet.compact(),
            repair: repair.0.clone(),
            tier: repair.1.clone(),
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
    if !promotable
        && !crate::completion::evidence_failures(ctx, task, &corral_core::RunHead::of(run)?)?
            .is_empty()
    {
        return Ok(Some(PmObservation::Owner {
            task: id.clone(),
            reason: "Required acceptance evidence is missing or stale.".into(),
        }));
    }
    Ok(None)
}

fn load_check(ctx: &Ctx, task: &Task, run: &Run, head: &str) -> Result<Option<PmObservation>> {
    load_checks::load(ctx, &task.id, &run.id, head)
}

fn range_diff(run: &Run, old_base: &str, old_head: &str, base: &str, head: &str) -> Result<String> {
    corral_git::work::record();
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

#[cfg(test)]
#[path = "observe_load_check_tests.rs"]
mod artifact_tests;
