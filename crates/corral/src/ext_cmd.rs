//! Command-only extension discovery (T-381 section 3.4).
//!
//! `corral <name> args...` forwards to `corral-ext-<name>` on PATH only when clap rejects
//! `<name>` as an unknown subcommand and the name is not in [`RESERVED`]. `corral ext run`
//! is the collision-proof form and `corral ext list` lists PATH candidates without
//! executing them. Forwarding passes argv directly (no shell), inherits stdio and returns
//! the child's exit status.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

pub const PREFIX: &str = "corral-ext-";

/// Names that never fall through to an extension. This is every built-in subcommand (a
/// test checks that it covers each `Cmd` variant, alias and `ext`), clap's `help`, and
/// names proposed or announced for built-in commands, so an extension installed today
/// cannot change meaning when the built-in lands.
pub const RESERVED: &[&str] = &[
    // Built-in subcommands, in `Cmd` order.
    "mode",
    "inbox",
    "attention",
    "agent",
    "pi",
    "question",
    "automation",
    "selection",
    "setup",
    "runtime-probe",
    "digest",
    "revert",
    "review-sprint",
    "idea",
    "lessons",
    "plan",
    "build-identity",
    "hooks",
    "run",
    "queue",
    "wait",
    "up",
    "doctor",
    "init",
    "pricing",
    "import",
    "stats",
    "new",
    "list",
    "search",
    "log",
    "show",
    "comment",
    "move",
    "edit",
    "route",
    "dispatch",
    "reply",
    "unstick",
    "done",
    "criteria",
    "evidence",
    "gate",
    "review-run",
    "verdict",
    "dashboard",
    "artifact",
    "pin",
    "unpin",
    "pins",
    "open",
    "review",
    "approve",
    "changes",
    "retry",
    "discard",
    "jump",
    "sync",
    "autopilot",
    "hook",
    "bind-session",
    "pm",
    "focus-board",
    "board",
    "capture",
    "artifacts",
    "settings",
    "harness",
    "plugin",
    "skill",
    "config",
    "ext",
    "help",
    // Proposed or announced built-ins: `corral migrate` and `corral read` (T-339 and
    // T-381 plans), `corral accept` and `corral report` (PM records).
    "accept",
    "migrate",
    "read",
    "report",
];

/// Run or list external `corral-ext-<name>` commands found on PATH
#[derive(Parser)]
#[command(name = "ext")]
pub struct ExtArgs {
    #[command(subcommand)]
    cmd: ExtCmd,
}

#[derive(Subcommand)]
enum ExtCmd {
    /// Run corral-ext-<NAME> from PATH with every later argument unchanged, even when NAME is a built-in or reserved name
    #[command(
        subcommand_required = true,
        disable_help_subcommand = true,
        subcommand_value_name = "NAME",
        override_usage = "corral ext run [OPTIONS] <NAME> [ARGS]..."
    )]
    Run {
        #[command(subcommand)]
        target: RunTarget,
    },
    /// List corral-ext-* executables on PATH, in PATH order, without running them
    List {
        #[arg(long)]
        json: bool,
    },
}

/// NAME is captured as an external subcommand so that every argument after it, including
/// `-h`, `--help` and `--root`, reaches the extension instead of corral's parser.
#[derive(Subcommand)]
enum RunTarget {
    #[command(external_subcommand)]
    Name(Vec<OsString>),
}

pub fn command() -> clap::Command {
    crate::Cli::command().subcommand(ExtArgs::command())
}

/// Forward a top-level unknown subcommand to its extension and exit with the child's
/// status. Returns without effect when the error is anything else, the name is reserved
/// or no `corral-ext-<name>` is on PATH, so the caller reports clap's ordinary error.
pub fn forward_unknown(err: &clap::Error) {
    if err.kind() != clap::error::ErrorKind::InvalidSubcommand {
        return;
    }
    let mut args = std::env::args_os().skip(1);
    let name = loop {
        match args.next() {
            Some(arg) if arg == "--root" => {
                args.next();
            }
            Some(arg) if arg.to_string_lossy().starts_with("--root=") => {}
            Some(arg) => break arg,
            None => return,
        }
    };
    let Some(name) = name.to_str() else { return };
    if RESERVED.contains(&name) {
        return;
    }
    if let Some(path) = find(name) {
        std::process::exit(exec(&path, args.collect()));
    }
}

