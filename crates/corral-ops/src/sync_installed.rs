//! Sync for a long-lived process that outlived its installed executable (T-587).

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::Ctx;

/// Runs one `corral sync` pass through `installed` for a process whose own runtime
/// [latched](corral_core::Store::runtime_latched) after an update. The outdated process must not
/// write, but the installed executable is current: its pass keeps autopilot moving instead of
/// stopping until someone restarts the board. The child targets this context's store, herdr
/// executable and session; the diagnostics it prints become messages, as an in-process pass's do.
pub fn sync_installed(ctx: &Ctx, installed: &Path) -> Result<()> {
    let mut command = Command::new(installed);
    command
        .arg("--root")
        .arg(ctx.store.root())
        .arg("sync")
        .current_dir(ctx.store.root())
        .env("HERDR_BIN_PATH", ctx.herdr.bin())
        .stdin(Stdio::null());
    if let Some(session) = ctx.herdr.session() {
        command.env("CORRAL_HERDR_SESSION", session);
    }
    // The board's pass is the owner's sync, never a gate attempt of the shell that started it.
    for name in [
        "CORRAL_GATE_ATTEMPT",
        "CORRAL_CANDIDATE_HEAD",
        "CORRAL_INTEGRATION_BASE",
    ] {
        command.env_remove(name);
    }
    let output = command
        .output()
        .with_context(|| format!("running {} sync", installed.display()))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut lines = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    if !output.status.success() {
        let last = lines.next_back().unwrap_or("no diagnostics");
        bail!("installed corral sync failed ({}): {last}", output.status);
    }
    for line in lines {
        crate::messages::report(None, "sync", line.to_owned());
    }
    Ok(())
}
