//! `corral pi`: start the corral-pi host from nothing. In a workspace on a terminal it opens the
//! launcher, which asks whether to open a build session, an assistant, the PM or a stopped session
//! to resume; `--build`, `--assistant`, `--pm` and `--resume A-<n>`/`B-<id>` open one directly, and
//! a plain `corral pi` without a terminal opens an assistant. Outside a workspace only `--build`
//! (and `--resume B-<id>`) start the corral-pi host; plain `corral pi` there, or `--bare`, starts
//! plain Pi. Handler bodies live in `corral_ops::agent::pi` and `corral_ops::agent::pi_build`.
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::Args;
use corral_core::{Config, ProfileChoice, Store};
use corral_ops::Ctx;
use corral_ops::agent::credentials;
use corral_ops::agent::hooks::pm::Overrides;
use corral_ops::agent::hooks::preflight;
use corral_ops::agent::launch::host_provider;
use corral_ops::agent::pi::{self, BareArgs};
use corral_ops::agent::pi_build::{self, BuildArgs, BuildSessionSummary};
use corral_tui::launcher::{self, LauncherContext, LauncherEntry, Login, ProviderLogin, Section};

use crate::entry::Entry;

#[derive(Args)]
pub struct PiArgs {
    /// Plain unbound Pi in the current directory: no store, binding or corral tools
    #[arg(long, conflicts_with_all = ["pm", "resume", "over_cap"])]
    bare: bool,
    /// Open the workspace PM in this pane, or take over a dead one
    #[arg(long, conflicts_with_all = ["resume", "over_cap"])]
    pm: bool,
    /// Open an assistant directly, without the launcher
    #[arg(long, conflicts_with_all = ["bare", "pm", "resume"])]
    assistant: bool,
    /// Open an untethered build session: parallel agents in one worktree, nothing reaches the board until you choose an exit
    #[arg(long, conflicts_with_all = ["bare", "pm", "assistant", "task", "over_cap", "continue_session"])]
    build: bool,
    /// Write in the current checkout and branch instead of a new build worktree
    #[arg(long, requires = "build", conflicts_with = "resume")]
    here: bool,
    /// Name the build session; a new worktree is `.worktrees/build-<NAME>` on branch `build/<NAME>`
    #[arg(
        long,
        value_name = "NAME",
        requires = "build",
        conflicts_with = "resume"
    )]
    name: Option<String>,
    /// Continue the previous PM conversation instead of starting fresh
    #[arg(short = 'c', long = "continue", requires = "pm",
        conflicts_with_all = ["bare", "resume", "over_cap", "task"])]
    continue_session: bool,
    /// Resume stopped assistant session A-<n> (fencing its previous generation) or build session B-<id> in this pane (inside an assistant, /resume picks one)
    #[arg(long, value_name = "ID", conflicts_with = "over_cap")]
    resume: Option<String>,
    /// Open an assistant even when assistant.max_live assistants are already live
    #[arg(long)]
    over_cap: bool,
    /// Preload bounded read-only context for this task in a new assistant
    #[arg(long, value_name = "TASK-ID", conflicts_with_all = ["bare", "pm", "resume"])]
    task: Option<String>,
    /// Model to run instead of the profile's (bare: Pi's default for the provider)
    #[arg(long)]
    model: Option<String>,
    /// Provider whose `corral agent login` runs the session (bare default: openai-codex)
    #[arg(long, value_parser = ["openai-codex", "anthropic"])]
    provider: Option<String>,
    /// Reasoning effort instead of the profile's
    #[arg(long)]
    effort: Option<String>,
}

/// What one `corral pi` invocation opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    /// Plain Pi; `notice` when it was chosen because no workspace was found.
    Bare {
        notice: bool,
    },
    /// Ask on the terminal what to open.
    Launcher,
    Pm,
    Assistant {
        over_cap: bool,
    },
    Resume(String),
    /// An untethered build session: a new one, or `resume` (`B-<id>`) continues a recorded one.
    Build {
        here: bool,
        name: Option<String>,
        resume: Option<String>,
    },
}

pub const BARE_NOTICE: &str =
    "corral pi: no corral workspace here; starting bare Pi (no store, binding or corral tools)";

