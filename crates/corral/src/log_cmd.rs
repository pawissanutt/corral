use std::io::{self, Write};

use anyhow::{Context, Result};
use clap::Args;
use corral_ops::{Ctx, TimelineFilter, timeline};

#[derive(Args)]
pub struct LogArgs {
    id: String,
    /// Include noise such as delivery receipts and unknown event kinds
    #[arg(long)]
    all: bool,
    /// Emit one TimelineEntry JSON object per line
    #[arg(long)]
    json: bool,
}

pub fn run(ctx: &Ctx, args: LogArgs) -> Result<()> {
    let filter = if args.all {
        TimelineFilter::All
    } else {
        TimelineFilter::Relevant
    };
    let entries = timeline(&ctx.store, &args.id, filter)?;
    let mut out = io::stdout().lock();
    let zone = jiff::tz::TimeZone::system();
    if !args.json {
        writeln!(
            out,
            "{:<19}  {:<20} {:<10} {:<10} {:<5} SUMMARY",
            "TIME", "KIND", "ACTOR", "VIA", "RUN"
        )
        .context("writing timeline header")?;
    }
    for entry in entries {
        if args.json {
            serde_json::to_writer(&mut out, &entry).context("writing timeline JSON")?;
            writeln!(out).context("writing timeline JSON line")?;
        } else {
            writeln!(
                out,
                "{}  {:<20} {:<10} {:<10} {:<5} {}",
                entry
                    .at
                    .to_zoned(zone.clone())
                    .strftime("%Y-%m-%d %H:%M:%S"),
                entry.kind,
                entry.actor.as_deref().unwrap_or("?"),
                entry.via.as_deref().unwrap_or("?"),
                entry.run.as_deref().unwrap_or("-"),
                entry.summary
            )
            .context("writing timeline row")?;
        }
    }
    Ok(())
}