pub fn run(matches: &clap::ArgMatches) -> i32 {
    let args = ExtArgs::from_arg_matches(matches).unwrap_or_else(|err| err.exit());
    let result = match args.cmd {
        ExtCmd::Run {
            target: RunTarget::Name(argv),
        } => run_named(argv),
        ExtCmd::List { json } => list(json).map(|()| 0),
    };
    result.unwrap_or_else(|err| {
        eprintln!("error: {err:#}");
        1
    })
}

fn run_named(argv: Vec<OsString>) -> Result<i32> {
    let mut argv = argv.into_iter();
    let name = argv.next().unwrap_or_default();
    let name = name.to_string_lossy();
    if !valid_name(&name) {
        bail!(
            "invalid extension name '{name}' (letters, digits, '-' and '_'; starts alphanumeric)"
        );
    }
    let path = find(&name).with_context(|| format!("no {PREFIX}{name} executable on PATH"))?;
    Ok(exec(&path, argv.collect()))
}

fn valid_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Absolute PATH directories in order; empty and relative entries are never searched.
fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|dir| dir.is_absolute())
        .collect()
}

fn find(name: &str) -> Option<PathBuf> {
    if !valid_name(name) {
        return None;
    }
    path_dirs()
        .into_iter()
        .map(|dir| dir.join(format!("{PREFIX}{name}")))
        .find(|path| is_executable(path))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Replace this process with the extension (Unix), or run it to completion elsewhere.
fn exec(path: &Path, args: Vec<OsString>) -> i32 {
    let mut cmd = Command::new(path);
    cmd.args(args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        eprintln!("error: cannot run {}: {err}", path.display());
        126
    }
    #[cfg(not(unix))]
    match cmd.status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(err) => {
            eprintln!("error: cannot run {}: {err}", path.display());
            126
        }
    }
}

struct Candidate {
    name: String,
    path: PathBuf,
    reserved: bool,
    shadowed: bool,
}

/// Every executable `corral-ext-*` on PATH in PATH order. Only directory listings and
/// metadata are read; no candidate is executed.
fn candidates() -> Vec<Candidate> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for dir in path_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut found: Vec<(String, PathBuf)> = entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let file = entry.file_name().into_string().ok()?;
                let name = file.strip_prefix(PREFIX)?.to_owned();
                let path = entry.path();
                (valid_name(&name) && is_executable(&path)).then_some((name, path))
            })
            .collect();
        found.sort();
        for (name, path) in found {
            out.push(Candidate {
                reserved: RESERVED.contains(&name.as_str()),
                shadowed: !seen.insert(name.clone()),
                name,
                path,
            });
        }
    }
    out
}

fn list(json: bool) -> Result<()> {
    let found = candidates();
    if json {
        let rows: Vec<_> = found
            .iter()
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "path": c.path,
                    "reserved": c.reserved,
                    "shadowed": c.shadowed,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if found.is_empty() {
        println!("no {PREFIX}* executables on PATH");
        return Ok(());
    }
    let width = found.iter().map(|c| c.name.len()).max().unwrap_or(0);
    for c in &found {
        let note = if c.shadowed {
            "  (shadowed by an earlier PATH entry)".to_owned()
        } else if c.reserved {
            format!("  (reserved name: run with `corral ext run {}`)", c.name)
        } else {
            String::new()
        };
        println!("{:width$}  {}{note}", c.name, c.path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_covers_every_cmd_variant_alias_and_help() {
        let mut cli = command();
        cli.build();
        let mut names = Vec::new();
        for sub in cli.get_subcommands() {
            names.push(sub.get_name().to_owned());
            names.extend(sub.get_all_aliases().map(str::to_owned));
        }
        assert!(names.iter().any(|n| n == "ext"), "ext is registered");
        assert!(names.iter().any(|n| n == "help"), "clap help is built");
        for name in &names {
            assert!(
                RESERVED.contains(&name.as_str()),
                "{name} missing from RESERVED"
            );
        }
        let mut unique = RESERVED.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), RESERVED.len(), "RESERVED has duplicates");
    }

    #[test]
    fn names_are_one_path_component() {
        assert!(valid_name("foo"));
        assert!(valid_name("my_tool-2"));
        for bad in ["", "-x", "../x", "a/b", "a b", ".hidden"] {
            assert!(!valid_name(bad), "{bad:?} accepted");
        }
    }
}
