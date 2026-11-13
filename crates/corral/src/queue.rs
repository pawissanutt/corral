use anyhow::Result;
use clap::Subcommand;
use corral_protocol::cmd::{Command, queue::QueueCommand};

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

pub fn run(inv: &crate::invocation::Invocation, command: QueueCmd) -> Result<()> {
    match command {
        QueueCmd::Add {
            id,
            profile,
            base,
            headless,
        } => {
            inv.submit(Command::Queue(QueueCommand::Add {
                id,
                profile: profile.profile,
                model: profile.model,
                effort: profile.effort,
                base,
                headless,
            }))?;
        }
        QueueCmd::Remove { id } => {
            inv.submit(Command::Queue(QueueCommand::Remove { id }))?;
        }
        QueueCmd::Pause => {
            inv.submit(Command::Queue(QueueCommand::Pause {}))?;
        }
        QueueCmd::Resume => {
            inv.submit(Command::Queue(QueueCommand::Resume {}))?;
        }
        QueueCmd::Admit { project, off } => {
            inv.submit(Command::Queue(QueueCommand::Admit { project, off }))?;
        }
        QueueCmd::List { json } => {
            let ctx = inv.ctx()?;
            let entries = corral_ops::dispatch_queue(ctx)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                let limits = &ctx.store.config.limits;
                let usage = corral_ops::slot_usage(ctx)?;
                if limits.capacity.is_some() {
                    println!("{}", corral_tui::board::slot_summary(&usage, limits));
                } else {
                    println!(
                        "coding slots: running {} / soft {} / hard {} ({} released by parked runs); review process budget: {}",
                        usage.total(),
                        corral_ops::soft_target(limits),
                        limits.running,
                        usage.parked,
                        limits.review
                    );
                }
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
