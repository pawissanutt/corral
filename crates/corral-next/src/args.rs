use std::{path::PathBuf, time::Duration};

use clap::{Args, Parser, Subcommand, ValueEnum};
use corral_engine::{EventKind, Priority, Status, UsageBy};
use serde::de::{DeserializeOwned, value::StrDeserializer};

#[derive(Parser)]
#[command(version, about = "Manage tasks and agent runs in a Corral workspace")]
pub(crate) struct Cli {
    /// Workspace root (otherwise use CORRAL_NEXT_ROOT or search ancestors).
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,
    /// Print JSON instead of human-readable text.
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Initialize a workspace in the current directory or the selected root.
    Init {
        #[arg(long)]
        first_id: Option<u32>,
    },
    /// Create a task (Backlog unless --idea or --ready is set).
    New {
        title: String,
        #[arg(short, long)]
        project: Option<String>,
        #[arg(long, conflicts_with = "ready")]
        idea: bool,
        #[arg(long)]
        ready: bool,
        #[arg(long, value_parser = from_name::<Priority>, default_value = "P2")]
        priority: Priority,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        size: Option<String>,
        #[arg(long)]
        parent: Option<String>,
        #[arg(long)]
        depends_on: Option<String>,
    },
    /// List tasks (excludes Done and Dropped by default).
    List {
        #[arg(long, value_parser = Status::parse)]
        status: Option<Status>,
        #[arg(long, short)]
        project: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// Show task details, runs, newest checks and artifacts.
    Show(Id),
    /// Show a task's event log in sequence order.
    Log(Id),
    /// Update task fields, or edit the title and body in $EDITOR with no flags.
    Edit(Edit),
    /// Append text to a task's body.
    Comment(Text),
    /// Change a task's status.
    Move {
        id: String,
        #[arg(value_parser = Status::parse)]
        status: Status,
        #[arg(long)]
        force: bool,
    },
    /// Create a worktree and start a worker.
    Dispatch {
        id: String,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        force: bool,
    },
    /// Send text to the worker.
    Reply(Text),
    /// Stop the worker session.
    Stop(Id),
    /// Restart a worker, optionally starting over from base.
    Retry {
        id: String,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        fresh: bool,
        #[arg(long)]
        note: Option<String>,
    },
    /// Submit the worker's changes and start the pipeline.
    Done {
        id: String,
        #[arg(long)]
        summary: String,
    },
    /// Run the project's gate on the worktree HEAD.
    Gate(Id),
    /// Start a reviewer.
    Review {
        id: String,
        #[arg(long)]
        profile: Option<String>,
    },
    /// Record a reviewer's verdict.
    Verdict {
        id: String,
        verdict: Verdict,
        #[arg(long, conflicts_with = "notes_file")]
        notes: Option<String>,
        #[arg(long)]
        notes_file: Option<PathBuf>,
    },
    /// Send work back with an owner note.
    Changes(Text),
    /// Land a task and clean up its worktree.
    Approve {
        id: String,
        #[arg(long)]
        force: bool,
    },
    /// Mark a task as Dropped.
    Drop(Id),
    /// Return a task to Backlog.
    Reopen(Id),
    /// Reconcile sessions, start Ready tasks, and advance pending pipelines.
    Sync {
        /// Repeat according to sync.watch_secs until interrupted.
        #[arg(long)]
        watch: bool,
    },
    /// Wait for the first matching event (exit 124 if the timeout expires).
    Wait {
        #[arg(long = "for", value_delimiter = ',', value_parser = from_name::<EventKind>)]
        kinds: Vec<EventKind>,
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        project: Option<String>,
        /// Only match events with a sequence number greater than this value
        /// (default: the newest event at the time of the call).
        #[arg(long)]
        after: Option<u64>,
        #[arg(long, default_value = "30m", value_parser = duration)]
        timeout: Duration,
    },
    /// Run one sync pass for a herdr event, always exiting successfully.
    Hook,
    /// Store or retrieve task artifacts.
    #[command(subcommand)]
    Artifact(ArtifactCommand),
    /// Report usage totals or record a model request.
    #[command(args_conflicts_with_subcommands = true)]
    Usage {
        #[arg(long, value_parser = from_name::<UsageBy>)]
        by: Option<UsageBy>,
        #[command(subcommand)]
        command: Option<UsageCommand>,
    },
    /// Check the workspace configuration.
    #[command(subcommand)]
    Config(ConfigCommand),
    #[command(hide = true)]
    Pipeline { task: String },
}

#[derive(Args)]
pub(crate) struct Id {
    pub id: String,
}

#[derive(Args)]
pub(crate) struct Text {
    pub id: String,
    pub text: String,
}

#[derive(Args)]
pub(crate) struct Edit {
    pub id: String,
    #[arg(long)]
    pub title: Option<String>,
    #[arg(long)]
    pub body_file: Option<PathBuf>,
    /// Use an empty value to clear the project.
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long, value_parser = from_name::<Priority>)]
    pub priority: Option<Priority>,
    #[arg(long)]
    pub kind: Option<String>,
    #[arg(long)]
    pub size: Option<String>,
    #[arg(long)]
    pub profile: Option<String>,
    #[arg(long)]
    pub parent: Option<String>,
    /// Comma-separated replacement dependency list; an empty value clears it.
    #[arg(long)]
    pub depends_on: Option<String>,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Verdict {
    Approve,
    Changes,
}

#[derive(Subcommand)]
pub(crate) enum ArtifactCommand {
    Add {
        id: String,
        path: PathBuf,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        title: Option<String>,
    },
    List {
        id: Option<String>,
    },
    /// Write the blob to stdout, or show its metadata with --json.
    Show {
        #[arg(value_name = "REF")]
        reference: String,
    },
}

#[derive(Subcommand)]
pub(crate) enum UsageCommand {
    Add {
        #[arg(long)]
        model: String,
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        run: Option<String>,
        #[arg(long, default_value_t)]
        input: u64,
        #[arg(long, default_value_t)]
        output: u64,
        #[arg(long, default_value_t)]
        cache_read: u64,
        #[arg(long, default_value_t)]
        cache_write: u64,
        #[arg(long, allow_negative_numbers = true)]
        usd: Option<f64>,
        #[arg(long)]
        billing: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum ConfigCommand {
    Check,
}

fn from_name<T: DeserializeOwned>(name: &str) -> Result<T, serde::de::value::Error> {
    T::deserialize(StrDeserializer::new(name))
}

fn duration(value: &str) -> anyhow::Result<Duration> {
    use anyhow::Context as _;
    value
        .parse::<jiff::SignedDuration>()
        .context("parse timeout duration (e.g. 30m or 250ms)")?
        .try_into()
        .context("timeout must not be negative")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globals_work_with_nested_usage_commands() {
        let cli = Cli::try_parse_from([
            "corral-next",
            "--root",
            "workspace",
            "usage",
            "add",
            "--model",
            "example",
            "--json",
        ])
        .unwrap();
        assert_eq!(cli.root, Some("workspace".into()));
        assert!(cli.json);
        assert!(matches!(
            cli.command,
            Command::Usage {
                by: None,
                command: Some(UsageCommand::Add { model, .. }),
            } if model == "example"
        ));
        assert!(
            Cli::try_parse_from([
                "corral-next",
                "usage",
                "--by",
                "day",
                "add",
                "--model",
                "example",
            ])
            .is_err()
        );
    }
}
