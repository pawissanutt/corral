use anyhow::Result;
use clap::Subcommand;
use jiff::Timestamp;

#[derive(Debug, Subcommand)]
pub enum DashboardCmd {
    /// Authorize recurring dispatch of a report task (also resumes a paused schedule)
    Set {
        id: String,
        #[arg(long)]
        interval_secs: u32,
        /// Explicit first/next due timestamp, e.g. 2026-10-16T12:00:00Z
        #[arg(long)]
        next_due: Timestamp,
        /// Grant permission for sync to enqueue successive runs automatically
        #[arg(long)]
        authorize_dispatch: bool,
    },
    /// Show a task's schedule as JSON (null means absent)
    Show { id: String },
    /// Pause future occurrences; already queued/running work remains authorized
    Pause { id: String },
    /// Remove future recurrence; already queued/running work remains authorized
    Remove { id: String },
}

pub fn run(inv: &crate::invocation::Invocation, cmd: DashboardCmd) -> Result<()> {
    use corral_protocol::cmd::{Command, meta::MetaCommand};
    let command = match cmd {
        DashboardCmd::Set {
            id,
            interval_secs,
            next_due,
            authorize_dispatch,
        } => MetaCommand::DashboardSet {
            id,
            interval_secs,
            next_due: next_due.to_string(),
            authorize_dispatch,
        },
        DashboardCmd::Show { id } => {
            let ctx = inv.ctx()?;
            let task = ctx.store.get(&id)?;
            println!("{}", serde_json::to_string_pretty(&task.dashboard)?);
            return Ok(());
        }
        DashboardCmd::Pause { id } => MetaCommand::DashboardPause { id },
        DashboardCmd::Remove { id } => MetaCommand::DashboardRemove { id },
    };
    inv.submit(Command::Meta(command))
}
