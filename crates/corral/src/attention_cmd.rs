use anyhow::Result;
use corral_core::Status;
use corral_ops::Ctx;

#[derive(clap::Args)]
pub struct AttentionArgs {
    #[command(subcommand)]
    pub cmd: Option<AttentionCmd>,
    /// Print the versioned assessment, including tasks without conditions
    #[arg(long, global = true)]
    pub json: bool,
    /// Include Done and Dropped tasks
    #[arg(long, global = true)]
    pub all: bool,
    /// Include only tasks with this exact project name
    #[arg(long, global = true)]
    pub project: Option<String>,
}

#[derive(clap::Subcommand)]
pub enum AttentionCmd {
    /// List the current read-only attention assessment (the default)
    List,
}

pub fn run(ctx: &Ctx, args: AttentionArgs) -> Result<()> {
    let mut board = corral_ops::attention::board(ctx, jiff::Timestamp::now())?;
    board.tasks.retain(|task| {
        (args.all || !matches!(task.status, Status::Done | Status::Dropped))
            && args
                .project
                .as_ref()
                .is_none_or(|project| task.project.as_ref() == Some(project))
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&board)?);
        return Ok(());
    }
    let mut rows = 0;
    for task in &board.tasks {
        if task.assessment.conditions.is_empty() {
            continue;
        }
        if let Some(primary) = &task.assessment.primary {
            let age = match primary.age_secs {
                Some(seconds) => format!("{seconds}s ({})", primary.since_source.label()),
                None => format!("unknown ({})", primary.since_source.label()),
            };
            let next = &primary.next_action;
            let action = match &next.command {
                Some(command) => format!("{command}: {}", next.description),
                None => next.description.clone(),
            };
            println!(
                "{}  {}  {}  {}  age {}  {}  next: {}",
                task.task,
                one_line(&task.title),
                primary.class.label(),
                primary.responsible.label(),
                age,
                one_line(&primary.reason),
                one_line(&action),
            );
            rows += 1;
        }
    }
    if rows == 0 {
        println!("No attention conditions.");
    }
    Ok(())
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