/// The invocation split: a build session needs only a Git repository (checked when it starts);
/// otherwise outside a workspace only plain Pi can start, and a plain `corral pi` on a terminal
/// (stdin and stdout both) asks with the launcher.
pub fn invocation(args: &PiArgs, in_workspace: bool, terminal: bool) -> Result<Invocation> {
    if args.bare {
        return Ok(Invocation::Bare { notice: false });
    }
    let resumes_build = args.resume.as_deref().is_some_and(is_build_session);
    if args.build || resumes_build {
        if let Some(id) = args.resume.as_deref().filter(|_| !resumes_build) {
            bail!(
                "corral pi refused: usage_error: --build resumes only build sessions (B-<id>); \
                 resume {id} with `corral pi --resume {id}`"
            );
        }
        return Ok(Invocation::Build {
            here: args.here,
            name: args.name.clone(),
            resume: args.resume.clone(),
        });
    }
    if !in_workspace {
        if args.pm
            || args.assistant
            || args.resume.is_some()
            || args.over_cap
            || args.task.is_some()
        {
            bail!(
                "corral pi refused: not_in_workspace: --pm, --assistant, --resume A-<n>, --task \
                 and --over-cap need a corral workspace; run them inside one, or `corral pi \
                 --build` or `corral pi --bare` here"
            );
        }
        return Ok(Invocation::Bare { notice: true });
    }
    let chosen = args.pm
        || args.assistant
        || args.resume.is_some()
        || args.task.is_some()
        || args.over_cap
        || args.continue_session;
    if terminal && !chosen {
        return Ok(Invocation::Launcher);
    }
    Ok(match (&args.resume, args.pm) {
        (Some(id), _) => Invocation::Resume(id.clone()),
        (None, true) => Invocation::Pm,
        (None, false) => Invocation::Assistant {
            over_cap: args.over_cap,
        },
    })
}

/// Build session ids are `B-` plus the host's lowercase base36; assistants are `A-<n>`.
fn is_build_session(id: &str) -> bool {
    id.starts_with("B-")
}

pub fn run(start: &Path, args: PiArgs, entry: &Entry) -> Result<()> {
    let terminal = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let invocation = invocation(&args, Store::find_root(start).is_ok(), terminal)?;
    let mut overrides = Overrides {
        model: args.model,
        provider: args.provider,
        effort: args.effort,
        continue_session: args.continue_session,
    };
    match invocation {
        Invocation::Bare { notice } => {
            if notice {
                eprintln!("{BARE_NOTICE}");
            }
            pi::bare(&BareArgs {
                model: overrides.model,
                provider: overrides.provider,
                effort: overrides.effort,
            })
        }
        Invocation::Build { here, name, resume } => start_build(overrides, here, name, resume),
        Invocation::Launcher => {
            let ctx = entry.open(start)?;
            let top = git_top_level(start);
            let entries = launcher_entries(&ctx, top.as_deref())?;
            let context = launcher_context(start, &ctx, top.as_deref(), &overrides);
            let Some((id, option)) = launcher::pick(&entries, &context)? else {
                return Ok(());
            };
            let (next, continue_session) = chosen(&id, option)?;
            overrides.continue_session = continue_session;
            if let Invocation::Build { here, name, resume } = next {
                // The build host never uses the store: release it before the host replaces us.
                drop(ctx);
                return start_build(overrides, here, name, resume);
            }
            open(start, &ctx, next, &overrides, None)
        }
        direct => open(
            start,
            &entry.open(start)?,
            direct,
            &overrides,
            args.task.as_deref(),
        ),
    }
}

/// Starts the untethered build host; `corral_ops::agent::pi_build` refuses outside a Git repository.
fn start_build(
    overrides: Overrides,
    here: bool,
    name: Option<String>,
    resume: Option<String>,
) -> Result<()> {
    pi_build::build(&BuildArgs {
        model: overrides.model,
        provider: overrides.provider,
        effort: overrides.effort,
        here,
        name,
        resume,
    })
}

