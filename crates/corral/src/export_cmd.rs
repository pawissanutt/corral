//! `corral export`: the store's records in portable forms (plan T-253, export compatibility).

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
}

pub fn run(ctx: &Ctx, cmd: ExportCmd) -> Result<()> {
    match cmd {
        ExportCmd::Tasks { dir } => tasks(ctx, &dir),
    }
}

fn tasks(ctx: &Ctx, dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tasks = ctx.store.list()?;
    for task in &tasks {
        let record = ctx.store.task_record(&task.id)?;
        let text = if record.recoveries.is_empty() {
            Store::render_task_markdown(&record.task)
        } else {
            Store::render_task_record_markdown(&record)?
        };
        let path = dir.join(format!("{}.md", task.id));
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    }
    println!("exported {} task(s) to {}", tasks.len(), dir.display());
    Ok(())
}
