//! Reap external load-sandbox containers by the unguessable launch ownership marker.
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};

use crate::Ctx;

#[cfg(all(test, unix))]
#[path = "sandbox_cleanup_tests.rs"]
mod tests;

pub(crate) fn reap(ctx: &Ctx, task: &str, run: &str) -> Result<()> {
    let path = crate::launch_exit::receipt(ctx, task, run).with_extension("owner");
    let marker = match std::fs::read_to_string(&path) {
        Ok(marker) => marker,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("reading sandbox owner"),
    };
    reap_with("podman", &marker)
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
