use anyhow::Result;
use clap::Subcommand;
use corral_ops::Ctx;
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

pub fn run(ctx: &Ctx, cmd: DashboardCmd) -> Result<()> {
    let task = match cmd {
        DashboardCmd::Set {
            id,
            interval_secs,
            next_due,
            authorize_dispatch,
        } => {
            corral_ops::dashboard::configure(ctx, &id, interval_secs, next_due, authorize_dispatch)?
        }
        DashboardCmd::Show { id } => ctx.store.get(&id)?,
        DashboardCmd::Pause { id } => corral_ops::dashboard::disable(ctx, &id, false)?,
        DashboardCmd::Remove { id } => corral_ops::dashboard::disable(ctx, &id, true)?,
    };
    println!("{}", serde_json::to_string_pretty(&task.dashboard)?);
    Ok(())
}
