//! Canonical owner inbox and question details; evidence references never authorize host paths.
use anyhow::{Context, Result, bail};
use corral_core::agent::{
    AnswerBy, ContainerRef, QuestionActor, QuestionEvidenceRef, QuestionState,
};
use corral_core::{TaskHead, TaskRef};
use corral_protocol::detail::{
    DetailBody, EvidenceBlock, EvidenceBody, Followup, QuestionAnswer, QuestionDeadline,
    QuestionDetail, QuestionFrom, QuestionHistory, QuestionOption, QuestionPlace, QuestionWaiting,
    WaitKind,
};
use corral_protocol::rows::{InboxKind, InboxRow, InboxState};
use std::collections::BTreeMap;

use super::{revision, short, tasks::excerpt};
use crate::{Ctx, questions::QuestionView};

pub fn inbox_rows(ctx: &Ctx) -> Result<Vec<InboxRow>> {
    // Pending discovery uses unfinished native head indexes, not every historical run.
    let views = crate::questions::list(ctx, None, false)?;
    let mut tasks = BTreeMap::new();
    let mut rows = Vec::new();
    for view in views {
        if !view.task.is_empty() && !tasks.contains_key(&view.task) {
            tasks.insert(view.task.clone(), ctx.store.head(&view.task)?);
        }
        let task = tasks.get(&view.task);
        let mut row = InboxRow {
            id: view.question.key.clone(),
            kind: kind(&view),
            task: task.map(|task| task.id.clone()),
            plan: task.and_then(plan_id),
            project: task.and_then(|task| task.project.clone()),
            title: short(&view.question.text, 120),
            from: author_role(&view).into(),
            asked_at: Some(view.question.asked.at.to_string()),
            needed_by: view.question.needed_by.map(|at| at.to_string()),
            blocking: view.question.blocking,
            state: state(&view),
            question_rev: Some(view.question.revision),
            answerable: view.pending,
            unanswerable_reason: None,
            rev: 0,
        };
        row.rev = revision(&row)?;
        rows.push(row);
    }
    for head in ctx
        .store
        .heads_checked(&corral_core::backend::HeadQuery::Open { project: None })?
    {
        let has_question = tasks.contains_key(&head.id);
        let planner_waits = head
            .last_plan_run
            .as_deref()
            .is_some_and(|run| run.state == corral_core::PlanRunState::AwaitingOwner);
        let candidate_waits = head.last_plan.as_deref().is_some_and(|plan| {
            plan.approval.is_none()
                && plan.current_review.as_ref().is_some_and(|review| {
                    review.verdict == Some(corral_core::Verdict::Approve) && !review.cancelled
                })
        });
        let run = head.current_run.as_deref();
        let escalated = head.status == corral_core::Status::NeedsMe
            && run.is_some_and(|run| {
                run.outcome == Some(corral_core::RunOutcome::Done)
                    && run
                        .autopilot
                        .as_ref()
                        .is_some_and(|stage| stage.stage == corral_core::AutoStage::Escalated)
            });
        let stuck = run.is_some_and(|run| run.outcome.is_none() && run.stuck.is_some());
        let selected = if planner_waits || candidate_waits {
            Some((InboxKind::PlanAcceptance, "plan_acceptance", "open_plan"))
        } else if escalated {
            Some((InboxKind::Escalated, "escalated", "open_task"))
        } else if stuck {
            Some((InboxKind::Stuck, "stuck", "open_task"))
        } else if head.status == corral_core::Status::Review {
            Some((InboxKind::Review, "review", "open_review"))
        } else if !has_question
            && (head.status == corral_core::Status::NeedsMe
                || head.attention.records.values().any(|record| {
                    record.resolved_at.is_none()
                        && record.responsible == corral_core::Responsible::Owner
                }))
        {
            Some((InboxKind::Other, "attention", "open_task"))
        } else {
            None
        };
        if let Some((kind, suffix, reason)) = selected {
            let mut row = InboxRow {
                id: format!("{}/{suffix}", head.id),
                kind,
                task: Some(head.id.clone()),
                plan: if kind == InboxKind::PlanAcceptance {
                    Some(head.id.clone())
                } else {
                    plan_id(&head)
                },
                project: head.project.clone(),
                title: short(&head.title, 120),
                from: String::new(),
                asked_at: head.status_since.map(|at| at.to_string()),
                needed_by: None,
                blocking: true,
                state: InboxState::Open,
                question_rev: None,
                answerable: false,
                unanswerable_reason: Some(reason.into()),
                rev: 0,
            };
            row.rev = revision(&row)?;
            rows.push(row);
        }
    }
    for session in ctx.store.active_assistant_ids()? {
        for prompt in crate::agent::ops::owner_prompt::list(&ctx.store, &session)? {
            if prompt.state != "open" || prompt.expires_at <= corral_core::clock::now() {
                continue;
            }
            let mut row = InboxRow {
                id: prompt.key,
                kind: InboxKind::Prompt,
                task: None,
                plan: None,
                project: None,
                title: short(prompt.command.as_deref().unwrap_or(&prompt.kind), 120),
                from: prompt.session,
                asked_at: Some(prompt.created.to_string()),
                needed_by: Some(prompt.expires_at.to_string()),
                blocking: true,
                state: InboxState::Open,
                question_rev: Some(prompt.revision),
                answerable: true,
                unanswerable_reason: None,
                rev: 0,
            };
            row.rev = revision(&row)?;
            rows.push(row);
        }
    }
    rows.sort_by(|a, b| b.asked_at.cmp(&a.asked_at).then_with(|| a.id.cmp(&b.id)));
    Ok(rows)
}

