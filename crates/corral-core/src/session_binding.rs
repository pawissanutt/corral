//! Explicit, durable ownership migration for references predating saved sockets.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{Run, Store, store::atomic_write};

/// Launch provenance supplied by the operator, never inferred from current config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrSessionBinding {
    pub socket: PathBuf,
    pub launch_evidence: String,
    pub bound_at: Timestamp,
}

impl Store {
    /// This is durable ownership, not rebuildable `state/` data.
    pub fn herdr_session_binding(&self) -> Result<Option<HerdrSessionBinding>> {
        let path = self.dir().join("herdr-session.json");
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display()))
                .map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Explicit migration: the caller attests that the evidence identifies the launch
    /// server of every legacy pane in this store. Existing ownership cannot be replaced.
    pub fn bind_herdr_session(&self, socket: &Path, launch_evidence: &str) -> Result<()> {
        if !socket.is_absolute() || launch_evidence.trim().is_empty() {
            bail!("binding requires an absolute socket path and nonempty launch evidence");
        }
        self.check_runtime()?;
        let _board = self.lock("dispatch")?;
        if let Some(binding) = self.herdr_session_binding()? {
            if binding.socket != socket {
                bail!("store is already bound to {}", binding.socket.display());
            }
            return Ok(());
        }
        for task in self.list()? {
            if let Some(saved) = task
                .launch
                .as_ref()
                .and_then(|launch| launch.socket.as_ref())
                && saved != socket
            {
                bail!("{} has a launch owned by {}", task.id, saved.display());
            }
            for run in &task.runs {
                check_run(run, socket)?;
            }
            for reference in task
                .plans
                .iter()
                .flat_map(|plan| &plan.reviews)
                .filter_map(|review| review.herdr.as_ref())
            {
                check_socket(reference.socket.as_deref(), socket)?;
            }
        }
        let binding = HerdrSessionBinding {
            socket: socket.to_path_buf(),
            launch_evidence: launch_evidence.into(),
            bound_at: Timestamp::now(),
        };
        atomic_write(
            &self.dir().join("herdr-session.json"),
            &serde_json::to_string_pretty(&binding).context("serializing herdr session binding")?,
        )
    }
}

fn check_socket(saved: Option<&Path>, socket: &Path) -> Result<()> {
    if let Some(saved) = saved
        && saved != socket
    {
        bail!(
            "saved pane belongs to {}, not {}",
            saved.display(),
            socket.display()
        );
    }
    Ok(())
}

fn check_run(run: &Run, socket: &Path) -> Result<()> {
    if let Some(reference) = &run.herdr {
        check_socket(reference.socket.as_deref(), socket)?;
    }
    for child in &run.review_runs {
        check_run(child, socket)?;
    }
    Ok(())
}
