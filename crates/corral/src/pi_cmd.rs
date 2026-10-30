//! `corral pi`: start the Pi-native host from nothing. Handler bodies live in
//! `corral_ops::agent::pi`.
use std::path::Path;

use anyhow::Result;
use clap::Args;
use corral_ops::agent::hooks::pm::Overrides;
use corral_ops::agent::pi::{self, BareArgs};

use crate::entry::Entry;

#[derive(Args)]
pub struct PiArgs {
    /// Plain unbound Pi in the current directory: no store, binding or corral tools
    #[arg(long)]
    bare: bool,
    /// Model to run instead of the PM profile's (bare: Pi's default for the provider)
    #[arg(long)]
    model: Option<String>,
    /// Provider whose `corral agent login` runs the session (bare default: openai-codex)
    #[arg(long, value_parser = ["openai-codex", "anthropic"])]
    provider: Option<String>,
    /// Reasoning effort instead of the PM profile's
    #[arg(long)]
    effort: Option<String>,
}

pub fn run(start: &Path, args: PiArgs, entry: &Entry) -> Result<()> {
    if args.bare {
        return pi::bare(&BareArgs {
            model: args.model,
            provider: args.provider,
            effort: args.effort,
        });
    }
    let ctx = entry.open(start)?;
    let overrides = Overrides {
        model: args.model,
        provider: args.provider,
        effort: args.effort,
    };
    pi::bound(start, &ctx, &overrides)
}
