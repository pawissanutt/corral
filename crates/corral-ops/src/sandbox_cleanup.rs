//! Reap external load-sandbox containers by the unguessable launch ownership marker.
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};
use corral_core::RunOutcome;
use serde_json::json;

use crate::Ctx;

#[cfg(all(test, unix))]
#[path = "sandbox_cleanup_tests.rs"]
mod tests;

/// Own the retry retirement effect, after durable supersession and before replacement dispatch.
/// A reap failure is an observed external outcome, not part of the preceding task transaction.
/// Ownership/issuer checks stay in `reap`; failed cleanup retains that debt for later recovery.
pub(crate) fn retire_superseded(ctx: &Ctx, task: &str, run: &str) -> Result<()> {
    let head = ctx.store.head(task)?;
    let intent = head
        .run_head(run)?
        .context("sandbox retirement run missing")?;
    ensure!(
        intent.outcome == Some(RunOutcome::Superseded) && intent.ended.is_some(),
        "sandbox retirement requires durable supersession intent"
    );
    if let Err(error) = reap(ctx, task, run) {
        // Preserve retry's best-effort diagnostic behavior, but only after actual failure.
        let _ = ctx.store.log_event(
            Some(task),
            "cleanup_failed",
            json!({
                "resource": "sandbox", "error": format!("{error:#}"),
            }),
        );
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    pub(crate) static PODMAN: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// One listing regardless of historical run count. Never call under an inventory lease.
/// Contending hook passes skip rather than queue behind an external container runtime.
pub(crate) fn sweep(ctx: &Ctx) -> Result<()> {
    let _span = corral_core::timing::span("sync.sandbox_cleanup_sweep");
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
    // Launch markers are scratch; the canonical issuer and named run must also exist.
    // Never hydrate the board or retained run history to authorize container cleanup.
    let owners = terminal_owners(ctx)?;
    if owners.is_empty() {
        return Ok(());
    }
    let listing = corral_core::timing::span("sync.sandbox_cleanup_sweep.podman");
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
    drop(listing);
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
    // inventory snapshot. Markers belong to launches, never titles or task labels. Only a
    // container this sweep could remove needs that refresh.
    let candidates: Vec<(Container, String)> = containers
        .into_iter()
        .filter_map(|container| {
            let marker = container.labels.get("corral.run-owner")?.clone();
            (container
                .labels
                .get("corral.load-sandbox")
                .map(String::as_str)
                == Some("true")
                && owners.contains(&marker))
            .then_some((container, marker))
        })
        .collect();
    if candidates.is_empty() {
        return Ok(());
    }
    let current = terminal_owners(ctx)?;
    let mut errors = Vec::new();
    for (container, marker) in candidates {
        if !current.contains(&marker) {
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
    let _span = corral_core::timing::span("sync.sandbox_cleanup_sweep.owners");
    let mut fresh = ctx.clone();
    fresh.store = ctx.store.read_only_snapshot()?;
    let dir = fresh.store.scratch_path("tmp")?.join("launch-exits");
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(error) => return Err(error).context("listing sandbox owners"),
    };
    let mut eligible = BTreeSet::new();
    let mut protected = BTreeSet::new();
    // Receipts of one run (its worker, gates and reviewers) share one task and run: on this
    // pinned view each task head and each run's terminal state is read once.
    let mut heads = HashMap::new();
    let mut runs: HashMap<(String, String), bool> = HashMap::new();
    for entry in entries {
        let entry = entry.context("reading sandbox owner entry")?;
        let name = entry.file_name();
        let Some(name) = name.to_str().and_then(|s| s.strip_suffix(".owner")) else {
            continue;
        };
        // T-NNN-RN[-child...] is the launch filename contract. Match the full run
        // component (R1 must not authorize R10), including gate/reviewer children.
        let marker = std::fs::read_to_string(entry.path()).context("reading sandbox owner")?;
        validate_marker(&marker)?;
        let Some(receipt) = crate::launch_exit::process_receipt(&fresh, marker.as_bytes())? else {
            continue;
        };
        if receipt.with_extension("owner") != entry.path() {
            continue;
        }
        let mut parts = name.split('-');
        let task = parts.by_ref().take(2).collect::<Vec<_>>().join("-");
        let Some(run) = parts.next() else { continue };
        let terminal = match runs.entry((task, run.to_owned())) {
            Entry::Occupied(known) => *known.get(),
            Entry::Vacant(slot) => {
                let (task, run) = slot.key();
                let head = match heads.entry(task.clone()) {
                    Entry::Occupied(known) => known.into_mut(),
                    Entry::Vacant(slot) => slot.insert(fresh.store.head(task)?),
                };
                let terminal = matches!(
                    head.status,
                    corral_core::Status::Done | corral_core::Status::Dropped
                ) && head.launch.is_none()
                    && head.run_head(run)?.is_some();
                *slot.insert(terminal)
            }
        };
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
        if !crate::session_scope::owns_inventory(&fresh)? {
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
    reap_scope(ctx, task, Some(run))
}

pub(crate) fn reap_task(ctx: &Ctx, task: &str) -> Result<()> {
    reap_scope(ctx, task, None)
}

fn reap_scope(ctx: &Ctx, task: &str, run: Option<&str>) -> Result<()> {
    reap_scope_with(ctx, task, run, "podman")
}

fn reap_scope_with(ctx: &Ctx, task: &str, run: Option<&str>, podman: &str) -> Result<()> {
    // Inventory actual cleanup debt, never every retained owner file or task/run history.
    // A label alone grants no authority: each opaque marker must have its canonical issuer.
    let output = match Command::new(podman)
        .args([
            "ps",
            "-a",
            "--filter",
            "label=corral.load-sandbox=true",
            "--format",
            "{{index .Labels \"corral.run-owner\"}}",
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
        if selected && let Err(error) = reap_with(podman, marker) {
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
