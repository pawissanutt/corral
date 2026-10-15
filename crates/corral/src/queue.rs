use anyhow::Result;
use clap::Subcommand;
use corral_ops::Ctx;

use crate::ProfileArgs;

#[derive(Subcommand)]
pub enum QueueCmd {
    /// Enqueue only, preserving explicit routing and base overrides
    Add {
        id: String,
        #[command(flatten)]
        profile: ProfileArgs,
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        headless: bool,
    },
    /// Dequeue and exclude from automatic Ready admission
    Remove { id: String },
    /// Pause all new coding dispatches, independently of review autopilot
    Pause,
    /// Resume coding dispatches; sync drains the queue
    Resume,
    /// Opt exactly this project into Ready admission (or revoke with --off)
    Admit {
        project: String,
        #[arg(long)]
        off: bool,
    },
    /// List candidates in dispatch order with their waiting reasons
    List {
        #[arg(long)]
        json: bool,
    },
}

pub fn run(ctx: &Ctx, command: QueueCmd) -> Result<()> {
    match command {
        QueueCmd::Add {
            id,
            profile,
            base,
            headless,
        } => {
            let opts = profile.dispatch_opts(headless, base, true);
            let task = corral_ops::enqueue(ctx, &id, &opts)?;
            println!("{} queued (enqueue only)", task.id);
        }
        QueueCmd::Remove { id } => {
            let task = corral_ops::dequeue(ctx, &id)?;
            println!("{} dequeued", task.id);
        }
        QueueCmd::Pause => {
            corral_ops::dispatch_pause(ctx, true)?;
            println!("dispatch paused; review autopilot is independent");
        }
        QueueCmd::Resume => {
            corral_ops::dispatch_pause(ctx, false)?;
            println!("dispatch resumed");
        }
        QueueCmd::Admit { project, off } => {
            corral_ops::admit_ready(ctx, &project, !off)?;
            println!(
                "{project}: Ready admission {}",
                if off { "off" } else { "on" }
            );
        }
        QueueCmd::List { json } => {
            let entries = corral_ops::dispatch_queue(ctx)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                println!(
                    "coding slots: {}; review process budget: {}",
                    ctx.store.config.limits.running, ctx.store.config.limits.review
                );
                for entry in entries {
                    println!(
                        "{} {} {}: {}",
                        entry.id,
                        entry.priority,
                        entry.project.as_deref().unwrap_or("(no project)"),
                        entry.reason
                    );
                }
            }
        }
    }
    Ok(())
}
