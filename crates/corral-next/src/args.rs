use std::{path::PathBuf, time::Duration};

use clap::{
    Args, Parser, Subcommand, ValueEnum,
    builder::{PossibleValuesParser, TypedValueParser},
};
use corral_engine::{ArtifactKind, EventKind, Priority, Status, TaskDifficulty, UsageBy};
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
    #[cfg(unix)]
    #[command(hide = true, subcommand)]
    PiStartup(crate::pi_startup_cmd::Command),
    #[command(hide = true, subcommand)]
    BoardPlan(crate::board_plan::Command),
    /// Initialize a workspace in the current directory or the selected root.
    Init {
        #[arg(long)]
        first_id: Option<u32>,
    },
    /// Import tasks from a legacy `corral export tasks` directory into an initialized workspace.
    Import {
        #[arg(value_name = "DIR")]
        dir: PathBuf,
        /// Include Done and Dropped tasks and retain dependencies on Done tasks.
        #[arg(long)]
        all: bool,
        /// A legacy `corral export events --json` file: dates each landing, move to Done and
        /// drop from the old event log instead of the task's last update.
        #[arg(long, value_name = "FILE")]
        events: Option<PathBuf>,
        /// Report what would be imported without changing tasks, events, or IDs.
        #[arg(long)]
        dry_run: bool,
    },
    /// Create a task (Backlog unless --idea or --ready is set).
    New {
        title: String,
        /// Read the task body from a UTF-8 file.
        #[arg(long, value_name = "FILE")]
        body_file: Option<PathBuf>,
        /// Store a file as a reference artifact. Repeat for multiple files.
        #[arg(long, value_name = "PATH")]
        attach: Vec<PathBuf>,
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
        /// Estimated work volume: xs, s, m, l, xl.
        #[arg(long)]
        size: Option<String>,
        /// Technical complexity or uncertainty: routine, standard, hard.
        #[arg(long)]
        difficulty: Option<TaskDifficulty>,
        /// Repeat for multiple labels. Suggested: accessibility, bug, dependencies, documentation, performance, testing. Custom labels also work.
        #[arg(long = "tag", value_name = "LABEL")]
        tags: Vec<String>,
        /// Target branch for dispatch, overriding the project default.
        #[arg(long)]
        base: Option<String>,
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
    Comment(Message),
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
    Reply(Message),
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
    /// Record or retry a saved reviewer verdict request.
    ReviewVerdict {
        task: String,
        run: String,
        request: String,
    },
    /// Send work back with an owner note.
    Changes(Text),
    /// Land a task and clean up its worktree.
    Approve {
        id: String,
        /// Land onto this branch instead of the run's recorded base.
        #[arg(long)]
        onto: Option<String>,
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
    /// Work with configured extensions.
    #[command(subcommand)]
    Ext(ExtCommand),
    /// List or check committed behaviour diagrams.
    #[command(subcommand)]
    Lens(crate::lens::LensCommand),
    /// List project tools or run one and print its stdout unchanged.
    Tool {
        /// Select the project before specifying a tool name.
        #[arg(short, long)]
        project: Option<String>,
        /// Tool name followed by arguments to pass through (including flags).
        #[arg(value_names = ["NAME", "ARGS"], trailing_var_arg = true, num_args = 0..)]
        invocation: Vec<String>,
    },
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
pub(crate) struct Message {
    #[command(flatten)]
    pub text: Text,
    /// Store a file as an artifact and append its reference. Repeat for multiple files.
    #[arg(long, value_name = "PATH")]
    pub attach: Vec<PathBuf>,
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
    /// Set work volume (xs, s, m, l, xl); an empty value clears it.
    #[arg(long)]
    pub size: Option<String>,
    /// Set difficulty (routine, standard, hard); an empty value clears it.
    #[arg(long)]
    pub difficulty: Option<String>,
    /// Replace all tags with these labels. Repeat for multiple labels.
    #[arg(long = "tag", value_name = "LABEL", conflicts_with = "clear_tags")]
    pub tags: Vec<String>,
    /// Remove all tags.
    #[arg(long)]
    pub clear_tags: bool,
    #[arg(long)]
    pub profile: Option<String>,
    /// Set the target branch for dispatch; an empty value clears it.
    #[arg(long)]
    pub base: Option<String>,
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
        /// Task ID or configured project name.
        #[arg(value_name = "OWNER")]
        owner: String,
        path: PathBuf,
        /// Artifact kind (required except for the owner, who defaults to reference).
        #[arg(long, value_parser = artifact_kind_parser())]
        kind: Option<ArtifactKind>,
        #[arg(long)]
        title: Option<String>,
        /// Replace the artifact at this reference without changing its stored contents.
        #[arg(long, value_name = "REF")]
        replaces: Option<String>,
    },
    List {
        /// Task ID or configured project name (omit to list the whole catalog).
        #[arg(value_name = "OWNER")]
        owner: Option<String>,
        /// Include replaced artifacts in text output (JSON always includes them).
        #[arg(long)]
        all: bool,
    },
    /// Write the blob or cited Markdown section to stdout, or show metadata with --json.
    Show {
        #[arg(value_name = "REF")]
        reference: String,
    },
}

pub(crate) fn artifact_kinds() -> impl Iterator<Item = ArtifactKind> {
    ArtifactKind::ALL
        .into_iter()
        .filter(|&kind| kind != ArtifactKind::Legacy)
}

fn artifact_kind_parser() -> impl TypedValueParser<Value = ArtifactKind> {
    PossibleValuesParser::new(artifact_kinds().map(ArtifactKind::as_str)).try_map(|name| {
        artifact_kinds()
            .find(|kind| kind.as_str() == name)
            .ok_or_else(|| format!("unknown artifact kind {name}"))
    })
}

#[derive(Subcommand)]
pub(crate) enum UsageCommand {
    /// Record a request. API estimates require all four token counts; pass 0 explicitly when known.
    Add {
        #[arg(long)]
        model: String,
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        input: Option<u64>,
        #[arg(long)]
        output: Option<u64>,
        #[arg(long)]
        cache_read: Option<u64>,
        #[arg(long)]
        cache_write: Option<u64>,
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

#[derive(Subcommand)]
pub(crate) enum ExtCommand {
    /// Run every line of an extension once with sample input (may have side effects).
    Check {
        name: String,
        /// Project context (otherwise CORRAL_PROJECT or the first eligible project).
        #[arg(long)]
        project: Option<String>,
    },
    /// Run one delivery pass of events to every enabled extension with `run`.
    Deliver,
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
    fn tool_preserves_arguments_after_the_name() {
        let cli = Cli::try_parse_from([
            "corral-next",
            "tool",
            "--project",
            "site",
            "context",
            "--json",
            "--project",
            "two words",
            "--",
            "",
        ])
        .unwrap();
        assert!(!cli.json);
        assert!(matches!(
            cli.command,
            Command::Tool { project: Some(project), invocation }
                if project == "site"
                    && invocation == ["context", "--json", "--project", "two words", "--", ""]
        ));
    }

    #[test]
    fn tool_accepts_option_like_names_after_separator() {
        let cli =
            Cli::try_parse_from(["corral-next", "tool", "--json", "--", "--help", "--x"]).unwrap();
        assert!(cli.json);
        assert!(matches!(
            cli.command,
            Command::Tool { project: None, invocation } if invocation == ["--help", "--x"]
        ));
    }

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