/// Opens a bound session: the PM, a new assistant (with optional task focus) or a resumed one.
fn open(
    start: &Path,
    ctx: &Ctx,
    invocation: Invocation,
    overrides: &Overrides,
    task: Option<&str>,
) -> Result<()> {
    match invocation {
        Invocation::Pm => pi::bound(start, ctx, overrides),
        Invocation::Assistant { over_cap } => {
            pi::assistant_focused(start, ctx, overrides, over_cap, task)
        }
        Invocation::Resume(id) => pi::resume_assistant(ctx, &id, overrides),
        other @ (Invocation::Bare { .. } | Invocation::Launcher | Invocation::Build { .. }) => {
            bail!("corral pi: {other:?} does not open a bound session")
        }
    }
}

const BUILD_ROW: &str = "build";
const ASSISTANT_ROW: &str = "assistant";
const PM_ROW: &str = "pm";
/// The Build row's option that writes in the current checkout (`--here`).
const BUILD_HERE: usize = 1;
/// Stopped sessions the launcher lists, newest first.
const RESUME_ROWS: usize = 8;

/// The Build row: a new worktree by default, or the current checkout ([`BUILD_HERE`]).
fn build_entry() -> LauncherEntry {
    LauncherEntry {
        id: BUILD_ROW.into(),
        section: Section::Start,
        key: Some('b'),
        name: "Build".into(),
        detail: "untethered · parallel agents · nothing reaches the board until you ship".into(),
        options: vec![
            "new worktree  .worktrees/build-<name>".into(),
            "here  the current checkout and branch".into(),
        ],
    }
}

/// A recorded build session's Resume row: its name, project (or directory), age and, once it has
/// exited, how.
fn build_resume_entry(session: &BuildSessionSummary, now: jiff::Timestamp) -> LauncherEntry {
    let place = match (&session.project, session.repo_root.file_name()) {
        (Some(project), _) => project.clone(),
        (None, Some(dir)) => dir.to_string_lossy().into_owned(),
        (None, None) => session.repo_root.display().to_string(),
    };
    let mut detail = format!("{} · {place}", session.name);
    if let Ok(updated) = session.updated.parse::<jiff::Timestamp>() {
        let age = corral_core::age_text(now.duration_since(updated).as_secs());
        detail.push_str(&format!(" · {age} ago"));
    }
    let exited = match session.exited.as_deref() {
        Some("keep") => Some("kept"),
        Some("ship") => Some("shipped"),
        Some("land") => Some("landed"),
        _ => None,
    };
    if let Some(word) = exited {
        detail.push_str(&format!(" · {word}"));
    }
    LauncherEntry {
        id: session.id.clone(),
        section: Section::Resume,
        key: None,
        name: "build".into(),
        detail,
        options: Vec::new(),
    }
}

/// The Git top-level of `start`, resolved; `None` outside a repository.
fn git_top_level(start: &Path) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(start)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let top = String::from_utf8(output.stdout).ok()?;
    PathBuf::from(top.trim_end_matches('\n'))
        .canonicalize()
        .ok()
}

/// The launcher's rows: start a build session, an assistant or the PM, or resume one of the newest
/// stopped sessions: assistants `corral pi --resume` accepts (open, with no running host) and the
/// build sessions recorded for a repository inside this workspace or `top`, the current one.
fn launcher_entries(ctx: &Ctx, top: Option<&Path>) -> Result<Vec<LauncherEntry>> {
    let mut entries = vec![
        build_entry(),
        LauncherEntry {
            id: ASSISTANT_ROW.into(),
            section: Section::Start,
            key: Some('a'),
            name: "Assistant".into(),
            detail: "board-aware · scratch worktrees · owner actions on your instruction".into(),
            options: Vec::new(),
        },
        LauncherEntry {
            id: PM_ROW.into(),
            section: Section::Start,
            key: Some('p'),
            name: "PM".into(),
            detail: "workspace coordinator · continues the previous conversation".into(),
            options: Vec::new(),
        },
    ];
    let mut stopped: Vec<_> = corral_ops::assistant_stats(&ctx.store, None)?
        .into_iter()
        .filter(|row| row.ended.is_none() && !row.live)
        .collect();
    stopped.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
    let now = jiff::Timestamp::now();
    let mut resume = Vec::new();
    for row in stopped.into_iter().take(RESUME_ROWS) {
        let project = ctx.store.get_assistant(&row.session)?.project;
        let title = if !row.summary.is_empty() {
            row.summary
        } else if row.focus != "workspace" {
            format!("focus {}", row.focus)
        } else {
            "no summary recorded".into()
        };
        let age = corral_core::age_text(now.duration_since(row.last_activity).as_secs());
        let project = project.as_deref().unwrap_or("workspace");
        resume.push((
            row.last_activity,
            LauncherEntry {
                id: row.session.clone(),
                section: Section::Resume,
                key: None,
                name: row.session,
                detail: format!("{title} · {project} · {age} ago"),
                options: Vec::new(),
            },
        ));
    }
    let workspace = ctx.store.root();
    for session in pi_build::list_sessions(&corral_ops::agent::install::data_root()?) {
        let Ok(updated) = session.updated.parse::<jiff::Timestamp>() else {
            continue;
        };
        let repo = session
            .repo_root
            .canonicalize()
            .unwrap_or_else(|_| session.repo_root.clone());
        if repo.starts_with(workspace) || top == Some(repo.as_path()) {
            resume.push((updated, build_resume_entry(&session, now)));
        }
    }
    resume.sort_by(|a, b| b.0.cmp(&a.0));
    entries.extend(resume.into_iter().take(RESUME_ROWS).map(|(_, entry)| entry));
    Ok(entries)
}

