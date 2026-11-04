//! Bounded, project-scoped reads of durable task content, never model-selected shell paths.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Ctx;
use crate::agent::admission::{self, Admission, Mode};
use crate::agent::call::{CallResponse, RejectCode, Rejection};
use crate::agent::envelope::Envelope;
use crate::agent::receipts::{self, Commit};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    reference: String,
    #[serde(default)]
    offset: usize,
    #[serde(default = "page_size")]
    limit: usize,
}

fn page_size() -> usize {
    4096
}

fn scope(ctx: &Ctx, id: &str, project: Option<&str>) -> Result<corral_core::Task> {
    let task = ctx.store.get(id)?;
    if task.project.as_deref() != project {
        return Err(
            Rejection::new(RejectCode::OutOfScope, "content belongs to another project").into(),
        );
    }
    Ok(task)
}

fn content(ctx: &Ctx, reference: &str, project: Option<&str>) -> Result<Vec<u8>> {
    if let Some((task, plan)) = reference.split_once('@') {
        let task = scope(ctx, task, project)?;
        let plan = task
            .plans
            .iter()
            .find(|p| p.id == plan)
            .context("plan reference not found")?;
        let bytes = corral_core::plan_policy::read_plan_snapshot(&ctx.store, &plan.artifact)?;
        ensure!(
            crate::verification::digest(&bytes) == plan.artifact.sha256,
            "plan hash mismatch"
        );
        return Ok(bytes);
    }
    if !reference.contains('/') && !reference.contains(':') {
        let task = scope(ctx, reference, project)?;
        // Never embed agent host state/receipts: each read records its own receipt, so
        // doing so would grow the paged document recursively while it is being read.
        let plans: Vec<_> = task
            .plans
            .iter()
            .map(|plan| {
                json!({
                    "id": plan.id, "reference": format!("{}@{}", task.id, plan.id),
                    "sha256": plan.artifact.sha256, "spec_revision": plan.spec_revision,
                    "reviews": plan.reviews.iter().map(|review| json!({
                        "id": review.id, "reviewer": review.reviewer,
                        "verdict": review.verdict, "notes": review.notes,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        return Ok(serde_json::to_vec(&json!({
            "task": task.id, "title": task.title, "spec": task.body,
            "criteria": task.completion, "plans": plans, "artifacts": task.artifacts,
        }))?);
    }
    let artifact = crate::resolve_artifact(ctx, reference)?;
    let owner =
        crate::artifact_storage::task_of(&artifact.path).context("artifact has no task owner")?;
    scope(ctx, owner, project)?;
    ctx.store.with_blob_lease(|| {
        let max = ctx.store.config.artifacts.max_bytes;
        let bytes = crate::artifact_storage::artifact_read_confined_limited(ctx, &artifact, max)?;
        ensure!(
            u64::try_from(bytes.len())? <= max,
            "artifact exceeds artifacts.max_bytes"
        );
        if artifact.purged_at.is_none() {
            ensure!(
                crate::verification::digest(&bytes) == artifact.sha256,
                "artifact hash mismatch"
            );
        }
        Ok(bytes)
    })
}

pub(super) fn call(ctx: &Ctx, envelope: &Envelope) -> Result<CallResponse> {
    let input: Input = crate::agent::ops::payload(envelope, "reference, offset?, limit?")?;
    ensure!(
        (1..=4096).contains(&input.limit),
        "limit must be 1..4096 bytes"
    );
    ensure!(input.reference.len() <= 512, "reference exceeds 512 bytes");
    let container = admission::container_of(&envelope.identity);
    let admitted = match admission::admit(ctx, envelope, Mode::Mutation { settles_turn: None })? {
        Admission::Replay(stored) => return receipts::respond(ctx, &container, *stored, true),
        Admission::New(admitted) => admitted,
    };
    receipts::commit_with_task(ctx, &admitted, |bound| {
        let bytes = content(ctx, &input.reference, bound.project.as_deref())?;
        let text = std::str::from_utf8(&bytes)
            .context("content is not UTF-8 text; use an artifact viewer for binary content")?;
        ensure!(
            input.offset <= text.len() && text.is_char_boundary(input.offset),
            "offset must be a UTF-8 byte boundary within the content"
        );
        let mut end = input.offset.saturating_add(input.limit).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        ensure!(
            end > input.offset || end == text.len(),
            "limit is smaller than the next UTF-8 character"
        );
        // JSON escaping (including a task manifest encoded inside text) can grow a page.
        // Stay within the receipt's serialized result budget, not only the raw text budget.
        let result: Value = loop {
            let page = json!({
                "reference": input.reference, "offset": input.offset,
                "text": &text[input.offset..end], "bytes": bytes.len(),
                "next_offset": (end < bytes.len()).then_some(end),
            });
            if serde_json::to_vec(&page)?.len() <= corral_core::agent::RESULT_MAX_BYTES {
                break page;
            }
            end = input.offset + (end - input.offset) / 2;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            ensure!(
                end > input.offset,
                "content metadata exceeds receipt budget"
            );
        };
        Ok(Commit {
            result,
            effect: None,
            post_pending: Vec::new(),
        })
    })
}
