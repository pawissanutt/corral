//! `corral pi`: start the corral-pi host from nothing. In a workspace on a terminal it opens the
//! launcher, which asks whether to open an assistant, the PM or a stopped assistant to resume;
//! `--assistant`, `--pm` and `--resume A-<n>` open one directly, and a plain `corral pi` without a
//! terminal opens an assistant. Outside a workspace, or with `--bare`, it starts plain Pi. Handler
//! bodies live in `corral_ops::agent::pi`.
use std::io::IsTerminal;
use std::path::Path;

use anyhow::{Result, bail};
use clap::Args;
use corral_core::Store;
use corral_ops::Ctx;
use corral_ops::agent::hooks::pm::Overrides;
use corral_ops::agent::pi::{self, BareArgs};
use corral_tui::launcher::{self, LauncherEntry, Section};

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
    /// Continue the previous PM conversation instead of starting fresh
    #[arg(short = 'c', long = "continue", requires = "pm",
        conflicts_with_all = ["bare", "resume", "over_cap", "task"])]
    continue_session: bool,
    /// Resume stopped assistant session A-<n> in this pane, fencing its previous generation (inside an assistant, /resume picks one)
    #[arg(long, value_name = "A-N", conflicts_with = "over_cap")]
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
}

pub const BARE_NOTICE: &str =
    "corral pi: no corral workspace here; starting bare Pi (no store, binding or corral tools)";

/// The invocation split: outside a workspace only plain Pi can start; a plain `corral pi` on a
/// terminal (stdin and stdout both) asks with the launcher.
pub fn invocation(args: &PiArgs, in_workspace: bool, terminal: bool) -> Result<Invocation> {
    if args.bare {
        return Ok(Invocation::Bare { notice: false });
    }
    if !in_workspace {
        if args.pm
            || args.assistant
            || args.resume.is_some()
            || args.over_cap
            || args.task.is_some()
        {
            bail!(
                "corral pi refused: not_in_workspace: --pm, --assistant, --resume, --task and \
                 --over-cap need a corral workspace; run them inside one, or `corral pi --bare`"
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
        Invocation::Launcher => {
            let ctx = entry.open(start)?;
            let Some(id) = launcher::pick(&launcher_entries(&ctx)?)? else {
                return Ok(());
            };
            let (next, continue_session) = chosen(&id)?;
            overrides.continue_session = continue_session;
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
        other @ (Invocation::Bare { .. } | Invocation::Launcher) => {
            bail!("corral pi: {other:?} does not open a bound session")
        }
    }
}

const ASSISTANT_ROW: &str = "assistant";
const PM_ROW: &str = "pm";
/// Stopped assistants the launcher lists, newest first.
const RESUME_ROWS: usize = 8;

/// The launcher's rows: start an assistant or the PM, or resume one of the newest stopped
/// assistants (those `corral pi --resume` accepts: open, with no running host).
fn launcher_entries(ctx: &Ctx) -> Result<Vec<LauncherEntry>> {
    let mut entries = vec![
        LauncherEntry {
            id: ASSISTANT_ROW.into(),
            section: Section::Start,
            key: Some('a'),
            name: "Assistant".into(),
            detail: "board-aware · scratch worktrees · owner actions on your instruction".into(),
        },
        LauncherEntry {
            id: PM_ROW.into(),
            section: Section::Start,
            key: Some('p'),
            name: "PM".into(),
            detail: "workspace coordinator · continues the previous conversation".into(),
        },
    ];
    let mut stopped: Vec<_> = corral_ops::assistant_stats(&ctx.store, None)?
        .into_iter()
        .filter(|row| row.ended.is_none() && !row.live)
        .collect();
    stopped.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
    let now = jiff::Timestamp::now();
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
        entries.push(LauncherEntry {
            id: row.session.clone(),
            section: Section::Resume,
            key: None,
            name: row.session,
            detail: format!("{title} · {project} · {age} ago"),
        });
    }
    Ok(entries)
}

/// What the launcher's row stands for, and whether it continues the previous PM conversation.
fn chosen(id: &str) -> Result<(Invocation, bool)> {
    Ok(match id {
        ASSISTANT_ROW => (Invocation::Assistant { over_cap: false }, false),
        PM_ROW => (Invocation::Pm, true),
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

    #[test]
    fn the_invocation_split_follows_flags_workspace_and_terminal() {
        use Invocation::*;
        // (argv, in a workspace, on a terminal, what opens; None refuses not_in_workspace)
        let cases: &[(&[&str], bool, bool, Option<Invocation>)] = &[
            (&[], true, true, Some(Launcher)),
            (
                &["--model", "m", "--effort", "high"],
                true,
                true,
                Some(Launcher),
            ),
            (&[], true, false, Some(Assistant { over_cap: false })),
            (
                &["--assistant"],
                true,
                true,
                Some(Assistant { over_cap: false }),
            ),
            (
                &["--assistant"],
                true,
                false,
                Some(Assistant { over_cap: false }),
            ),
            (
                &["--assistant", "--over-cap"],
                true,
                true,
                Some(Assistant { over_cap: true }),
            ),
            (
                &["--over-cap"],
                true,
                true,
                Some(Assistant { over_cap: true }),
            ),
            (
                &["--over-cap"],
                true,
                false,
                Some(Assistant { over_cap: true }),
            ),
            (&["--pm"], true, true, Some(Pm)),
            (&["--pm", "-c"], true, true, Some(Pm)),
            (&["--pm", "--continue"], true, false, Some(Pm)),
            (
                &["--task", "T-42"],
                true,
                true,
                Some(Assistant { over_cap: false }),
            ),
            (&["--task", "T-42"], false, true, None),
            (&["--resume", "A-1"], true, true, Some(Resume("A-1".into()))),
            (&["--bare"], true, true, Some(Bare { notice: false })),
            (&["--bare"], false, false, Some(Bare { notice: false })),
            (&[], false, true, Some(Bare { notice: true })),
            (&[], false, false, Some(Bare { notice: true })),
            (&["--model", "m"], false, true, Some(Bare { notice: true })),
            (&["--pm"], false, true, None),
            (&["--assistant"], false, true, None),
            (&["--resume", "A-1"], false, true, None),
            (&["--over-cap"], false, true, None),
        ];
        for (argv, in_workspace, terminal, expected) in cases {
            let args = parse(argv).unwrap();
            let got = invocation(&args, *in_workspace, *terminal);
            match expected {
                Some(expected) => assert_eq!(&got.unwrap(), expected, "{argv:?} {terminal}"),
                None => {
                    let error = got.unwrap_err().to_string();
                    assert!(error.contains("not_in_workspace"), "{argv:?}: {error}");
                }
            }
        }
    }

    #[test]
    fn launcher_rows_open_what_their_flags_open() {
        assert_eq!(
            chosen(ASSISTANT_ROW).unwrap(),
            (Invocation::Assistant { over_cap: false }, false)
        );
        // The PM row says it continues the previous conversation: `--pm -c`.
        assert_eq!(chosen(PM_ROW).unwrap(), (Invocation::Pm, true));
        assert_eq!(
            chosen("A-7").unwrap(),
            (Invocation::Resume("A-7".into()), false)
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