pub(super) fn question(ctx: &Ctx, key: &str) -> Result<DetailBody> {
    let view = crate::questions::show(ctx, key)?;
    let q = &view.question;
    let head = if view.task.is_empty() {
        None
    } else {
        Some(ctx.store.head(&view.task)?)
    };
    let context = head
        .as_ref()
        .map(|head| super::plans::plan_context(ctx, head))
        .transpose()?
        .flatten();
    let mut waiting = Vec::new();
    if let Some(head) = &head {
        for id in &head.depends_on {
            let dependency = ctx.store.head(id)?;
            if dependency.status != corral_core::Status::Done {
                waiting.push(QuestionWaiting {
                    id: dependency.id.clone(),
                    title: short(&dependency.title, 160),
                    kind: WaitKind::Info,
                });
            }
        }
    }
    let mut evidence = Vec::new();
    for reference in q.evidence.iter().take(5) {
        evidence.push(evidence_block(ctx, reference));
    }
    for reference in q.context_refs.iter().take(5 - evidence.len()) {
        evidence.push(legacy_evidence(ctx, reference));
    }
    let options = q
        .options
        .iter()
        .map(|label| {
            let detail = q
                .option_details
                .iter()
                .find(|option| option.label == *label);
            QuestionOption {
                label: excerpt(label, 512),
                next: detail
                    .and_then(|option| option.next.as_deref())
                    .map(|text| excerpt(text, 1024)),
                unblocks: detail
                    .map(|option| {
                        option
                            .unblocks
                            .iter()
                            .map(|text| excerpt(text, 256))
                            .collect()
                    })
                    .unwrap_or_default(),
                cost: detail
                    .and_then(|option| option.cost.as_deref())
                    .map(|text| excerpt(text, 256)),
                recommended: detail.map_or_else(
                    || q.recommendation.as_deref() == Some(label),
                    |option| option.recommended,
                ),
                evidence: detail
                    .and_then(|option| option.evidence)
                    .filter(|index| *index < evidence.len()),
            }
        })
        .collect();
    let mut history = q
        .triage
        .iter()
        .map(|entry| QuestionHistory {
            by: actor(entry.by).into(),
            via: None,
            at: entry.at.to_string(),
            text: excerpt(
                &entry.note.as_ref().map_or_else(
                    || entry.action.clone(),
                    |note| format!("{}: {note}", entry.action),
                ),
                1024,
            ),
            source_ref: Some(q.key.clone()),
        })
        .collect::<Vec<_>>();
    if let Some(head) = &head {
        for id in &q.owner_decisions {
            if let Some(decision) = head.decision_by_id(id)? {
                history.push(QuestionHistory {
                    by: decision.request.owner.clone(),
                    via: None,
                    at: decision.registered_at.to_string(),
                    text: excerpt(&decision.request.question, 1024),
                    source_ref: Some(format!("{}/decision/{}", head.id, decision.request.id)),
                });
            }
        }
    }
    history.sort_by(|a, b| a.at.cmp(&b.at));
    if history.len() > 5 {
        history.drain(..history.len() - 5);
    }
    let from = question_from(ctx, &view, head.as_ref())?;
    let answer = q.answer.as_ref().map(|answer| QuestionAnswer {
        by: answer_by(answer.by).into(),
        via: answer
            .attribution
            .as_ref()
            .map(|attribution| attribution.via.clone())
            .unwrap_or_default(),
        device: answer
            .attribution
            .as_ref()
            .and_then(|attribution| attribution.device.clone()),
        at: answer.at.to_string(),
        text: excerpt(&answer.text, 3 * 1024),
        option: answer.option.as_deref().map(|text| excerpt(text, 512)),
    });
    Ok(DetailBody::Question(QuestionDetail {
        key: q.key.clone(),
        rev: q.revision,
        state: state(&view),
        kind: kind(&view),
        blocking: q.blocking,
        task: head.as_ref().map(|head| head.id.clone()),
        plan: head.as_ref().and_then(plan_id),
        from,
        asked_at: q.asked.at.to_string(),
        needed_by: q.needed_by.map(|at| at.to_string()),
        text: excerpt(&q.text, 3 * 1024),
        options,
        recommendation: q.recommendation.as_deref().map(|text| excerpt(text, 512)),
        place: QuestionPlace {
            goal: context
                .as_ref()
                .and_then(|(_, goal)| goal.as_deref())
                .map(|text| excerpt(text, 512)),
            position: context.map(|(position, _)| position),
            waiting,
        },
        evidence,
        thread: q
            .followups
            .iter()
            .map(|entry| Followup {
                by: actor(entry.by).into(),
                via: entry.attribution.via.clone(),
                at: entry.at.to_string(),
                text: excerpt(&entry.text, 4096),
            })
            .collect(),
        history,
        answer,
        deadline: q
            .deadline
            .as_ref()
            .filter(|deadline| deadline.for_revision == q.revision)
            .map(|deadline| QuestionDeadline {
                at: deadline.at.to_string(),
                instruction: excerpt(&deadline.instruction, 512),
            }),
    }))
}

