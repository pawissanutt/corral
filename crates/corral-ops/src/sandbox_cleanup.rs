//! Reap external load-sandbox containers by the unguessable launch ownership marker.
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};

use crate::Ctx;

#[cfg(all(test, unix))]
#[path = "sandbox_cleanup_tests.rs"]
mod tests;

pub(crate) fn reap(ctx: &Ctx, task: &str, run: &str) -> Result<()> {
    let path = crate::launch_exit::receipt(ctx, task, run).with_extension("owner");
    let dir = path.parent().context("sandbox owner directory missing")?;
    let prefix = format!("{task}-{run}");
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("listing sandbox owners"),
    };
    let mut errors = Vec::new();
    for entry in entries {
        let entry = entry.context("reading sandbox owner entry")?;
        let name = entry.file_name();
        let Some(name) = name.to_str().and_then(|s| s.strip_suffix(".owner")) else {
            continue;
        };
        if name != prefix
            && !name
                .strip_prefix(&prefix)
                .is_some_and(|s| s.starts_with('-'))
        {
            continue;
        }
        let result = std::fs::read_to_string(entry.path())
            .context("reading sandbox owner")
            .and_then(|marker| reap_with("podman", &marker));
        if let Err(error) = result {
            errors.push(format!("{name}: {error:#}"));
        }
    }
    ensure!(errors.is_empty(), "{}", errors.join("; "));
    Ok(())
}

fn reap_with(podman: &str, marker: &str) -> Result<()> {
    ensure!(
        marker.len() == 64 && marker.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid sandbox owner"
    );
    let output = match Command::new(podman)
        .args([
            "ps",
            "-a",
            "--filter",
            "label=corral.load-sandbox=true",
            "--filter",
            &format!("label=corral.run-owner={marker}"),
            "--format",
            "{{.ID}}",
        ])
        .stdin(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("listing sandbox containers"),
    };
    ensure!(
        output.status.success(),
        "listing sandbox containers: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ids = std::str::from_utf8(&output.stdout).context("invalid container listing")?;
    for id in ids.lines().filter(|id| !id.is_empty()) {
        ensure!(
            id.len() >= 12 && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid container id"
        );
        let output = Command::new(podman)
            .args(["rm", "--force", "--time", "0", id])
            .stdin(Stdio::null())
            .output()
            .context("removing sandbox container")?;
        ensure!(
            output.status.success(),
            "removing sandbox {id}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}
