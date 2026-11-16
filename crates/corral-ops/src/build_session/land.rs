//! Owner-only, receipt-bound fast-forward landing without a task or run record.
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::gate::GateReceipt;
use crate::Ctx;

#[derive(Debug, Clone, Serialize)]
pub struct LandResult {
    pub landed: String,
    pub base_branch: String,
    pub previous: String,
}

pub(crate) struct LandRequest<'a> {
    pub session: &'a str,
    pub worktree: &'a Path,
    pub project: &'a str,
    pub expected_head: &'a str,
    pub gate_attempt: &'a str,
}

/// Land exactly `expected_head` after a current full-tier receipt and board landing checks.
pub fn run(
    ctx: &Ctx,
    session: &str,
    worktree: &Path,
    project: &str,
    expected_head: &str,
    gate_attempt: &str,
) -> Result<LandResult> {
    let authority = crate::assistant::owner_action::build_shell_authority(ctx)?;
    run_authorized(
        ctx,
        &LandRequest {
            session,
            worktree,
            project,
            expected_head,
            gate_attempt,
        },
        &authority,
    )
}

pub(crate) fn run_authorized(
    ctx: &Ctx,
    request: &LandRequest<'_>,
    _authority: &crate::assistant::owner_action::BuildAuthority,
) -> Result<LandResult> {
    let LandRequest {
        session,
        worktree,
        project,
        expected_head,
        gate_attempt,
    } = *request;
    let session_path = super::session_dir(session)?;
    let (repo, base) = super::project(ctx, project, worktree)?;
    let head = corral_git::rev_sha(worktree, "HEAD").context("reading landing candidate HEAD")?;
    ensure!(
        head == expected_head,
        "head_mismatch: HEAD is {head}, expected {expected_head}"
    );
    super::gate::require_clean(worktree)?;
    ensure!(
        super::gate::valid_attempt(gate_attempt),
        "gate_missing: invalid gate attempt {gate_attempt:?}"
    );
    let receipt_path = session_path
        .join("gates")
        .join(format!("{gate_attempt}.json"));
    let bytes = std::fs::read(&receipt_path)
        .map_err(|error| anyhow::anyhow!("gate_missing: {}: {error}", receipt_path.display()))?;
    let receipt: GateReceipt = serde_json::from_slice(&bytes)
        .map_err(|error| anyhow::anyhow!("gate_missing: unreadable receipt: {error}"))?;
    let commands = ctx.store.config.review_policy(Some(project), None).gate;
    ensure!(
        receipt.attempt == gate_attempt
            && receipt.tier == "full"
            && receipt.head == expected_head
            && receipt.commands == commands,
        "gate_stale: receipt does not match the current full gate and candidate"
    );
    ensure!(receipt.result != "fail", "gate_failed: full gate failed");
    ensure!(
        receipt.result == "pass" || (receipt.result == "skipped" && commands.is_empty()),
        "gate_failed: receipt is not a passing full gate"
    );
    require_checkout(&repo, &base)?;
    let _lock = corral_git::landing_lock(&repo).context("locking build landing")?;
    require_checkout(&repo, &base)?;
    let previous = corral_git::rev_sha(&repo, &base).context("reading current landing base")?;
    ensure!(
        corral_git::is_ancestor(&repo, &previous, expected_head)
            .context("checking landing ancestry")?,
        "landing_stale: candidate does not contain the current {base} head {previous}"
    );
    let branch =
        corral_git::current_branch(worktree).context("reading landing candidate branch")?;
    let landed =
        corral_git::land_checked(&repo, worktree, &branch, &base, expected_head, &previous)
            .map_err(|error| {
                let detail = error.to_string();
                if detail.starts_with("stale_retry:") {
                    anyhow::anyhow!("landing_stale: {detail}")
                } else if detail.starts_with("main checkout ") {
                    checkout_error(error)
                } else {
                    error
                }
            })?;
    ctx.store
        .log_event(
            None,
            "build_landed",
            serde_json::json!({
                "session": session, "project": project, "head": expected_head,
                "landed": landed, "previous": previous,
            }),
        )
        .context("recording build landing event")?;
    Ok(LandResult {
        landed,
        base_branch: base,
        previous,
    })
}

fn require_checkout(repo: &Path, base: &str) -> Result<()> {
    corral_git::require_landing_checkout(repo, base).map_err(checkout_error)
}

fn checkout_error(error: anyhow::Error) -> anyhow::Error {
    let detail = error.to_string();
    if (detail.starts_with("main checkout ") && detail.contains(" is not on "))
        || detail.ends_with(" has a detached HEAD")
    {
        anyhow::anyhow!("main_off_base: {detail}")
    } else if detail.starts_with("main checkout ") && detail.ends_with(" has uncommitted changes") {
        anyhow::anyhow!("main_dirty: {detail}")
    } else {
        error
    }
}

#[cfg(all(test, unix))]
#[path = "land_tests.rs"]
mod tests;
