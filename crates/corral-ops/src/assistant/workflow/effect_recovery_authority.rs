//! This constructor is deliberately not JSON/flag/model/owner-action callable.
use super::{Preview, preview};
use anyhow::{Context, Result, ensure};
use corral_core::Store;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, IsTerminal, Read, Write};

#[derive(Debug)]
pub struct Confirmation {
    pub(super) preview: Preview,
    pub(super) provenance: Provenance,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Provenance {
    version: u32,
    reason: String,
    terminal: String,
    at: String,
    preview_sha256: String,
}

pub(super) fn recheck(store: &Store, confirmation: &Confirmation) -> Result<()> {
    crate::recovery::authority::require_owner_store(store)?;
    ensure!(
        confirmation.provenance.version == 1
            && !confirmation.provenance.reason.trim().is_empty()
            && confirmation.provenance.preview_sha256 == confirmation.preview.digest,
        "invalid in-process effect reconciliation confirmation"
    );
    Ok(())
}

/// Only an unmanaged owner with a real controlling terminal can construct this single-use
/// exact-target capability. Prior takeover approval, task prose and preview digests cannot.
pub fn confirm(store: &Store, proposal: &Preview, reason: &str) -> Result<Confirmation> {
    crate::recovery::authority::require_owner_store(store)?;
    ensure!(
        !reason.trim().is_empty() && reason.len() <= 4096,
        "requires a bounded nonblank owner reason"
    );
    ensure!(
        std::io::stdin().is_terminal(),
        "redirected/noninteractive effect recovery authorization forbidden"
    );
    ensure!(
        preview(store, &proposal.request)? == *proposal,
        "effect recovery preview is stale"
    );
    ensure!(
        proposal.blocked.is_none(),
        "effect recovery association blocked: {}",
        proposal.blocked.as_deref().unwrap_or("unknown")
    );
    ensure!(
        proposal.selected.iter().all(|s| s.blocked.is_none()),
        "selected unknown executors cannot be reconciled"
    );
    #[cfg(unix)]
    {
        let mut tty = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .context("opening owner controlling terminal")?;
        ensure!(tty.is_terminal(), "requires a controlling owner terminal");
        let challenge = format!(
            "confirm effects {} {} {}",
            proposal.request.assistant,
            proposal.request.task.as_deref().unwrap_or("workspace"),
            proposal.digest
        );
        writeln!(tty, "Record-only effect reconciliation; NOT handoff drain or renewed consent.\n{}\nReason: {reason}\nType `{challenge}` (anything else cancels):",
            serde_json::to_string_pretty(proposal)?).context("displaying exact effect recovery proposal")?;
        tty.flush().context("flushing effect recovery challenge")?;
        let mut answer = String::new();
        std::io::BufReader::new(&tty)
            .take(1024)
            .read_line(&mut answer)
            .context("reading owner effect recovery challenge")?;
        ensure!(
            answer.ends_with('\n') && answer.trim_end() == challenge,
            "effect reconciliation cancelled"
        );
        crate::recovery::authority::require_owner_store(store)?;
        ensure!(
            preview(store, &proposal.request)? == *proposal,
            "effect recovery changed during confirmation"
        );
        Ok(Confirmation {
            preview: proposal.clone(),
            provenance: Provenance {
                version: 1,
                reason: reason.trim().into(),
                terminal: "/dev/tty".into(),
                at: corral_core::clock::now().to_string(),
                preview_sha256: proposal.digest.clone(),
            },
        })
    }
    #[cfg(not(unix))]
    anyhow::bail!("effect reconciliation requires a supported unmanaged owner terminal")
}

#[cfg(test)]
pub(super) fn fixture(proposal: Preview) -> Confirmation {
    Confirmation {
        provenance: Provenance {
            version: 1,
            reason: "isolated fixture only".into(),
            terminal: "test-only".into(),
            at: corral_core::clock::now().to_string(),
            preview_sha256: proposal.digest.clone(),
        },
        preview: proposal,
    }
}