fn kind(view: &QuestionView) -> InboxKind {
    if view.container == "pm" {
        InboxKind::PmAsk
    } else if view.container.starts_with("assistant/") {
        InboxKind::AssistantAsk
    } else {
        InboxKind::WorkerQuestion
    }
}

fn author_role(view: &QuestionView) -> &'static str {
    match kind(view) {
        InboxKind::PmAsk => "pm",
        InboxKind::AssistantAsk => "assistant",
        _ if view.container.split('/').count() > 2 => "reviewer",
        _ if view.container.contains("/plan-") => "planner",
        _ => "worker",
    }
}

fn state(view: &QuestionView) -> InboxState {
    match view.effective_state {
        QuestionState::Open | QuestionState::Escalated => InboxState::Open,
        QuestionState::Expired => InboxState::Expired,
        QuestionState::Withdrawn | QuestionState::Cancelled => InboxState::Cancelled,
        QuestionState::Answered => {
            view.question
                .answer
                .as_ref()
                .map_or(InboxState::Other, |answer| InboxState::Answered {
                    by: answer_by(answer.by).into(),
                    via: answer
                        .attribution
                        .as_ref()
                        .map(|attribution| attribution.via.clone())
                        .unwrap_or_default(),
                    at: answer.at.to_string(),
                })
        }
    }
}

fn plan_id(head: &TaskHead) -> Option<String> {
    head.parent.clone().or_else(|| {
        head.plan
            .as_deref()
            .and_then(|plan| plan.split_once('@'))
            .map(|(id, _)| id.into())
    })
}

fn actor(value: QuestionActor) -> &'static str {
    match value {
        QuestionActor::Agent => "agent",
        QuestionActor::Pm => "pm",
        QuestionActor::Owner => "owner",
        QuestionActor::System => "system",
    }
}
fn answer_by(value: AnswerBy) -> &'static str {
    match value {
        AnswerBy::Pm => "pm",
        AnswerBy::Owner => "owner",
        AnswerBy::OwnerDeadline => "owner_deadline",
    }
}

