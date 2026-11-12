//! Read-only shadow observations and offline bench-response export.
use anyhow::{Result, ensure};
use clap::Subcommand;
use std::path::Path;

#[derive(Subcommand)]
pub enum PmShadowCmd {
    /// Show cumulative counters and the most recent observations
    List {
        #[arg(long)]
        json: bool,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Export non-stale choices as pm bench response JSONL
    Export,
}

pub(crate) fn run(start: &Path, cmd: PmShadowCmd, entry: &crate::entry::Entry) -> Result<()> {
    let ctx = entry.open(start)?;
    match cmd {
        PmShadowCmd::List { json, limit } => list(&ctx, json, limit),
        PmShadowCmd::Export => export(&ctx),
    }
}

pub(crate) fn list(ctx: &corral_ops::Ctx, json: bool, limit: usize) -> Result<()> {
    ensure!(
        (1..=corral_core::agent::pm_shadow::MAX_DECISIONS).contains(&limit),
        "limit must be 1..512"
    );
    let mut shadow = corral_core::agent::pm_shadow::load(&ctx.store)?;
    shadow
        .decisions
        .drain(..shadow.decisions.len().saturating_sub(limit));
    if json {
        println!("{}", serde_json::to_string_pretty(&shadow)?);
    } else {
        println!("{}", serde_json::to_string(&shadow.counters)?);
        for decision in shadow.decisions.iter().rev() {
            println!(
                "{} {} {} {}{}",
                decision.id,
                decision.model,
                decision.kind,
                decision.choice,
                if decision.stale { " (stale)" } else { "" }
            );
        }
    }
    Ok(())
}

pub(crate) fn export(ctx: &corral_ops::Ctx) -> Result<()> {
    for row in corral_ops::pm_shadow_export(ctx)? {
        println!("{}", serde_json::to_string(&row)?);
    }
    Ok(())
}
