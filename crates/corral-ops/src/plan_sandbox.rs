//! The same device-capable, read-only repository wrapper for probes and planners.
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};

use crate::prompt;

pub(crate) fn command(repo: &Path, store: &Path, command: &str) -> String {
    let quote = |p: &Path| format!("'{}'", prompt::sh_escape(&p.display().to_string()));
    format!(
        "bwrap --bind / / --dev /dev --proc /proc --ro-bind {repo} {repo} \
         --bind {store} {store} --chdir {repo} -- /bin/sh -c '{}'",
        prompt::sh_escape(command),
        repo = quote(repo),
        store = quote(store),
    )
}

pub(crate) fn check(repo: &Path, store: &Path) -> Result<()> {
    let output = Command::new("/bin/sh")
        .args(["-c", &command(repo, store, "/bin/true")])
        .stdin(Stdio::null())
        .output()
        .context("plan mode requires bubblewrap (bwrap) for read-only repository access")?;
    ensure!(
        output.status.success(),
        "plan sandbox unavailable: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[cfg(test)]
#[path = "plan_sandbox_tests.rs"]
mod tests;
