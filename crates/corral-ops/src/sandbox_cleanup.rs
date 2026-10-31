//! Reap external load-sandbox containers by the unguessable launch ownership marker.
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};

use crate::Ctx;

#[cfg(all(test, unix))]
#[path = "sandbox_cleanup_tests.rs"]
mod tests;

pub(crate) fn reap(ctx: &Ctx, task: &str, run: &str) -> Result<()> {
    reap_scope(ctx, task, Some(run))
}

pub(crate) fn reap_task(ctx: &Ctx, task: &str) -> Result<()> {
    reap_scope(ctx, task, None)
}

fn reap_scope(ctx: &Ctx, task: &str, run: Option<&str>) -> Result<()> {
    // Inventory actual cleanup debt, never every retained owner file or task/run history.
    // A label alone grants no authority: each opaque marker must have its canonical issuer.
    let output = match Command::new("podman")
        .args([
            "ps",
            "-a",
            "--filter",
            "label=corral.load-sandbox=true",
            "--format",
            "{{.Label \"corral.run-owner\"}}",
        ])
        .stdin(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("listing sandbox owners"),
    };
    ensure!(
        output.status.success(),
        "listing sandbox owners: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    if output.stdout.is_empty() {
        return Ok(());
    }
    let mut read = ctx.clone();
    read.store = ctx.store.read_only_snapshot()?;
    let prefix = run.map(|run| format!("{task}-{run}"));
    let task_prefix = format!("{task}-R");
    let markers: std::collections::BTreeSet<_> = std::str::from_utf8(&output.stdout)
        .context("invalid sandbox ownership listing")?
        .lines()
        .collect();
    let mut errors = Vec::new();
    for marker in markers {
        let Some(receipt) = crate::launch_exit::process_receipt(&read, marker.as_bytes())? else {
            continue;
        };
        let Some(name) = receipt.file_stem().and_then(|name| name.to_str()) else {
            continue;
        };
        let selected = match &prefix {
            Some(prefix) => {
                name == prefix.as_str()
                    || name
                        .strip_prefix(prefix)
                        .is_some_and(|tail| tail.starts_with('-'))
            }
            None => name.strip_prefix(&task_prefix).is_some_and(|tail| {
                let n = tail.split('-').next().unwrap_or_default();
                !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())
            }),
        };
        if selected && let Err(error) = reap_with("podman", marker) {
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