fn question_from(ctx: &Ctx, view: &QuestionView, head: Option<&TaskHead>) -> Result<QuestionFrom> {
    let profile = if let Some(session) = view.container.strip_prefix("assistant/") {
        Some(ctx.store.get_assistant(session)?.profile)
    } else if let Some(head) = head {
        let parsed = crate::questions::QuestionKey::parse(&view.question.key)?;
        match parsed.container {
            ContainerRef::Worker { run, .. } => head
                .run_head(&run)?
                .map(|run| run.profile.unwrap_or(run.agent)),
            ContainerRef::Reviewer { run, review, .. } => head
                .review_child(&run, &review)?
                .map(|run| run.profile.unwrap_or(run.agent)),
            ContainerRef::Planner { run, .. } => head.plan_run_head(&run)?.map(|run| run.profile),
            ContainerRef::PlanReviewer { plan, review, .. } => head
                .plan_review(&plan, &review)?
                .map(|review| review.reviewer),
            _ => None,
        }
    } else {
        None
    };
    Ok(QuestionFrom {
        role: author_role(view).into(),
        session: (!view.question.asked.session_id.is_empty())
            .then(|| view.question.asked.session_id.clone()),
        profile,
    })
}

fn evidence_block(ctx: &Ctx, reference: &QuestionEvidenceRef) -> EvidenceBlock {
    let source_ref = match reference {
        QuestionEvidenceRef::Task { id } => id.clone(),
        QuestionEvidenceRef::Plan { task, id } => format!("{task}/{id}"),
        QuestionEvidenceRef::Run { task, run } => format!("{task}/{run}"),
        QuestionEvidenceRef::Artifact { id, .. } => id.clone(),
        QuestionEvidenceRef::Diff { task, run, path } => format!("{task}/{run}/diff/{path}"),
        QuestionEvidenceRef::Log { task, run } => format!("{task}/{run}/log"),
        QuestionEvidenceRef::Decision { task, id } => format!("{task}/decision/{id}"),
    };
    let body = resolve_evidence(ctx, reference).unwrap_or_else(|error| EvidenceBody::Unresolved {
        reason: excerpt(&error.to_string(), 256),
    });
    let mut block = EvidenceBlock {
        title: excerpt(&source_ref, 128),
        source_ref,
        body,
    };
    let mut encoded = serde_json::to_vec(&block).map_or(0, |bytes| bytes.len());
    while encoded > 8192 {
        match &mut block.body {
            EvidenceBody::Diff {
                lines, truncated, ..
            }
            | EvidenceBody::Excerpt {
                lines, truncated, ..
            }
            | EvidenceBody::LogTail { lines, truncated }
                if !lines.is_empty() =>
            {
                if let Some(line) = lines.pop() {
                    // Escapes and the separator only make the actual reduction larger.
                    encoded = encoded.saturating_sub(line.len() + 2);
                }
                *truncated = true;
            }
            _ => break,
        }
    }
    block
}

