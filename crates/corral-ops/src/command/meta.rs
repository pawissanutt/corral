//! `meta` command executors.
use anyhow::{Context, Result};
use corral_core::ArtifactTier;
use corral_protocol::cmd::meta::MetaCommand;

use super::{Exec, Verb};

impl Verb for MetaCommand {
    fn execute(self, cx: &mut Exec<'_>) -> Result<()> {
        match self {
            Self::Pin {
                target,
                label,
                follow_latest,
            } => {
                let (n, pin) = cx.ctx.store.pin(&target, label.as_deref(), follow_latest)?;
                let follow = if pin.follow.is_some() {
                    " (follows latest)"
                } else {
                    ""
                };
                cx.out
                    .line(format_args!("pinned #{n} {}{follow}", pin.target));
                Ok(())
            }
            Self::Unpin { query } => {
                let pin = cx.ctx.store.unpin(&query)?;
                cx.out.line(format_args!("unpinned {}", pin.target));
                Ok(())
            }
            Self::DashboardSet {
                id,
                interval_secs,
                next_due,
                authorize_dispatch,
            } => {
                let task = crate::dashboard::configure(
                    &cx.ctx,
                    &id,
                    interval_secs,
                    next_due.parse()?,
                    authorize_dispatch,
                )?;
                cx.out.json_pretty(&task.dashboard)
            }
            Self::DashboardPause { id } => {
                let task = crate::dashboard::disable(&cx.ctx, &id, false)?;
                cx.out.json_pretty(&task.dashboard)
            }
            Self::DashboardRemove { id } => {
                let task = crate::dashboard::disable(&cx.ctx, &id, true)?;
                cx.out.json_pretty(&task.dashboard)
            }
            Self::ArtifactSetTier { reference, tier } => {
                let tier = ArtifactTier::parse(&tier).context("unknown artifact tier")?;
                let a = crate::set_artifact_tier(&cx.ctx, &reference, tier, cx.invoker)?;
                cx.out
                    .line(format_args!("{} {}", a.path, a.tier().as_str()));
                Ok(())
            }
            Self::Other => Err(super::unknown_command("other")),
        }
    }
}
