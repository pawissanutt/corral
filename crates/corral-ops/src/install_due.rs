//! Self-hosted product landings without a configured post-land installer.
use crate::{Ctx, hooks::PostLandOperation};
use anyhow::Result;
use serde_json::json;

pub(crate) fn check(ctx: &Ctx, op: &PostLandOperation) -> Result<()> {
    check_with(ctx, op, || {
        // Inspect the installed product, not the supervisor's possibly older in-memory build.
        let output = std::process::Command::new("corral")
            .arg("build-identity")
            .stdin(std::process::Stdio::null())
            .output();
        output
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|identity| !identity.is_empty())
            .unwrap_or_else(|| "unknown".into())
    })
}

fn check_with(ctx: &Ctx, op: &PostLandOperation, installed: impl FnOnce() -> String) -> Result<()> {
    if !op.landed
        || op.base != "main"
        || !op.hooks.is_empty()
        || !op.repo.join("crates/corral/Cargo.toml").is_file()
    {
        return Ok(());
    }
    emit(ctx, op, &installed())
}

fn emit(ctx: &Ctx, op: &PostLandOperation, installed: &str) -> Result<()> {
    if installed == op.landed_sha {
        return Ok(());
    }
    // A later installed build also satisfies this landing. Unknown identities (older builds
    // without build-identity support) conservatively require an install.
    if installed != "unknown"
        && corral_git::rev_sha(&op.repo, &format!("{installed}^{{commit}}")).is_ok()
        && corral_git::is_ancestor(&op.repo, &op.landed_sha, installed)?
    {
        return Ok(());
    }
    // The existing repository landing lock serializes replay. The marker and event
    // commit together, so a crash cannot publish either half or require a history scan.
    ctx.store.records_transaction(|| {
        if !ctx.store.insert_record("install-notices", &op.key, &true)? {
            return Ok(());
        }
        ctx.store.log_event(
            Some(&op.task),
            "install_due",
            json!({
                "project": op.project, "run": op.run, "operation": op.key,
                "landed_sha": op.landed_sha, "installed_sha": installed,
                "reason": "corral main advanced past the installed build; reinstall corral"
            }),
        )
    })
}

#[cfg(test)]
#[path = "install_due_tests.rs"]
mod tests;
