//! Reap external load-sandbox containers by the unguessable launch ownership marker.
use std::collections::{BTreeMap, BTreeSet};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};

use crate::Ctx;

#[cfg(all(test, unix))]
#[path = "sandbox_cleanup_tests.rs"]
mod tests;

#[cfg(test)]
thread_local! {
    pub(crate) static PODMAN: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// One listing regardless of historical run count. Never call under an inventory lease.
/// Contending hook passes skip rather than queue behind an external container runtime.
pub(crate) fn sweep(ctx: &Ctx) -> Result<()> {
    #[cfg(test)]
    let bin = PODMAN
        .with(|slot| slot.borrow().clone())
        .unwrap_or_else(|| "podman".into());
    #[cfg(not(test))]
    let bin = "podman".to_owned();
    sweep_with(ctx, &bin)
}

fn sweep_with(ctx: &Ctx, podman: &str) -> Result<()> {
    let Some(_flight) = ctx.store.try_lock("sandbox-sweep")? else {
        return Ok(());
    };
    let owners = terminal_owners(ctx)?;
    if owners.is_empty() {
        return Ok(());
    }
    let output = match Command::new(podman)
        .args([
            "ps",
            "-a",
            "--filter",
            "label=corral.load-sandbox=true",
            "--format",
            "json",
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
    #[derive(serde::Deserialize)]
    struct Container {
        #[serde(rename = "Id", alias = "ID")]
        id: String,
        #[serde(rename = "Labels", default)]
        labels: BTreeMap<String, String>,
    }
    let containers: Vec<Container> =
        serde_json::from_slice(&output.stdout).context("parsing sandbox containers")?;
    // Podman may have blocked while a task was resumed. Refresh from disk, not the
    // inventory snapshot. Markers belong to launches, never titles or task labels.
    let current = terminal_owners(ctx)?;
    let mut errors = Vec::new();
    for container in containers {
        let Some(marker) = container.labels.get("corral.run-owner") else {
            continue;
        };
        if container
            .labels
            .get("corral.load-sandbox")
            .map(String::as_str)
            != Some("true")
            || !owners.contains(marker)
            || !current.contains(marker)
        {
            continue;
        }
        if let Err(error) = remove_with(podman, &container.id) {
            errors.push(format!("{error:#}"));
        }
    }
    ensure!(errors.is_empty(), "{}", errors.join("; "));
    Ok(())
}

fn terminal_owners(ctx: &Ctx) -> Result<BTreeSet<String>> {
    let fresh = ctx.clone();
    let tasks = fresh.store.list()?;
    let mut runs = BTreeMap::new();
    for task in &tasks {
        let terminal = matches!(
            task.status,
            corral_core::Status::Done | corral_core::Status::Dropped
        ) && task.launch.is_none();
        for run in &task.runs {
            runs.insert(format!("{}-{}", task.id, run.id), terminal);
        }
    }
    let dir = fresh.store.state_dir().join("launch-exits");
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(error) => return Err(error).context("listing sandbox owners"),
    };
    let mut eligible = BTreeSet::new();
    let mut protected = BTreeSet::new();
    for entry in entries {
        let entry = entry.context("reading sandbox owner entry")?;
        let name = entry.file_name();
        let Some(name) = name.to_str().and_then(|s| s.strip_suffix(".owner")) else {
            continue;
        };
        // T-NNN-RN[-child...] is the launch filename contract. Match the full run
        // component (R1 must not authorize R10), including gate/reviewer children.
        let prefix = name.split('-').take(3).collect::<Vec<_>>().join("-");
        let terminal = runs.get(&prefix).copied().unwrap_or(false);
        let marker = std::fs::read_to_string(entry.path()).context("reading sandbox owner")?;
        validate_marker(&marker)?;
        if terminal {
            eligible.insert(marker);
        } else {
            protected.insert(marker);
        }
    }
    eligible.retain(|marker| !protected.contains(marker));
    if !eligible.is_empty() {
        // A no-op sweep must not add binding reads/scans to ordinary sync. Only
        // refresh ownership when these markers could authorize external cleanup.
        fresh.store.begin_session_view()?;
        if !crate::session_scope::owns_tasks(&fresh, &fresh.store, &tasks)? {
            eligible.clear();
        }
    }
    Ok(eligible)
}

fn validate_marker(marker: &str) -> Result<()> {
    ensure!(
        marker.len() == 64 && marker.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid sandbox owner"
    );
    Ok(())
}

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
    validate_marker(marker)?;
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
        remove_with(podman, id)?;
    }
    Ok(())
}

fn remove_with(podman: &str, id: &str) -> Result<()> {
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
    Ok(())
}
