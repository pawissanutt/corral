//! Run configured shell gates without tasks, capacity reservations, panes or store locks.
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::Ctx;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateReceipt {
    pub attempt: String,
    pub tier: String,
    pub head: String,
    pub result: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_command: Option<String>,
    pub commands: Vec<String>,
    pub started: String,
    pub ended: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
    pub receipt: String,
}

pub struct GateRequest<'a> {
    pub session: &'a str,
    pub worktree: &'a Path,
    pub project: &'a str,
    pub tier: &'a str,
    pub base: Option<&'a str>,
}

/// A completed failing gate is a receipt, not a command refusal.
pub fn run(ctx: &Ctx, request: &GateRequest<'_>) -> Result<GateReceipt> {
    let authority = crate::assistant::owner_action::build_shell_authority(ctx)?;
    run_authorized(ctx, request, &authority)
}

pub(crate) fn run_authorized(
    ctx: &Ctx,
    request: &GateRequest<'_>,
    _authority: &crate::assistant::owner_action::BuildAuthority,
) -> Result<GateReceipt> {
    let session = super::session_dir(request.session)?;
    let (repo, branch) = super::project(ctx, request.project, request.worktree)?;
    ensure!(
        matches!(request.tier, "quick" | "full"),
        "gate_tier_undefined: unknown tier {:?}",
        request.tier
    );
    let policy = ctx.store.config.review_policy(Some(request.project), None);
    let commands = crate::gate_tier::tier_commands(&policy, request.tier).with_context(|| {
        format!(
            "gate_tier_undefined: project {:?} has no {} tier",
            request.project, request.tier
        )
    })?;
    require_clean(request.worktree)?;
    let head =
        corral_git::rev_sha(request.worktree, "HEAD").context("reading gate candidate HEAD")?;
    let base = match request.base {
        Some(base) => corral_git::rev_sha(request.worktree, base).context("resolving gate base")?,
        None => corral_git::rev_sha(&repo, &branch).context("reading gate integration base")?,
    };
    let fork = corral_git::merge_base(request.worktree, &base, &head)
        .context("reading gate merge base")?;
    let changed = corral_git::changed_paths_between(request.worktree, &fork, &head)
        .context("reading gate changed paths")?;
    if let Some(parent) = session.parent() {
        super::private_dir(parent)?;
    }
    super::private_dir(&session)?;
    let gates = session.join("gates");
    super::private_dir(&gates)?;
    let (attempt, receipt_path, mut receipt_file) = claim(&gates)?;
    let log_path = gates.join(format!("{attempt}.log"));
    let mut log = super::create_private(&log_path).context("creating build gate log")?;
    let context_path = gates.join(format!("{attempt}.context.json"));
    // The selector's schema is independent of task storage. Empty task/run fields explicitly
    // denote a session gate; no task identity, row or fake run is created.
    let context = serde_json::json!({
        "schema_version": 1, "task": "", "run": "", "attempt": attempt,
        "tier": request.tier, "commands": commands, "scope": [], "tests": [],
        "expected_failures": [], "destination": {"kind": "main", "branch": branch},
        "integration_base": base, "plan_verify": false, "changed_paths": changed,
    });
    let mut context_file =
        super::create_private(&context_path).context("creating build gate context")?;
    serde_json::to_writer_pretty(&mut context_file, &context)
        .context("writing build gate context")?;
    let mut receipt = GateReceipt {
        attempt,
        tier: request.tier.into(),
        head: head.clone(),
        result: if commands.is_empty() {
            "skipped"
        } else {
            "pass"
        }
        .into(),
        failed_command: None,
        commands,
        started: corral_core::clock::now().to_string(),
        ended: String::new(),
        log: Some(log_path.to_string_lossy().into_owned()),
        receipt: receipt_path.to_string_lossy().into_owned(),
    };
    if receipt.commands.is_empty() {
        receipt.log = None;
    }
    for command in &receipt.commands {
        writeln!(log, "\n$ {command}").context("writing build gate command header")?;
        // Match task gates: bash pipefail, null stdin, configured environment, and role
        // identity scrubbed inside the command shell, not in the calling process.
        let script = format!(
            "unset -v ${{!CORRAL_ASSISTANT@}} ${{!CORRAL_SANDBOX@}} ${{!CORRAL_AGENT@}} CORRAL_PM CORRAL_REVIEWER CORRAL_OWNER_ACTION AI_AGENT PI_CODING_AGENT\n{command}"
        );
        let status = Command::new("bash")
            .args(["-o", "pipefail", "-c", &script])
            .current_dir(request.worktree)
            .envs(&policy.env)
            .env("CORRAL_INTEGRATION_BASE", &base)
            .env("CORRAL_CANDIDATE_HEAD", &head)
            .env("CORRAL_GATE_ATTEMPT", &receipt.attempt)
            .env("CORRAL_GATE_TIER", request.tier)
            .env("CORRAL_GATE_CONTEXT", &context_path)
            .env_remove("CORRAL_GATE_PROCESS_OWNER")
            .env_remove("CORRAL_GATE_OWNER_TOKEN")
            .stdin(Stdio::null())
            .stdout(log.try_clone().context("opening build gate stdout")?)
            .stderr(log.try_clone().context("opening build gate stderr")?)
            .status()
            .context("running build gate command")?;
        if !status.success() {
            receipt.result = "fail".into();
            receipt.failed_command = Some(command.clone());
            break;
        }
    }
    if corral_git::rev_sha(request.worktree, "HEAD").context("checking gated HEAD")? != head
        || !corral_git::is_clean(request.worktree).context("checking gated worktree")?
    {
        receipt.result = "fail".into();
        receipt.failed_command = Some("gate modified the candidate".into());
    }
    receipt.ended = corral_core::clock::now().to_string();
    log.flush().context("flushing build gate log")?;
    serde_json::to_writer_pretty(&mut receipt_file, &receipt)
        .context("writing build gate receipt")?;
    receipt_file
        .sync_all()
        .context("syncing build gate receipt")?;
    Ok(receipt)
}

pub(super) fn require_clean(worktree: &Path) -> Result<()> {
    ensure!(
        corral_git::is_clean(worktree).context("checking build worktree")?,
        "worktree_dirty: tracked or untracked files changed"
    );
    Ok(())
}

pub(super) fn valid_attempt(attempt: &str) -> bool {
    attempt.strip_prefix('G').is_some_and(|number| {
        !number.starts_with('0')
            && number.bytes().all(|byte| byte.is_ascii_digit())
            && number.parse::<u64>().is_ok_and(|n| n > 0)
    })
}

fn claim(gates: &Path) -> Result<(String, PathBuf, File)> {
    let mut next = 1_u64;
    for entry in std::fs::read_dir(gates).context("reading build gate attempts")? {
        let path = entry.context("reading build gate attempt")?.path();
        if path.extension().is_some_and(|ext| ext == "json")
            && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
            && valid_attempt(stem)
        {
            let number: u64 = stem[1..].parse().context("parsing build gate attempt")?;
            next = next.max(
                number
                    .checked_add(1)
                    .context("build gate attempts exhausted")?,
            );
        }
    }
    loop {
        let attempt = format!("G{next}");
        let path = gates.join(format!("{attempt}.json"));
        match super::create_private(&path) {
            Ok(file) => return Ok((attempt, path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                next = next
                    .checked_add(1)
                    .context("build gate attempts exhausted")?;
            }
            Err(error) => return Err(error).context("claiming build gate attempt"),
        }
    }
}

#[cfg(all(test, unix))]
#[path = "gate_tests.rs"]
pub(super) mod tests;
