//! The existing gate runner and receipt checks, executed on the combined candidate.
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use corral_core::{AggregateAttempt, GateRecord, GateResult};
use jiff::Timestamp;

use crate::Ctx;
use crate::gate::{GatePaths, ResultFile, command_log_valid, prepare_owner};
use crate::verification::digest;

pub(super) fn paths(ctx: &Ctx, plan: &str, attempt: &str) -> GatePaths {
    let dir = ctx.store.state_dir().join("plan-gates");
    let file = |ext: &str| dir.join(format!("{plan}-{attempt}.{ext}"));
    GatePaths {
        script: file("sh"),
        result: file("json"),
        failed: file("failed"),
        log: file("log"),
    }
}

/// Run every configured command in the verification checkout and validate its receipts.
/// The result file and command log, not the runner's exit status, are the authority.
pub(super) fn run(
    ctx: &Ctx,
    attempt: &AggregateAttempt,
    checkout: &Path,
    commands: &[String],
) -> Result<GateRecord> {
    let identity = &attempt.identity;
    let paths = paths(ctx, &identity.task, &attempt.id);
    let dir = paths.log.parent().context("plan gate has no directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    for path in [&paths.result, &paths.failed, &paths.log] {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("removing {}", path.display()));
            }
        }
    }
    let owner = paths.result.with_extension("owner");
    prepare_owner(&owner)?;
    let script = crate::gate_script::render(identity, checkout, commands, &paths)?;
    std::fs::write(&paths.script, script)
        .with_context(|| format!("writing {}", paths.script.display()))?;
    Command::new("bash")
        .arg(&paths.script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("running Plan gate")?;
    let text = std::fs::read_to_string(&paths.result)
        .with_context(|| format!("Plan gate produced no result {}", paths.result.display()))?;
    let parsed: ResultFile = serde_json::from_str(text.trim())
        .with_context(|| format!("parsing {}", paths.result.display()))?;
    if parsed.schema_version != 2 {
        bail!("unsupported gate result schema {}", parsed.schema_version);
    }
    let token = std::fs::read_to_string(&owner).context("reading Plan gate ownership")?;
    if token.is_empty() || parsed.owner_token != token {
        bail!("gate result ownership token does not match this attempt");
    }
    let log =
        std::fs::read(&paths.log).with_context(|| format!("reading {}", paths.log.display()))?;
    let reported = match parsed.result.as_str() {
        "pass" => GateResult::Pass,
        "fail" => GateResult::Fail,
        other => bail!("unknown gate result {other:?}"),
    };
    let receipts_valid = command_log_valid(&log, commands, &parsed.commands, reported);
    let (result, failed_cmd) = if parsed.identity != *identity {
        (
            GateResult::Fail,
            Some("stale_result: gate identity differs from the Plan attempt".into()),
        )
    } else if !receipts_valid
        || (reported == GateResult::Pass
            && (parsed.exit_code != 0 || parsed.commands.len() != commands.len()))
    {
        (GateResult::Fail, Some("invalid_command_receipts".into()))
    } else if reported == GateResult::Fail {
        let failed = std::fs::read_to_string(&paths.failed)
            .with_context(|| format!("reading {}", paths.failed.display()))?;
        (GateResult::Fail, Some(failed))
    } else {
        (GateResult::Pass, None)
    };
    Ok(GateRecord {
        identity: Some(identity.clone()),
        commands: parsed.commands,
        log_sha256: Some(digest(&log)),
        result,
        process_lost: false,
        failed_cmd,
        at: Timestamp::now(),
        sha: identity.head.clone(),
        pane: None,
        socket: None,
    })
}

/// The recorded pass still matches its unchanged log and the current gate commands.
pub(super) fn passed(ctx: &Ctx, attempt: &AggregateAttempt, commands: &[String]) -> Result<bool> {
    let Some(gate) = attempt
        .gate
        .as_ref()
        .filter(|gate| gate.result == GateResult::Pass)
    else {
        return Ok(false);
    };
    let path = paths(ctx, &attempt.identity.task, &attempt.id).log;
    let log = match std::fs::read(&path) {
        Ok(log) => log,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    Ok(gate.identity.as_ref() == Some(&attempt.identity)
        && gate.log_sha256.as_deref() == Some(digest(&log).as_str())
        && command_log_valid(&log, commands, &gate.commands, GateResult::Pass))
}
