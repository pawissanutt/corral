use std::io::{self, Write};

use anyhow::{Context, Result};
use corral_ops::Ctx;

pub fn run(ctx: &Ctx, id: &str) -> Result<()> {
    let task = ctx.store.get(id)?;
    let process = task
        .current_run()
        .and_then(|r| r.stuck.as_ref())
        .context("task has no stuck process; run corral sync first")?;
    println!(
        "{} stuck: {} (PID {})",
        task.id,
        process.argv.join(" "),
        process.pid
    );
    print!("Send SIGTERM, then SIGKILL after 3 seconds if needed? [y/N] ");
    io::stdout()
        .flush()
        .context("flushing confirmation prompt")?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .context("reading unstick confirmation")?;
    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        println!("Cancelled.");
        return Ok(());
    }
    let recovered = corral_ops::unstick(ctx, &task.id, process)?;
    println!("{} unstuck ({})", recovered.id, recovered.status.label());
    Ok(())
}
