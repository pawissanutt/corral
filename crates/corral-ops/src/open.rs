use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// Opens `path` with the external opener: `$CORRAL_OPENER` (a program plus whitespace-separated args) when
/// set, else xdg-open (open on macOS). Null stdio. `wait` blocks until the opener exits and fails on a
/// non-zero exit; otherwise the opener is spawned detached.
pub fn open_external(path: &Path, wait: bool) -> Result<()> {
    let custom = std::env::var("CORRAL_OPENER")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let default = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let line = custom.as_deref().unwrap_or(default);
    let mut parts = line.split_whitespace();
    let prog = parts.next().unwrap_or(default);
    let mut cmd = Command::new(prog);
    cmd.args(parts)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if !wait {
        cmd.spawn()
            .with_context(|| format!("spawning {line} {}", path.display()))?;
        return Ok(());
    }
    let status = cmd
        .status()
        .with_context(|| format!("running {line} {}", path.display()))?;
    if !status.success() {
        bail!("{line} {} exited with {status}", path.display());
    }
    Ok(())
}
