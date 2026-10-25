//! `corral export`: the store's records in portable forms (plan T-253, export compatibility).

use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use corral_core::Store;
use corral_ops::Ctx;

#[derive(clap::Subcommand)]
pub enum ExportCmd {
    /// Write every task as a Markdown file (YAML frontmatter, then the body) into DIR
    Tasks {
        /// Directory to write `<id>.md` files into (created when missing)
        dir: PathBuf,
    },
    /// Write stored events as JSONL in ascending sequence order
    Events {
        /// Export events strictly after this sequence (0 exports all events)
        #[arg(long, default_value_t = 0)]
        since: u64,
        /// Emit one stored Event JSON object per line
        #[arg(long, required = true)]
        json: bool,
    },
}

pub fn run(ctx: &Ctx, cmd: ExportCmd) -> Result<()> {
    match cmd {
        ExportCmd::Tasks { dir } => tasks(ctx, &dir),
        ExportCmd::Events { since, json: _ } => events(&ctx.store, since, &mut io::stdout().lock()),
    }
}

fn events(store: &Store, mut after: u64, out: &mut impl Write) -> Result<()> {
    let snapshot = store.read_only_snapshot()?;
    loop {
        let page = snapshot
            .events_page(None, &[], None, after, 1000)
            .context("reading stored events for export")?;
        if page.is_empty() {
            return Ok(());
        }
        for event in page {
            serde_json::to_writer(&mut *out, &event).context("writing event JSON")?;
            writeln!(out).context("writing event JSON line")?;
            after = event.seq;
        }
    }
}

fn tasks(ctx: &Ctx, dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let records = ctx.store.task_records()?;
    for record in &records {
        let task = &record.task;
        let text = if record.recoveries.is_empty() {
            Store::render_task_markdown(&record.task)
        } else {
            Store::render_task_record_markdown(record)?
        };
        let path = dir.join(format!("{}.md", task.id));
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    }
    println!("exported {} task(s) to {}", records.len(), dir.display());
    Ok(())
}