/// What the launcher's option block shows about the build session it would open: the project a
/// build resolves for this checkout (the deepest project containing `top`, the repository root,
/// else `start`) and the providers its agents can use, each with what the launch's credential check
/// says of its login. File reads only: no subprocess, no network.
fn launcher_context(
    start: &Path,
    ctx: &Ctx,
    top: Option<&Path>,
    overrides: &Overrides,
) -> LauncherContext {
    let project = corral_ops::infer_project(ctx, top.unwrap_or(start));
    let config = ctx
        .store
        .config_layers()
        .and_then(|layers| layers.config())
        .unwrap_or_else(|_| ctx.store.config.clone());
    let build = config
        .build
        .profiles
        .values()
        .filter_map(|profile| profile.model.split_once('/'))
        .map(|(provider, _)| provider.to_owned());
    let mut ids: Vec<String> = Vec::new();
    for id in assistant_provider(&config, overrides)
        .into_iter()
        .chain(build)
    {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let data_root = corral_ops::agent::install::data_root().ok();
    let providers = ids
        .into_iter()
        .map(|id| ProviderLogin {
            login: provider_login(data_root.as_deref(), &id),
            id,
        })
        .collect();
    LauncherContext { project, providers }
}

/// The provider the assistant profile, which a build session's parent runs, uses with this launch's
/// `--provider` and `--model`; `None` when it names no model or no known provider.
fn assistant_provider(config: &Config, overrides: &Overrides) -> Option<String> {
    if let Some(id) = &overrides.provider {
        return Some(id.clone());
    }
    let name = config.assistant.agent.as_ref().unwrap_or(&config.pm.agent);
    let choice = ProfileChoice {
        profile: Some(name.clone()),
        ..ProfileChoice::default()
    };
    let resolved = config.resolve_profile(&choice, None, None).ok()?;
    let vendor = overrides
        .model
        .as_deref()
        .and_then(corral_core::infer_vendor)
        .or(resolved.vendor);
    let model = overrides.model.clone().or(resolved.model)?;
    host_provider(vendor.as_deref(), &model)
}

/// A provider's login as the build launch's credential check sees it from file metadata; unknown
/// when there is no data root or the provider has no corral-owned login.
fn provider_login(data_root: Option<&Path>, id: &str) -> Login {
    let (Some(root), Some(provider)) = (data_root, credentials::provider(id)) else {
        return Login::Unknown;
    };
    match preflight::credential(root, Some(provider.vendor)) {
        Ok(()) => Login::Present,
        Err(_) => Login::Missing,
    }
}

/// What the launcher's row (with its chosen option) stands for, and whether it continues the
/// previous PM conversation.
fn chosen(id: &str, option: usize) -> Result<(Invocation, bool)> {
    Ok(match id {
        BUILD_ROW => (
            Invocation::Build {
                here: option == BUILD_HERE,
                name: None,
                resume: None,
            },
            false,
        ),
        ASSISTANT_ROW => (Invocation::Assistant { over_cap: false }, false),
        PM_ROW => (Invocation::Pm, true),
        _ if is_build_session(id) => (
            Invocation::Build {
                here: false,
                name: None,
                resume: Some(id.to_owned()),
            },
            false,
        ),
        _ if id.starts_with("A-") => (Invocation::Resume(id.to_owned()), false),
        _ => bail!("corral pi: the launcher returned an unknown row {id}"),
    })
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        pi: PiArgs,
    }

    fn parse(argv: &[&str]) -> Result<PiArgs, clap::Error> {
        Cli::try_parse_from(std::iter::once("pi").chain(argv.iter().copied())).map(|cli| cli.pi)
    }

    fn build(here: bool, name: Option<&str>, resume: Option<&str>) -> Invocation {
        Invocation::Build {
            here,
            name: name.map(Into::into),
            resume: resume.map(Into::into),
        }
    }

    #[test]
    fn the_invocation_split_follows_flags_workspace_and_terminal() {
        use Invocation::*;
        // (argv, in a workspace, on a terminal, what opens or the refusal code)
        type Case<'a> = (&'a [&'a str], bool, bool, Result<Invocation, &'a str>);
        let cases: &[Case] = &[
            (&[], true, true, Ok(Launcher)),
            (
                &["--model", "m", "--effort", "high"],
                true,
                true,
                Ok(Launcher),
            ),
            (&[], true, false, Ok(Assistant { over_cap: false })),
            (
                &["--assistant"],
                true,
                true,
                Ok(Assistant { over_cap: false }),
            ),
            (
                &["--assistant"],
                true,
                false,
                Ok(Assistant { over_cap: false }),
            ),
            (
                &["--assistant", "--over-cap"],
                true,
                true,
                Ok(Assistant { over_cap: true }),
            ),
            (
                &["--over-cap"],
                true,
                true,
                Ok(Assistant { over_cap: true }),
            ),
            (
                &["--over-cap"],
                true,
                false,
                Ok(Assistant { over_cap: true }),
            ),
            (&["--pm"], true, true, Ok(Pm)),
            (&["--pm", "-c"], true, true, Ok(Pm)),
            (&["--pm", "--continue"], true, false, Ok(Pm)),
            (
                &["--task", "T-42"],
                true,
                true,
                Ok(Assistant { over_cap: false }),
            ),
            (&["--task", "T-42"], false, true, Err("not_in_workspace")),
            (&["--resume", "A-1"], true, true, Ok(Resume("A-1".into()))),
            (&["--resume", "A-3"], true, false, Ok(Resume("A-3".into()))),
            (&["--build"], true, true, Ok(build(false, None, None))),
            (&["--build"], true, false, Ok(build(false, None, None))),
            (
                &["--build", "--here"],
                true,
                true,
                Ok(build(true, None, None)),
            ),
            (
                &["--build", "--name", "x"],
                true,
                true,
                Ok(build(false, Some("x"), None)),
            ),
            (
                &["--build", "--model", "m"],
                true,
                true,
                Ok(build(false, None, None)),
            ),
            (&["--build"], false, true, Ok(build(false, None, None))),
            (
                &["--build", "--here", "--name", "x"],
                false,
                false,
                Ok(build(true, Some("x"), None)),
            ),
            (
                &["--resume", "B-abc123"],
                true,
                true,
                Ok(build(false, None, Some("B-abc123"))),
            ),
            (
                &["--resume", "B-abc123"],
                false,
                true,
                Ok(build(false, None, Some("B-abc123"))),
            ),
            (
                &["--build", "--resume", "B-abc123"],
                true,
                true,
                Ok(build(false, None, Some("B-abc123"))),
            ),
            (
                &["--build", "--resume", "A-3"],
                true,
                true,
                Err("usage_error"),
            ),
            (&["--bare"], true, true, Ok(Bare { notice: false })),
            (&["--bare"], false, false, Ok(Bare { notice: false })),
            (&[], false, true, Ok(Bare { notice: true })),
            (&[], false, false, Ok(Bare { notice: true })),
            (&["--model", "m"], false, true, Ok(Bare { notice: true })),
            (&["--pm"], false, true, Err("not_in_workspace")),
            (&["--assistant"], false, true, Err("not_in_workspace")),
            (&["--resume", "A-1"], false, true, Err("not_in_workspace")),
            (&["--over-cap"], false, true, Err("not_in_workspace")),
        ];
        for (argv, in_workspace, terminal, expected) in cases {
            let args = parse(argv).unwrap();
            let got = invocation(&args, *in_workspace, *terminal);
            match expected {
                Ok(expected) => assert_eq!(&got.unwrap(), expected, "{argv:?} {terminal}"),
                Err(code) => {
                    let error = got.unwrap_err().to_string();
                    assert!(error.contains(code), "{argv:?}: {error}");
                }
            }
        }
    }

    #[test]
    fn launcher_rows_open_what_their_flags_open() {
        // Build's first option is a new worktree (`--build`), its second the checkout (`--here`).
        let options = build_entry().options;
        assert_eq!(options.len(), 2);
        assert!(options[BUILD_HERE].starts_with("here"));
        assert_eq!(
            chosen(BUILD_ROW, 0).unwrap(),
            (build(false, None, None), false)
        );
        assert_eq!(
            chosen(BUILD_ROW, BUILD_HERE).unwrap(),
            (build(true, None, None), false)
        );
        assert_eq!(
            chosen(ASSISTANT_ROW, 0).unwrap(),
            (Invocation::Assistant { over_cap: false }, false)
        );
        // The PM row says it continues the previous conversation: `--pm -c`.
        assert_eq!(chosen(PM_ROW, 0).unwrap(), (Invocation::Pm, true));
        assert_eq!(
            chosen("A-7", 0).unwrap(),
            (Invocation::Resume("A-7".into()), false)
        );
        // A build Resume row is `--resume B-<id>`.
        let session = BuildSessionSummary {
            id: "B-abc123".into(),
            name: "checkout-copy".into(),
            repo_root: "/work/mitiplay".into(),
            worktree: "/work/mitiplay/.worktrees/build-checkout-copy".into(),
            here: false,
            project: None,
            updated: "2026-11-12T10:00:00Z".into(),
            exited: Some("ship".into()),
        };
        let now: jiff::Timestamp = "2026-11-12T10:23:00Z".parse().unwrap();
        let row = build_resume_entry(&session, now);
        assert_eq!(row.name, "build");
        assert_eq!(row.detail, "checkout-copy · mitiplay · 23m ago · shipped");
        assert_eq!(
            chosen(&row.id, 0).unwrap(),
            (build(false, None, Some("B-abc123")), false)
        );
    }

    #[test]
    fn conflicting_modes_are_usage_errors() {
        for argv in [
            &["--bare", "--pm"][..],
            &["--bare", "--task", "T-42"],
            &["--pm", "--task", "T-42"],
            &["--resume", "A-1", "--task", "T-42"],
            &["--bare", "--resume", "A-1"],
            &["--pm", "--resume", "A-1"],
            &["--pm", "--over-cap"],
            &["--resume", "A-1", "--over-cap"],
            &["--assistant", "--bare"],
            &["--assistant", "--pm"],
            &["--assistant", "--resume", "A-1"],
            &["-c"],
            &["--bare", "-c"],
            &["--resume", "A-1", "-c"],
            &["--over-cap", "-c"],
            &["--task", "T-42", "-c"],
            &["--build", "--bare"],
            &["--build", "--pm"],
            &["--build", "--assistant"],
            &["--build", "--task", "T-42"],
            &["--build", "--over-cap"],
            &["--build", "-c"],
            &["--build", "--pm", "-c"],
            &["--here"],
            &["--name", "x"],
            &["--resume", "B-abc123", "--here"],
            &["--resume", "B-abc123", "--name", "x"],
            &["--build", "--here", "--resume", "B-abc123"],
            &["--build", "--name", "x", "--resume", "B-abc123"],
        ] {
            assert!(parse(argv).is_err(), "{argv:?}");
        }
    }

    #[test]
    fn pm_continuation_is_explicit() {
        assert!(!parse(&["--pm"]).unwrap().continue_session);
        for flag in ["-c", "--continue"] {
            assert!(parse(&["--pm", flag]).unwrap().continue_session);
        }
    }
}
