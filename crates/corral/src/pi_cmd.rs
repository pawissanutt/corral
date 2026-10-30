//! `corral pi`: start the Pi-native host from nothing. In a workspace it opens an assistant, with
//! `--pm` the PM and with `--resume A-<n>` an assistant's next generation; outside a workspace,
//! or with `--bare`, plain Pi. Handler bodies live in `corral_ops::agent::pi`.
use std::path::Path;

use anyhow::{Result, bail};
use clap::Args;
use corral_core::Store;
use corral_ops::agent::hooks::pm::Overrides;
use corral_ops::agent::pi::{self, BareArgs};

use crate::entry::Entry;

#[derive(Args)]
pub struct PiArgs {
    /// Plain unbound Pi in the current directory: no store, binding or corral tools
    #[arg(long, conflicts_with_all = ["pm", "resume", "over_cap"])]
    bare: bool,
    /// Open the workspace PM in this pane, or take over a dead one (plain `corral pi` opens an assistant)
    #[arg(long, conflicts_with_all = ["resume", "over_cap"])]
    pm: bool,
    /// Resume assistant session A-<n> in this pane, fencing its previous generation
    #[arg(long, value_name = "A-N", conflicts_with = "over_cap")]
    resume: Option<String>,
    /// Open an assistant even when assistant.max_live assistants are already live
    #[arg(long)]
    over_cap: bool,
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
    Pm,
    Assistant {
        over_cap: bool,
    },
    Resume(String),
}

pub const BARE_NOTICE: &str =
    "corral pi: no corral workspace here; starting bare Pi (no store, binding or corral tools)";

/// The invocation split: outside a workspace only plain Pi can start.
pub fn invocation(args: &PiArgs, in_workspace: bool) -> Result<Invocation> {
    if args.bare {
        return Ok(Invocation::Bare { notice: false });
    }
    if !in_workspace {
        if args.pm || args.resume.is_some() || args.over_cap {
            bail!(
                "corral pi refused: not_in_workspace: --pm, --resume and --over-cap need a corral \
                 workspace; run them inside one, or `corral pi --bare`"
            );
        }
        return Ok(Invocation::Bare { notice: true });
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
    let invocation = invocation(&args, Store::find_root(start).is_ok())?;
    let overrides = Overrides {
        model: args.model,
        provider: args.provider,
        effort: args.effort,
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
        Invocation::Pm => pi::bound(start, &entry.open(start)?, &overrides),
        Invocation::Assistant { over_cap } => {
            pi::assistant(&entry.open(start)?, &overrides, over_cap)
        }
        Invocation::Resume(id) => pi::resume_assistant(&entry.open(start)?, &id, &overrides),
    }
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
    fn the_invocation_split_follows_flags_and_workspace() {
        use Invocation::*;
        let cases: &[(&[&str], bool, Option<Invocation>)] = &[
            (&[], true, Some(Assistant { over_cap: false })),
            (&["--over-cap"], true, Some(Assistant { over_cap: true })),
            (&["--pm"], true, Some(Pm)),
            (&["--resume", "A-1"], true, Some(Resume("A-1".into()))),
            (&["--bare"], true, Some(Bare { notice: false })),
            (&["--bare"], false, Some(Bare { notice: false })),
            (&[], false, Some(Bare { notice: true })),
            (&["--model", "m"], false, Some(Bare { notice: true })),
            (&["--pm"], false, None),
            (&["--resume", "A-1"], false, None),
            (&["--over-cap"], false, None),
        ];
        for (argv, in_workspace, expected) in cases {
            let args = parse(argv).unwrap();
            let got = invocation(&args, *in_workspace);
            match expected {
                Some(expected) => assert_eq!(&got.unwrap(), expected, "{argv:?}"),
                None => {
                    let error = got.unwrap_err().to_string();
                    assert!(error.contains("not_in_workspace"), "{argv:?}: {error}");
                }
            }
        }
    }

    #[test]
    fn conflicting_modes_are_usage_errors() {
        for argv in [
            &["--bare", "--pm"][..],
            &["--bare", "--resume", "A-1"],
            &["--pm", "--resume", "A-1"],
            &["--pm", "--over-cap"],
            &["--resume", "A-1", "--over-cap"],
        ] {
            assert!(parse(argv).is_err(), "{argv:?}");
        }
    }
}
