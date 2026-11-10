use super::*;
use std::io::{BufRead, IsTerminal, Read, Write};

/// An in-process, single-use value. Persisted attestation and preview JSON cannot construct it.
#[derive(Debug)]
pub struct OwnerConfirmation {
    pub(super) preview: RecoveryPreview,
    pub(super) authorization: RecoveryAuthorization,
}

/// Known managed-agent contexts can never grant recovery, on any owner surface.
fn require_unmanaged() -> Result<()> {
    // Unlike ordinary owner actions, delegation through an assistant is never recovery authority.
    for name in [
        "AI_AGENT",
        "PI_CODING_AGENT",
        "CORRAL_ASSISTANT",
        "CORRAL_PM",
        "CORRAL_TASK",
        "CORRAL_RUN",
        "CORRAL_AGENT_SESSION",
        "CORRAL_AGENT_ROLE",
    ] {
        ensure!(
            std::env::var_os(name).is_none(),
            "recovery authorization is unavailable in managed context ({name})"
        );
    }
    Ok(())
}

/// The owner checks for the confirmation's surface. The CLI ceremony keeps its full fence; the
/// interactive owner board is trusted inside herdr, but never in a managed-agent context.
pub(super) fn require_owner_for(ctx: &Ctx, interaction: OwnerInteraction) -> Result<()> {
    match interaction {
        OwnerInteraction::Cli => require_owner(ctx),
        OwnerInteraction::Tui => require_unmanaged(),
    }
}

pub(super) fn require_owner(ctx: &Ctx) -> Result<()> {
    require_owner_store(&ctx.store)
}

/// Shared ceremony fence only; this does not grant any recovery operation or waiver.
pub(crate) fn require_owner_store(store: &corral_core::Store) -> Result<()> {
    require_unmanaged()?;
    let cwd = std::env::current_dir().context("reading recovery owner directory")?;
    crate::automation_controls::require_owner_entry(store, &cwd, false, false)?;
    // A pane binding is not an owner interaction. Refuse even if environment role flags vanished.
    // The CLI ceremony is deliberately outside herdr: a pane may be a planner/reviewer or
    // a PM with no task, and absent/contradictory role metadata is not owner authority.
    ensure!(
        std::env::var_os("HERDR_PANE_ID").is_none(),
        "recovery owner confirmation requires an unmanaged local terminal outside herdr"
    );
    Ok(())
}

/// Trusted local CLI ceremony. There is deliberately no constructor taking an actor or `yes`.
/// TUI integration must obtain the same exact confirmation through its trusted owner surface.
pub fn recover_confirm(
    ctx: &Ctx,
    preview: &RecoveryPreview,
    input: &RecoveryInput,
    reason: &str,
) -> Result<OwnerConfirmation> {
    require_owner(ctx)?;
    ensure!(
        !reason.trim().is_empty(),
        "recovery needs a nonblank owner reason"
    );
    ensure!(
        std::io::stdin().is_terminal(),
        "noninteractive recovery authorization is forbidden"
    );
    current_preview(ctx, preview, input)?;
    #[cfg(unix)]
    {
        let mut terminal = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .context("opening controlling owner terminal")?;
        ensure!(
            terminal.is_terminal(),
            "recovery needs a controlling owner terminal"
        );
        writeln!(
            terminal,
            "Owner recovery (not ordinary policy success):\n{}\nReason: {}",
            serde_json::to_string_pretty(preview)?,
            reason
        )
        .context("displaying recovery confirmation")?;
        let expected = format!("confirm {} {}", preview.request.target.task, preview.digest);
        writeln!(
            terminal,
            "Type `{expected}` to authorize this exact operation (anything else cancels):"
        )
        .context("displaying recovery challenge")?;
        terminal.flush().context("flushing recovery challenge")?;
        let mut response = String::new();
        // Bound input; never accept a partial, oversized confirmation.
        std::io::BufReader::new(&terminal)
            .take(1024)
            .read_line(&mut response)
            .context("reading owner recovery confirmation")?;
        ensure!(response.trim_end() == expected, "recovery cancelled");
        require_owner(ctx)?;
        current_preview(ctx, preview, input)?;
        Ok(OwnerConfirmation {
            preview: preview.clone(),
            authorization: RecoveryAuthorization {
                version: RECOVERY_VERSION,
                preview_digest: preview.digest.clone(),
                reason: reason.to_string(),
                attestation: RecoveryAttestation::LocalOwnerAttestation {
                    interaction: OwnerInteraction::Cli,
                    at: corral_core::clock::now(),
                    controlling_terminal: "/dev/tty".into(),
                    context: None,
                },
            },
        })
    }
    #[cfg(not(unix))]
    anyhow::bail!("trusted recovery terminal interaction is not available on this platform")
}

/// The interactive owner board's explicit confirmation. It binds the exact current preview and
/// the owner's reason in-process; there is no persisted or model-callable form of this value.
/// Known managed-agent contexts are refused; the board itself is trusted inside herdr.
pub fn recover_confirm_tui(
    ctx: &Ctx,
    preview: &RecoveryPreview,
    input: &RecoveryInput,
    reason: &str,
) -> Result<OwnerConfirmation> {
    require_unmanaged()?;
    ensure!(
        !reason.trim().is_empty(),
        "recovery needs a nonblank owner reason"
    );
    current_preview(ctx, preview, input)?;
    let pane = std::env::var("HERDR_PANE_ID").ok();
    Ok(OwnerConfirmation {
        preview: preview.clone(),
        authorization: RecoveryAuthorization {
            version: RECOVERY_VERSION,
            preview_digest: preview.digest.clone(),
            reason: reason.trim().to_string(),
            attestation: RecoveryAttestation::LocalOwnerAttestation {
                interaction: OwnerInteraction::Tui,
                at: corral_core::clock::now(),
                controlling_terminal: pane.map_or_else(|| "tui".into(), |p| format!("tui:{p}")),
                context: None,
            },
        },
    })
}
