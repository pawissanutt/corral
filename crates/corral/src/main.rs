mod artifact_cmd;
mod artifact_show;
mod automation_cmd;
mod cmds;
mod completion_cmd;
mod config_cmd;
mod dashboard_cmd;
mod digest_cmd;
mod doctor;
mod entry;
mod harness;
mod hooks_cmd;
mod idea_cmd;
mod import_cmd;
mod lessons_cmd;
mod list_cols;
mod log_cmd;
mod mode_cmd;
mod pins;
mod plan_cmd;
mod plugin;
mod pm_cmd;
mod pricing_cmd;
mod queue;
mod run_cmd;
mod search_cmd;
mod setup_cmd;
mod skill;
mod sprint_cmd;
mod stats;
mod unstick;
mod up;
mod wait_cmd;

use std::path::PathBuf;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use corral_core::{CommentAuthor, Origin, TaskKind, TaskSize};

fn parse_comment_author(value: &str) -> Result<CommentAuthor, String> {
    match value {
        "owner" => Ok(CommentAuthor::Owner),
        "pm" => Ok(CommentAuthor::Pm),
        _ => Err(format!("unknown author '{value}' (owner, pm)")),
    }
}

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
    pub cmd: Option<Cmd>,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Select Assist or explicitly confirm scoped Autopilot for this workspace
    Mode(mode_cmd::ModeArgs),
    /// Preview, explicitly confirm, or inspect finite operating authority
    Automation(automation_cmd::AutomationArgs),
    /// Review and apply owner-approved profile setup; JSON output by default
    Setup(setup_cmd::SetupArgs),
    #[command(hide = true)]
    RuntimeProbe {
        #[arg(long)]
        pid: Option<u32>,
    },
    /// Generate and pin a deterministic digest of recent outcomes and cost/time totals
    Digest(digest_cmd::DigestArgs),
    /// Revert a task's exact landed commits on base and reopen it in Backlog; never push
    Revert { id: String },
    /// Build and pin a sprint review from events, artifacts and recorded usage
    ReviewSprint(sprint_cmd::SprintArgs),
    /// Record a reasoned idea disposition; owner ideas require explicit consent
    Idea(idea_cmd::IdeaArgs),
    /// Inspect project lessons, distill history, pin rules or delete them
    Lessons(lessons_cmd::LessonsArgs),
    /// Submit, review and approve implementation plans independently of code review
    Plan {
        #[command(subcommand)]
        cmd: plan_cmd::PlanCmd,
    },
    /// Print the immutable landed identity embedded by a post-land install
    BuildIdentity,
    /// Inspect, reconcile and retry durable project post-land operations
    Hooks {
        #[command(subcommand)]
        cmd: hooks_cmd::HooksCmd,
    },
    /// Persist run progress, checkpoints, waits, decisions and resume receipts
    Run {
        #[command(subcommand)]
        cmd: run_cmd::RunCmd,
    },
    /// Authorize, pause, and inspect the coding dispatch queue
    Queue {
        #[command(subcommand)]
        cmd: queue::QueueCmd,
    },
    /// Wait for the first matching event, starting at EOF by default (timeout exits 124)
    Wait(wait_cmd::WaitArgs),
    /// Start or attach the board; also the default when no command is given
    Up {
        /// Named engine session (default: [herdr] session, else corral)
        #[arg(long)]
        session: Option<String>,
        /// Accept initialization with detected repositories without prompting
        #[arg(long)]
        yes: bool,
    },
    /// Check the terminal engine and workspace setup
    Doctor {
        /// Apply safe plugin, keybinding and skill fixes (never agents or panes)
        #[arg(long)]
        fix: bool,
        #[arg(long)]
        json: bool,
    },
    /// Create .corral/ in --root or the current directory
    Init,
    /// Inspect coverage of API-equivalent model prices
    Pricing {
        #[command(subcommand)]
        cmd: pricing_cmd::PricingCmd,
    },
    /// Import a Markdown plan as a tracker and dependent child tasks
    Import(import_cmd::ImportArgs),
    /// Outcome evidence grouped by profile (default), model, kind, project or task
    Stats {
        /// Artifact storage totals per task and workspace, including limit warnings
        #[arg(long, conflicts_with = "since")]
        storage: bool,
        #[arg(long, default_value = "profile", value_parser = ["profile", "model", "kind", "project", "task"])]
        by: String,
        /// Include runs dispatched in the last N days, hours or weeks (e.g. 30d)
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Create a task and print its id
    New {
        /// Who filed this task; PMs and agents should pass their own origin
        #[arg(long, default_value = "owner", value_parser = idea_cmd::parse_origin)]
        origin: Origin,
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
        /// Task category used by routing rules
        #[arg(long)]
        kind: Option<TaskKind>,
        /// Estimated task size used by routing rules
        #[arg(long)]
        size: Option<TaskSize>,
        /// Route the idea or task through design planning before implementation
        #[arg(long, visible_alias = "plan-required")]
        design: bool,
        /// Task labels; owner-only excludes Ideas from automatic promotion
        #[arg(long = "label", value_delimiter = ',')]
        labels: Vec<String>,
    },
    /// List tasks (done/dropped hidden unless --all; --done lists finished tasks, newest first)
    List {
        /// Search all statuses in relevance order; explicit filters still apply
        #[arg(long)]
        query: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        all: bool,
        /// Only Done tasks, newest finished first, with finish date and the sha their approval landed
        #[arg(long, conflicts_with_all = ["status", "all"])]
        done: bool,
        /// With --done: only tasks finished in the last N hours, days or weeks (e.g. 7d)
        #[arg(long, requires = "done")]
        since: Option<String>,
        #[arg(long)]
        json: bool,
        /// Print only these comma-separated columns: id, status, project, title or any board card field (cost, tokens, ...)
        #[arg(long, value_delimiter = ',', conflicts_with_all = ["done", "json"])]
        columns: Vec<String>,
    },
    /// Search tasks or indexed artifact, specification and review content
    Search(search_cmd::SearchArgs),
    /// Show one task's lifecycle timeline, oldest first
    Log(log_cmd::LogArgs),
    /// Show one task
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Append a comment to a task, optionally sending it to the agent
    Comment {
        id: String,
        text: String,
        #[arg(long, default_value = "owner", value_parser = parse_comment_author)]
        author: CommentAuthor,
        #[arg(long)]
        send: bool,
    },
    /// Move a task to another status
    Move {
        id: String,
        status: String,
        #[arg(long)]
        force: bool,
        /// Record explicit owner approval for dropping or deferring this task
        #[arg(long)]
        owner_ok: bool,
        #[arg(long)]
        reason: Option<String>,
        /// Who made this move, independent of who filed the task
        #[arg(long, default_value = "owner", value_parser = ["owner", "pm", "agent"])]
        author: String,
    },
    /// Open the task file in $EDITOR
    Edit {
        id: String,
        #[arg(long)]
        owner_ok: bool,
    },
    /// Explain the implementer and reviewer mapping without starting a run
    Route {
        id: String,
        /// Override this task's reviewer with a profile, pm, cross-vendor, or none
        #[arg(long, value_name = "NAME", conflicts_with = "clear_reviewer")]
        set_reviewer: Option<String>,
        /// Remove this task's reviewer override
        #[arg(long)]
        clear_reviewer: bool,
        #[command(flatten)]
        profile: ProfileArgs,
        #[arg(long)]
        json: bool,
    },
    /// Start an agent run for a task
    Dispatch {
        id: String,
        /// Run a read-only planning session without a Git candidate
        #[arg(long)]
        plan: bool,
        #[command(flatten)]
        profile: ProfileArgs,
        #[arg(long)]
        headless: bool,
        #[arg(long)]
        base: Option<String>,
        /// Move to Ready without launching; sync admits when capacity permits
        #[arg(long)]
        queue: bool,
        /// Override machine resource pressure (not slot or dependency limits)
        #[arg(long)]
        force: bool,
        /// Explain routing without creating a worktree or launching an agent
        #[arg(long)]
        explain: bool,
        #[arg(long, requires = "explain")]
        json: bool,
    },
    /// Send a reply to the task's agent
    Reply {
        id: String,
        #[arg(required_unless_present_any = ["resend", "mark_delivered"], conflicts_with_all = ["resend", "mark_delivered"], num_args = 1..)]
        text: Vec<String>,
        /// Retry the stored reply that was not delivered
        #[arg(long)]
        resend: bool,
        /// Acknowledge an externally delivered pending live-run reply without sending it
        #[arg(long, conflicts_with_all = ["text", "resend"], requires_all = ["reason", "author"])]
        mark_delivered: bool,
        /// Evidence explaining how delivery was confirmed
        #[arg(long, requires = "mark_delivered")]
        reason: Option<String>,
        /// Operator recording the delivery acknowledgement
        #[arg(long, requires = "mark_delivered", value_parser = parse_comment_author)]
        author: Option<CommentAuthor>,
    },
    /// Terminate a stuck foreign foreground process after confirmation, then retry the pending reply
    Unstick { id: String },
    /// Mark the run finished and move the task to Review
    Done {
        id: String,
        #[arg(long, required_unless_present = "check")]
        summary: Option<String>,
        /// Cumulative outcome: original goal, achieved scope, and remaining limitations
        #[arg(long, conflicts_with = "check")]
        result: Option<String>,
        /// Read-only completion audit; no commits or status changes
        #[arg(long)]
        check: bool,
        #[arg(long, requires = "check")]
        json: bool,
    },
    /// Manage explicit owner-reviewed criterion manifests
    Criteria {
        #[command(subcommand)]
        cmd: completion_cmd::CriteriaCmd,
    },
    /// Record and inspect criterion-linked worker assertions
    Evidence {
        #[command(subcommand)]
        cmd: completion_cmd::EvidenceCmd,
    },
    /// Run the review gate commands in the current run's worktree (in a herdr pane)
    Gate {
        id: String,
        /// Block until the gate finishes and print its result
        #[arg(long)]
        wait: bool,
        /// Override machine resource pressure (not slot limits)
        #[arg(long)]
        force: bool,
    },
    /// Start an independent review of the current run (a reviewer profile in its own pane, or the PM)
    ReviewRun {
        id: String,
        /// "pm", "cross-vendor" or a profile; default: the configured `[review] reviewer`
        #[arg(long)]
        reviewer: Option<String>,
    },
    /// Record a review verdict on the current run
    Verdict {
        id: String,
        #[arg(value_parser = ["approve", "changes", "escalate"])]
        verdict: String,
        /// Review notes (markdown)
        #[arg(
            long,
            required_unless_present = "notes_file",
            conflicts_with = "notes_file"
        )]
        notes: Option<String>,
        /// Read the review notes from this file
        #[arg(long)]
        notes_file: Option<PathBuf>,
        /// Bind this verdict to the reviewer's assigned gate attempt
        #[arg(long)]
        attempt: Option<String>,
    },
    /// Configure recurring living-dashboard report tasks
    Dashboard {
        #[command(subcommand)]
        cmd: dashboard_cmd::DashboardCmd,
    },
    /// Manage artifacts
    Artifact {
        #[command(subcommand)]
        cmd: ArtifactCmd,
    },
    /// Pin a task (T-042) or artifact (T-042:R1:<file>) so it is one step away
    Pin {
        #[arg(value_name = "REF")]
        target: String,
        #[arg(long)]
        label: Option<String>,
        /// Follow the newest artifact of the task with the same title or source path
        #[arg(long)]
        follow_latest: bool,
    },
    /// Remove a pin (by ref or pin number)
    Unpin {
        #[arg(value_name = "REF|N")]
        target: String,
    },
    /// List pins in order
    Pins {
        #[arg(long)]
        json: bool,
    },
    /// Open a pin (number or ref) or any task/artifact ref with its viewer: [viewers], else $CORRAL_OPENER, else
    /// terminal-browser for HTML inside herdr, else the system opener (WSL-aware)
    Open {
        #[arg(value_name = "REF|N")]
        target: String,
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
        /// Keep the merged task branch after removing its worktree
        #[arg(long)]
        keep_branch: bool,
    },
    /// Request changes on a task in Review
    Changes {
        id: String,
        /// Start a fresh run on this profile's escalate_to instead of reusing the live session
        #[arg(long)]
        escalate: bool,
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
    },
    /// Start a fresh run for a task
    Retry {
        id: String,
        /// Guard retry with the source run ID; requires its current checkpoint
        #[arg(long)]
        run: Option<String>,
        #[command(flatten)]
        profile: ProfileArgs,
    },
    /// Discard the current run and drop the task
    Discard {
        id: String,
        /// Retain the discarded branch after removing its worktree
        #[arg(long)]
        keep_branch: bool,
        #[arg(long)]
        owner_ok: bool,
        #[arg(long)]
        reason: Option<String>,
        /// Who made this discard decision, independent of who filed the task
        #[arg(long, default_value = "owner", value_parser = ["owner", "pm", "agent"])]
        author: String,
    },
    /// Focus the task's herdr pane
    Jump { id: String },
    /// Reconcile tasks with herdr and advance the review autopilot (--watch: keep syncing every 2 s, headless)
    Sync {
        #[arg(long)]
        watch: bool,
    },
    /// Compatibility alias: shared status, workspace pause, or guidance for scoped activation
    Autopilot {
        #[arg(value_parser = ["on", "off", "status"], default_value = "status")]
        action: String,
        #[arg(long)]
        json: bool,
    },
    /// herdr plugin event hook: run sync once, always exit 0
    Hook,
    /// Bind legacy pane ownership using verified launch or plugin-registration evidence
    BindSession {
        /// Absolute socket path of the original launch server (not the current preference)
        #[arg(long)]
        socket: PathBuf,
        /// File recording evidence that all legacy panes were launched on this socket
        #[arg(long)]
        launch_record: PathBuf,
    },
    /// Open the PM pane, or inspect the next PM decision
    Pm {
        #[command(subcommand)]
        cmd: Option<pm_cmd::PmCmd>,
    },
    /// Go back to this workspace's board pane, opening one only if none is running (the back-to-board key)
    FocusBoard,
    /// Open the board TUI
    Board {
        /// Do not capture the mouse (also: CORRAL_MOUSE=0, or `[ui] mouse = false`)
        #[arg(long)]
        no_mouse: bool,
    },
    /// Open the capture popup
    Capture {
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Do not capture the mouse (also: CORRAL_MOUSE=0, or `[ui] mouse = false`)
        #[arg(long)]
        no_mouse: bool,
    },
    /// Open the artifact finder
    Artifacts {
        /// Do not capture the mouse (also: CORRAL_MOUSE=0, or `[ui] mouse = false`)
        #[arg(long)]
        no_mouse: bool,
    },
    /// Open the settings screen
    Settings {
        /// Do not capture the mouse (also: CORRAL_MOUSE=0, or `[ui] mouse = false`)
        #[arg(long)]
        no_mouse: bool,
    },
    /// Installed agent harnesses, their versions and models
    Harness {
        #[command(subcommand)]
        cmd: HarnessCmd,
    },
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
    /// Read, change and check layered user and workspace configuration
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
}

