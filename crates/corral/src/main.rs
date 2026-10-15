mod cmds;
mod plugin;
mod skill;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "corral",
    version,
    about = "Project management control plane for agentic coding, inside herdr"
)]
pub struct Cli {
    /// Workspace root (the directory containing .corral/); default: discovered from the current dir
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Create .corral/ in --root or the current directory
    Init,
    /// Create a task and print its id
    New {
        #[arg(required = true, num_args = 1..)]
        title: Vec<String>,
        #[arg(short, long)]
        project: Option<String>,
        #[arg(long, conflicts_with = "ready")]
        idea: bool,
        #[arg(long)]
        ready: bool,
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        body_file: Option<PathBuf>,
        /// Preferred profile for dispatch
        #[arg(long, visible_alias = "agent")]
        profile: Option<String>,
        #[arg(long, value_delimiter = ',')]
        depends: Vec<String>,
        #[arg(long)]
        priority: Option<String>,
    },
    /// List tasks (done/dropped hidden unless --all)
    List {
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show one task
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Move a task to another status
    Move {
        id: String,
        status: String,
        #[arg(long)]
        force: bool,
    },
    /// Open the task file in $EDITOR
    Edit { id: String },
    /// Start an agent run for a task
    Dispatch {
        id: String,
        #[command(flatten)]
        profile: ProfileArgs,
        #[arg(long)]
        headless: bool,
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        queue: bool,
    },
    /// Send a reply to the task's agent
    Reply {
        id: String,
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
    },
    /// Mark the run finished and move the task to Review
    Done {
        id: String,
        #[arg(long)]
        summary: String,
    },
    /// Manage artifacts
    Artifact {
        #[command(subcommand)]
        cmd: ArtifactCmd,
    },
    /// Show the review summary of a task
    Review {
        id: String,
        #[arg(long)]
        diff: bool,
    },
    /// Merge the run and mark the task Done
    Approve {
        id: String,
        #[arg(long, default_value = "rebase", value_parser = ["ff", "rebase", "keep"])]
        merge: String,
        #[arg(long)]
        keep_worktree: bool,
    },
    /// Request changes on a task in Review
    Changes {
        id: String,
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
    },
    /// Start a fresh run for a task
    Retry {
        id: String,
        #[command(flatten)]
        profile: ProfileArgs,
    },
    /// Discard the current run and drop the task
    Discard { id: String },
    /// Focus the task's herdr pane
    Jump { id: String },
    /// Reconcile tasks with herdr
    Sync {
        #[arg(long)]
        watch: bool,
    },
    /// herdr plugin event hook: run sync once, always exit 0
    Hook,
    /// Open or focus the PM pane
    Pm,
    /// Open the board TUI
    Board,
    /// Open the capture popup
    Capture {
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Open the artifact finder
    Artifacts,
    /// Manage the herdr plugin
    Plugin {
        #[command(subcommand)]
        cmd: PluginCmd,
    },
    /// Print the corral agent skill (SKILL.md), or install it for your agents
    Skill {
        #[command(subcommand)]
        cmd: Option<SkillCmd>,
    },
}

#[derive(Subcommand)]
pub enum SkillCmd {
    /// Write corral/SKILL.md into agent skill dirs (default: every harness found in $HOME)
    Install {
        /// Harness to install for; repeatable
        #[arg(long, value_parser = skill::HARNESS_NAMES)]
        harness: Vec<String>,
        /// Install into this skills directory instead
        #[arg(long, conflicts_with = "harness")]
        dir: Option<PathBuf>,
        /// Replace a SKILL.md that corral did not write
        #[arg(long)]
        force: bool,
    },
}

/// Run overrides: profile, model and effort (config profiles define the defaults).
#[derive(clap::Args)]
pub struct ProfileArgs {
    /// Profile (harness + model + effort preset) for this run
    #[arg(long, visible_alias = "agent")]
    pub profile: Option<String>,
    /// Model override for this run
    #[arg(long)]
    pub model: Option<String>,
    /// Effort level override for this run (valid levels depend on the harness)
    #[arg(long)]
    pub effort: Option<String>,
}

#[derive(Subcommand)]
pub enum ArtifactCmd {
    /// Snapshot a file as a task artifact
    Add {
        id: String,
        path: PathBuf,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        title: Option<String>,
    },
    /// List artifacts
    List {
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub enum PluginCmd {
    /// Write the herdr plugin manifest and link it
    Install {
        #[arg(long)]
        print: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    if let Err(err) = cmds::run(cli) {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
