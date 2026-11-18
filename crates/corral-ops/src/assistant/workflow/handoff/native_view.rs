//! The exact handoff state a native sheet shows. Its digest is bound into every native
//! selection, so a workflow, plan, review, board transfer or source change between display
//! and consumption refuses instead of rebinding consent to work the owner never saw.
use anyhow::{Context as _, Result};
use corral_core::TaskRef as _;
use corral_core::assistant::AssistantContainer;
use corral_core::assistant_workflow::ScopeOwner;
use corral_core::blob_ref::BlobHash;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

use crate::Ctx;

pub const STALE: &str =
    "handoff_stale_view: what you selected changed since the sheet showed it; reopen /handoff";

pub struct View {
    pub digest: String,
    /// Per-scratch source fingerprints, rechecked after a package snapshot is captured.
    pub sources: BTreeMap<String, String>,
}

/// The same bounded scan a handoff snapshot uses: HEAD, base, branch tip and detachment, the
/// exact index bytes and every tracked/untracked file's content and mode. Files are read
/// no-follow and regular-only under per-file/total caps; no diff helper or filter runs.
fn source(path: &Path, base: &str, branch: &str) -> Result<String> {
    let scan = super::git::scan(path, base, branch)?;
    Ok(BlobHash::of(&serde_json::to_vec(&scan.state)?)
        .as_str()
        .to_string())
}

/// An unsupported or unreadable scratch refuses the read rather than becoming a fingerprint.
pub(super) fn sources(ctx: &Ctx, session: &str) -> Result<BTreeMap<String, String>> {
    let record = ctx.store.get_assistant(session)?;
    record
        .scratch
        .iter()
        .map(|s| {
            let print = source(&s.path, &s.base, &s.branch).with_context(|| {
                format!(
                    "handoff_source_unsupported: scratch {}/{} cannot be inspected safely",
                    s.project, s.slug
                )
            })?;
            Ok((format!("{}/{}", s.project, s.slug), print))
        })
        .collect()
}

pub fn view(ctx: &Ctx, session: &str, container: &AssistantContainer) -> Result<View> {
    let current = container.workflow.as_ref().map(|i| &i.current);
    // Local scratch sources are what a local choice packages; a board-owned scope is pinned
    // by its board continuation identity (plan and code) below instead.
    let sources = match current.map(|w| &w.owner) {
        Some(ScopeOwner::Board { .. }) => BTreeMap::new(),
        _ => sources(ctx, session)?,
    };
    let workflow = current.map(|w| {
        json!({"id": w.id, "state_revision": w.state_revision, "epoch": w.authority_epoch,
            "phase": w.phase, "owner": w.owner,
            "plan": w.current_plan.as_ref().map(|p| (p.revision, p.artifact.blob.hash().clone()))})
    });
    let reviews = match super::super::review::read(&ctx.store, session, container, None, 0, 0) {
        Ok(read) => json!(read["reviews"].as_array().map(|all| {
            all.iter()
                .map(|r| json!([r["review"], r["revision"], r["lifecycle"], r["freshness"]]))
                .collect::<Vec<_>>()
        })),
        Err(e) => json!({"unavailable": format!("{e:#}")}),
    };
    let board = match current.map(|w| &w.owner) {
        Some(ScopeOwner::Board { task }) => {
            let t = ctx.store.get(task)?;
            let transfer = match super::lifecycle::for_task(&ctx.store, &t) {
                Ok(j) => json!(j.as_ref().and_then(|j| j.transfer.as_ref()).map(|x| json!([
                    x.revision,
                    x.disposition,
                    x.goal,
                    x.contract_revision
                ]))),
                Err(e) => json!({"unavailable": format!("{e:#}")}),
            };
            // The continuation identity a Start would pin: plan/producer/findings and code.
            let plan = match t.last_plan_head()? {
                Some(_) => match super::lifecycle::plan_identity(&t) {
                    Ok(p) => json!(p),
                    Err(e) => json!({"unavailable": format!("{e:#}")}),
                },
                None => Value::Null,
            };
            let code = match super::code_pin::current(ctx, &t) {
                Ok(c) => json!(c),
                Err(e) => json!({"unavailable": format!("{e:#}")}),
            };
            json!({"task": task, "status": t.status, "hold": t.dispatch_hold,
                "contract": t.contract_revision()?, "transfer": transfer,
                "plan": plan, "code": code})
        }
        _ => Value::Null,
    };
    let digest = BlobHash::of(&serde_json::to_vec(&json!({"workflow": workflow,
        "reviews": reviews, "board": board, "sources": sources}))?)
    .as_str()
    .to_string();
    Ok(View { digest, sources })
}