#[derive(Subcommand)]
pub enum ConfigCmd {
    /// Show the latest recorded semantic config change, or all changes since a timestamp
    Diff {
        /// Include changes at or after this RFC3339 timestamp
        #[arg(long)]
        since: Option<jiff::Timestamp>,
        /// Print an array of the original event records
        #[arg(long)]
        json: bool,
    },
    /// Show recorded config changes and invalid-config diagnostics in log order
    History {
        /// Print an array of the original event records
        #[arg(long)]
        json: bool,
    },
    /// Print a setting by dotted key (e.g. limits.running, projects.app.path); defaults included
    Get {
        key: String,
        /// Include the effective value's layer and file (or built-in default)
        #[arg(long)]
        source: bool,
    },
    /// Change a setting in place, keeping comments; refused when the result would be invalid
    Set {
        key: String,
        /// TOML value (3, true, ["a", "b"], "quoted"); anything else is taken as a string
        value: String,
        /// Save personal preferences in $XDG_CONFIG_HOME/corral/config.toml
        #[arg(long, conflicts_with = "workspace")]
        user: bool,
        /// Save in .corral/corral.toml (the default)
        #[arg(long)]
        workspace: bool,
    },
    /// Check both config layers and list problems with their file (non-zero when invalid)
    #[command(visible_alias = "check")]
    Validate,
    /// Print the path of corral.toml
    Path,
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
    /// Override an artifact's inferred importance tier
    SetTier {
        reference: String,
        #[arg(value_parser = ["keep", "normal", "transient"])]
        tier: String,
    },
    /// Deduplicate artifacts and apply the configured retention policy
    Gc {
        /// Preview actions and bytes saved without changing any files
        #[arg(long, conflicts_with = "yes")]
        dry_run: bool,
        /// Apply the policy without prompting; required for a real run
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    /// Snapshot a file as a task artifact
    Add {
        id: String,
        path: PathBuf,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long)]
        title: Option<String>,
    },
    /// List artifacts, most important first (keep, normal, transient)
    List {
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        json: bool,
        /// Only this importance tier: keep, normal or transient
        #[arg(long)]
        tier: Option<String>,
    },
    /// Serve an artifact (or the HTML/Markdown index) from the loopback live server and open it in a browser
    Serve {
        /// Artifact ref: T-042:R1:<file>, pin number or snapshot path; omit for the index
        reference: Option<String>,
        /// Serve this task's document index instead of one artifact
        #[arg(long, conflicts_with = "reference")]
        task: Option<String>,
        /// Print the URL without opening a browser
        #[arg(long)]
        no_open: bool,
        /// Stop the running server
        #[arg(long, conflicts_with_all = ["reference", "task", "no_open"])]
        stop: bool,
        /// Run the server in the foreground (started by corral itself)
        #[arg(long, hide = true, conflicts_with_all = ["reference", "task", "no_open", "stop"])]
        daemon: bool,
        /// Seconds without requests or open pages before the server exits
        #[arg(long, hide = true, default_value_t = corral_ops::DEFAULT_IDLE.as_secs())]
        idle_secs: u64,
    },
    /// Print an artifact formatted for reading (Markdown, JSON, TOML, logs, diffs, CSV)
    Show {
        /// Artifact ref: T-042:R1:<file> (see corral artifact list)
        reference: String,
        /// Print the decoded stored content unformatted
        #[arg(long)]
        raw: bool,
        /// Print an inclusive decoded source-line slice, e.g. 10-20
        #[arg(long)]
        lines: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum HarnessCmd {
    /// Show each configured harness: install state, version, model count, effort levels (cached)
    List {
        #[arg(long)]
        json: bool,
        /// Re-detect instead of reading .corral/state/harnesses.json
        #[arg(long)]
        refresh: bool,
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
    let matches = Cli::command().try_get_matches().unwrap_or_else(|err| {
        // Preserve machine-readable failures even when setup's arguments do not parse.
        let mut args = std::env::args_os().skip(1);
        let setup = loop {
            match args.next() {
                Some(arg) if arg == "--root" => {
                    args.next();
                }
                Some(arg) if arg.to_string_lossy().starts_with("--root=") => {}
                Some(arg) => break arg == "setup",
                None => break false,
            }
        };
        if setup && err.use_stderr() {
            eprintln!(
                "{}",
                serde_json::json!({"command":"setup","error":err.to_string()})
            );
            std::process::exit(err.exit_code());
        }
        err.exit()
    });
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|err| err.exit());
    let setup_json = match &cli.cmd {
        Some(Cmd::Setup(args)) => Some(args.cmd.name()),
        _ => None,
    };
    let run_json = match &cli.cmd {
        Some(Cmd::Run { cmd }) => cmd
            .args()
            .filter(|args| args.json)
            .map(|args| (cmd.name(), args.id.clone())),
        _ => None,
    };
    if let Err(err) = cmds::run(cli, &matches) {
        if err.is::<wait_cmd::WaitTimeout>() {
            std::process::exit(124);
        }
        if let Some(command) = setup_json {
            eprintln!(
                "{}",
                serde_json::json!({
                    "command": format!("setup {command}"),
                    "error": format!("{err:#}")
                })
            );
        } else if let Some((command, task)) = run_json {
            eprintln!(
                "{}",
                serde_json::json!({
                    "command": format!("run {command}"),
                    "task": task,
                    "error": format!("{err:#}"),
                    "hint": "Read current identity with corral run progress <task-id> --json; reconcile stale input before resubmitting."
                })
            );
        } else {
            eprintln!("error: {}", corral_herdr::friendly_error(&err));
        }
        std::process::exit(1);
    }
}