fn resolve_evidence(ctx: &Ctx, reference: &QuestionEvidenceRef) -> Result<EvidenceBody> {
    match reference {
        QuestionEvidenceRef::Task { id } => {
            let head = ctx.store.head(id)?;
            text_body(&head.body, None)
        }
        QuestionEvidenceRef::Plan { task, id } => {
            let head = ctx.store.head(task)?;
            let plan = head.plan_head(id)?.context("canonical plan not found")?;
            text_body(&serde_json::to_string_pretty(&plan.document)?, None)
        }
        QuestionEvidenceRef::Run { task, run } => {
            let head = ctx.store.head(task)?;
            let run = head.run_head(run)?.context("canonical run not found")?;
            text_body(
                run.summary
                    .as_deref()
                    .or(run.blocked_excerpt.as_deref())
                    .context("run has no recorded text")?,
                None,
            )
        }
        QuestionEvidenceRef::Artifact { task, id } => {
            let artifact = crate::resolve_artifact(ctx, id)?;
            anyhow::ensure!(
                crate::artifact_storage::task_of(&artifact.path) == Some(task.as_str()),
                "artifact belongs to a different container"
            );
            let bytes =
                crate::artifact_storage::artifact_read_confined_limited(ctx, &artifact, 8192)?;
            let text = std::str::from_utf8(&bytes).context("artifact is not UTF-8 text")?;
            let mut body = text_body(text, None)?;
            if let EvidenceBody::Excerpt { truncated, .. } = &mut body {
                *truncated |= artifact.bytes > bytes.len() as u64;
            }
            Ok(body)
        }
        QuestionEvidenceRef::Log { task, run } => {
            let head = ctx.store.head(task)?;
            let artifacts = head.artifacts_of(run)?;
            if let Some(artifact) = artifacts
                .iter()
                .rev()
                .find(|artifact| artifact.kind == corral_core::ArtifactKind::Log)
            {
                let bytes =
                    crate::artifact_storage::artifact_read_confined_limited(ctx, artifact, 8192)?;
                let text = std::str::from_utf8(&bytes).context("log artifact is not UTF-8 text")?;
                let mut body = text_body(text, None)?;
                if let EvidenceBody::Excerpt { truncated, .. } = &mut body {
                    *truncated |= artifact.bytes > bytes.len() as u64;
                }
                return Ok(body);
            }
            let run = head.run_head(run)?.context("canonical run not found")?;
            let text = run
                .blocked_excerpt
                .as_deref()
                .context("no recorded log excerpt")?;
            let mut tail = text.lines().rev().take(40).collect::<Vec<_>>();
            tail.reverse();
            let text_tail = tail.join("\n");
            let (lines, truncated) = lines(&text_tail)?;
            Ok(EvidenceBody::LogTail {
                lines,
                truncated: truncated || text.lines().count() > tail.len(),
            })
        }
        QuestionEvidenceRef::Diff { task, run, path } => {
            let head = ctx.store.head(task)?;
            let artifacts = head.artifacts_of(run)?;
            let artifact = artifacts
                .iter()
                .rev()
                .find(|artifact| artifact.kind == corral_core::ArtifactKind::Diff)
                .context("no recorded diff artifact")?;
            let bytes =
                crate::artifact_storage::artifact_read_confined_limited(ctx, artifact, 256 * 1024)?;
            let text = std::str::from_utf8(&bytes).context("diff artifact is not UTF-8 text")?;
            let marker = format!("diff --git a/{path} b/{path}");
            let mut section = String::new();
            let mut found = false;
            for line in text.lines() {
                if line.starts_with("diff --git ") {
                    if found {
                        break;
                    }
                    found = line == marker;
                }
                if found {
                    section.push_str(line);
                    section.push('\n');
                }
            }
            if !found {
                bail!("path is not in the bounded recorded diff");
            }
            let (lines, truncated) = lines(&section)?;
            Ok(EvidenceBody::Diff {
                path: path.clone(),
                lines,
                truncated: truncated || artifact.bytes > bytes.len() as u64,
            })
        }
        QuestionEvidenceRef::Decision { task, id } => {
            let head = ctx.store.head(task)?;
            let decision = head
                .decision_by_id(id)?
                .context("canonical decision not found")?;
            text_body(&decision.request.question, None)
        }
    }
}

fn legacy_evidence(ctx: &Ctx, reference: &str) -> EvidenceBlock {
    if reference.starts_with("T-") && !reference.contains(['/', ':', '@']) {
        return evidence_block(
            ctx,
            &QuestionEvidenceRef::Task {
                id: reference.into(),
            },
        );
    }
    if let Ok(corral_core::PinRef::Artifact { task, .. }) = corral_core::PinRef::parse(reference) {
        return evidence_block(
            ctx,
            &QuestionEvidenceRef::Artifact {
                task,
                id: reference.into(),
            },
        );
    }
    EvidenceBlock {
        source_ref: reference.into(),
        title: excerpt(reference, 128),
        body: EvidenceBody::Unresolved {
            reason: "Reference has no canonical typed evidence resolver".into(),
        },
    }
}

fn text_body(text: &str, path: Option<String>) -> Result<EvidenceBody> {
    let (lines, truncated) = lines(text)?;
    Ok(EvidenceBody::Excerpt {
        path,
        start_line: None,
        lines,
        truncated,
    })
}

// Budget the serialized lines, not just UTF-8 bytes: control characters can expand sixfold.
fn lines(text: &str) -> Result<(Vec<String>, bool)> {
    let mut output = Vec::new();
    let mut remaining = 6 * 1024;
    let mut truncated = false;
    for line in text.lines() {
        let encoded = serde_json::to_string(line)?.len() + 1;
        if encoded <= remaining {
            output.push(line.to_owned());
            remaining -= encoded;
        } else {
            let clipped = short(line, remaining.saturating_sub(3) / 6);
            if !clipped.is_empty() {
                output.push(clipped);
            }
            truncated = true;
            break;
        }
    }
    Ok((output, truncated))
}
