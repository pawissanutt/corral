//! Canonical dependencies and earlier owner decisions, ordered by relationship before recency.
use anyhow::Result;
use corral_core::agent::AgentQuestion;
use corral_core::backend::{HeadQuery, PageReq, ReadView, RowQuery, TaskNum, TaskPart};
use corral_core::{DecisionRecord, ReceiptOutcome, TaskHead};
use corral_protocol::detail::{QuestionHistory, QuestionWaiting, WaitKind};
use std::collections::BTreeSet;

use super::{short, tasks::excerpt};
use crate::Ctx;

pub(super) fn waiting(ctx: &Ctx, head: &TaskHead) -> Result<Vec<QuestionWaiting>> {
    Ok(ctx
        .store
        .heads_checked(&HeadQuery::EdgeTo {
            kind: "depends_on".into(),
            other: head.id.clone(),
        })?
        .into_iter()
        .filter(|task| {
            !matches!(
                task.status,
                corral_core::Status::Done | corral_core::Status::Dropped
            )
        })
        .map(|task| QuestionWaiting {
            id: task.id.clone(),
            title: short(&task.title, 160),
            kind: WaitKind::You,
        })
        .collect())
}

pub(super) fn related(
    ctx: &Ctx,
    head: &TaskHead,
    question: &AgentQuestion,
) -> Result<Vec<QuestionHistory>> {
    let mut found = Vec::new();
    let mut decisions = BTreeSet::new();
    for id in &question.owner_decisions {
        if let Some(decision) = head.decision_by_id(id)? {
            add(&head.id, decision, question, &mut decisions, &mut found);
        }
        if found.len() == 5 {
            return Ok(found);
        }
    }
    let view = ctx.store.view()?;
    let mut tasks = BTreeSet::from([head.num]);
    task_decisions(
        &*view,
        head.num,
        &head.id,
        question,
        &mut decisions,
        &mut found,
    )?;
    if found.len() < 5
        && let Some(plan) = super::questions::plan_id(head)
        && let Some(parent) = ctx.store.head_if_exists(&plan)?
        && tasks.insert(parent.num)
    {
        task_decisions(
            &*view,
            parent.num,
            &parent.id,
            question,
            &mut decisions,
            &mut found,
        )?;
    }
    if let Some(project) = &head.project {
        let mut page = PageReq::first(32);
        while found.len() < 5 {
            let result = view.heads(
                &HeadQuery::ProjectUpdated {
                    project: project.clone(),
                },
                page,
            )?;
            for candidate in result.items {
                if tasks.insert(candidate.header.num) {
                    task_decisions(
                        &*view,
                        candidate.header.num,
                        &candidate.header.id,
                        question,
                        &mut decisions,
                        &mut found,
                    )?;
                }
                if found.len() == 5 {
                    break;
                }
            }
            let Some(after) = result.next else { break };
            page = PageReq {
                after: Some(after),
                limit: 32,
            };
        }
    }
    Ok(found)
}

fn task_decisions(
    view: &dyn ReadView,
    num: TaskNum,
    task: &str,
    question: &AgentQuestion,
    seen: &mut BTreeSet<String>,
    found: &mut Vec<QuestionHistory>,
) -> Result<()> {
    let mut page = PageReq::first(64);
    while found.len() < 5 {
        let rows = view.rows(
            num,
            &RowQuery::part(TaskPart::RunObligation).newest_first(),
            page,
        )?;
        for row in rows.items {
            if row.key.contains("/decision/") {
                let decision = serde_json::from_value(row.doc)?;
                add(task, decision, question, seen, found);
            }
            if found.len() == 5 {
                break;
            }
        }
        let Some(after) = rows.next else { break };
        page = PageReq {
            after: Some(after),
            limit: 64,
        };
    }
    Ok(())
}

fn add(
    task: &str,
    decision: DecisionRecord,
    question: &AgentQuestion,
    seen: &mut BTreeSet<String>,
    found: &mut Vec<QuestionHistory>,
) {
    let Some(receipt) = decision.receipt.as_ref().filter(|receipt| {
        receipt.outcome == ReceiptOutcome::Answered && receipt.observed_at <= question.asked.at
    }) else {
        return;
    };
    let source = format!("{task}/decision/{}", decision.request.id);
    if !seen.insert(source.clone()) {
        return;
    }
    found.push(QuestionHistory {
        by: decision.request.owner,
        via: None,
        at: receipt.observed_at.to_string(),
        text: excerpt(
            &receipt.answer.as_ref().map_or_else(
                || decision.request.question.clone(),
                |answer| format!("{}: {answer}", decision.request.question),
            ),
            1024,
        ),
        source_ref: Some(source),
    });
}

pub(super) fn image(
    ctx: &Ctx,
    key: &str,
    artifact: &corral_core::Artifact,
) -> Result<corral_protocol::detail::EvidenceBody> {
    anyhow::ensure!(
        artifact.purged_at.is_none() && artifact.bytes <= 8 * 1024 * 1024,
        "image is unavailable or exceeds the 8 MiB evidence limit"
    );
    let bytes = crate::artifact_storage::artifact_read_confined_limited(ctx, artifact, 32)?;
    let media_type = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()) {
        "image/webp"
    } else if bytes.starts_with(b"BM") {
        "image/bmp"
    } else {
        anyhow::bail!("artifact is not a supported raster image");
    };
    Ok(corral_protocol::detail::EvidenceBody::Image {
        artifact: corral_protocol::detail::ArtifactRef {
            source_ref: key.to_owned(),
            title: excerpt(&artifact.title, 128),
            media_type: Some(media_type.into()),
        },
    })
}
