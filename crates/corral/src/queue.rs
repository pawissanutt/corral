use anyhow::Result;
use clap::Subcommand;
use corral_ops::Ctx;

use crate::ProfileArgs;

#[derive(Subcommand)]
pub enum QueueCmd {
    /// Move to Ready without launching, preserving routing and base overrides
    Add {
        id: String,
        #[command(flatten)]
        profile: ProfileArgs,
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        headless: bool,
    },
    /// Return to Backlog and hold against automatic readmission
    Remove { id: String },
    /// Legacy coding-admission hold only; use mode pause for all automatic actions
    Pause,
    /// Lift only the coding-admission hold; never resumes the operating policy or grants authority
    Resume,
    /// Restrict Ready admission to named projects (empty restriction admits all)
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
            println!("{} Ready queued (enqueue only)", task.id);
        }
        QueueCmd::Remove { id } => {
            let task = corral_ops::dequeue(ctx, &id)?;
            println!("{} Backlog (dequeued)", task.id);
        }
        QueueCmd::Pause => {
            corral_ops::dispatch_pause(ctx, true)?;
            println!(
                "coding admission paused only; `corral mode pause` pauses all automatic actions"
            );
        }
        QueueCmd::Resume => {
            corral_ops::dispatch_pause(ctx, false)?;
            println!(
                "coding admission resumed only; operating pause and scoped authority are unchanged"
            );
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
                let limits = &ctx.store.config.limits;
                let usage = corral_ops::slot_usage(ctx)?;
                println!(
                    "coding slots: running {} / soft {} / hard {} ({} released by parked runs); review process budget: {}",
                    usage.total(),
                    corral_ops::soft_target(limits),
                    limits.running,
                    usage.parked,
                    limits.review
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
