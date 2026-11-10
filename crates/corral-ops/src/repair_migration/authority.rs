//! Unexposed owner ceremony. No flags, serialized requests or delegated PM messages
//! construct this capability. Later integration must retain this genuine input boundary.
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
    preview_digest: String,
}

pub(super) fn recheck(store: &Store, confirmation: &Confirmation) -> Result<()> {
    crate::recovery::authority::require_owner_store(store)?;
    ensure!(
        confirmation.provenance.version == 1
            && !confirmation.provenance.reason.trim().is_empty()
            && confirmation.provenance.preview_digest == confirmation.preview.digest,
        "invalid repair import confirmation"
    );
    Ok(())
}

pub fn confirm(ctx: &crate::Ctx, proposal: &Preview, reason: &str) -> Result<Confirmation> {
    crate::recovery::authority::require_owner_store(&ctx.store)?;
    ensure!(
        !reason.trim().is_empty() && reason.len() <= 4096,
        "bounded owner reason required"
    );
    ensure!(
        std::io::stdin().is_terminal(),
        "noninteractive repair import confirmation forbidden"
    );
    ensure!(
        preview(ctx, &proposal.request)? == *proposal,
        "repair import preview stale"
    );
    #[cfg(unix)]
    {
        let mut terminal = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .context("opening repair import owner terminal")?;
        ensure!(
            terminal.is_terminal(),
            "owner controlling terminal required"
        );
        let challenge = format!(
            "import repairs {} {}",
            proposal.request.task, proposal.digest
        );
        writeln!(terminal, "Record-only repair import. Starts nothing, grants nothing.\nTask: {}\nHistory: {}\nPolicy: {}\nOrdered evidence: {}\nReason: {reason}\nType `{challenge}`:", proposal.request.task, proposal.sources.digest()?, serde_json::to_string(&proposal.policy)?, serde_json::to_string_pretty(&proposal.entries)?).context("displaying repair import confirmation")?;
        terminal
            .flush()
            .context("flushing repair import challenge")?;
        let mut answer = String::new();
        std::io::BufReader::new(&terminal)
            .take(1024)
            .read_line(&mut answer)
            .context("reading repair import owner input")?;
        ensure!(
            answer.ends_with('\n') && answer.trim_end() == challenge,
            "repair import cancelled"
        );
        crate::recovery::authority::require_owner_store(&ctx.store)?;
        ensure!(
            preview(ctx, &proposal.request)? == *proposal,
            "repair import changed during confirmation"
        );
        Ok(Confirmation {
            preview: proposal.clone(),
            provenance: Provenance {
                version: 1,
                reason: reason.trim().into(),
                terminal: "/dev/tty".into(),
                at: ctx.store.now().to_string(),
                preview_digest: proposal.digest.clone(),
            },
        })
    }
    #[cfg(not(unix))]
    anyhow::bail!("repair import requires a supported unmanaged terminal")
}

#[cfg(test)]
pub(super) fn fixture(preview: Preview) -> Confirmation {
    Confirmation {
        provenance: Provenance {
            version: 1,
            reason: "isolated fixture only".into(),
            terminal: "test-only".into(),
            at: "2026-11-01T00:00:00Z".into(),
            preview_digest: preview.digest.clone(),
        },
        preview,
    }
}
