//! Attribution established before a command starts producing events.
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use clap::{ArgMatches, parser::ValueSource};
use corral_core::{Actor, Store};
use corral_ops::Ctx;

use crate::Cmd;
use crate::pm_cmd::PmCmd;

mod class;

/// An assistant shell ran a command outside its Read and File classes; the CLI exits 2.
#[derive(Debug)]
pub struct AssistantRefused(String);

impl std::fmt::Display for AssistantRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "assistant_refused: {} needs the owner; use corral_owner_action or corral pm inbox post",
            self.0
        )
    }
}

impl std::error::Error for AssistantRefused {}

pub struct Entry {
    actor: Option<Actor>,
    via: &'static str,
    assistant: Option<String>,
}

impl Entry {
    pub fn new(cmd: &Cmd, matches: &ArgMatches) -> Result<Self> {
        if let Some(session) = std::env::var_os("CORRAL_ASSISTANT") {
            return Self::assistant(cmd, matches, session);
        }
        let explicit = matches.subcommand().and_then(|(_, args)| {
            ["author", "origin"].into_iter().find_map(|key| {
                (args.try_contains_id(key).unwrap_or(false)
                    && args.value_source(key) == Some(ValueSource::CommandLine))
                .then(|| args.get_raw(key).and_then(|mut values| values.next()))
                .flatten()
            })
        });
        let actor = explicit
            .map(|value| {
                serde_json::from_value(serde_json::Value::String(
                    value.to_string_lossy().into_owned(),
                ))
            })
            .transpose()
            .context("parsing command event author")?;
        let (actor, via) = match cmd {
            Cmd::Hook | Cmd::Plugin { .. } | Cmd::FocusBoard => (Some(Actor::System), "hook"),
            Cmd::Pm { cmd } if !cmd.as_ref().is_some_and(PmCmd::acts_as_caller) => {
                (Some(Actor::Pm), "pm")
            }
            Cmd::Board { .. }
            | Cmd::Capture { .. }
            | Cmd::Artifacts { .. }
            | Cmd::Settings { .. } => (Some(Actor::Owner), "tui"),
            _ if std::env::var("CORRAL_PM").as_deref() == Ok("1") => {
                (actor.or(Some(Actor::Pm)), "pm")
            }
            _ => (actor, "cli"),
        };
        Ok(Self {
            actor,
            via,
            assistant: None,
        })
    }

    /// An assistant shell outranks the PM marker, a run worktree cwd and explicit author flags.
    fn assistant(cmd: &Cmd, matches: &ArgMatches, session: std::ffi::OsString) -> Result<Self> {
        let session = session
            .into_string()
            .map_err(|_| anyhow!("CORRAL_ASSISTANT is not valid UTF-8"))?;
        corral_core::assistant::validate_id(&session)
            .context("CORRAL_ASSISTANT must name this shell's assistant session")?;
        if class::of(cmd, &session) == class::CommandClass::Refused {
            return Err(AssistantRefused(command_path(matches)).into());
        }
        Ok(Self {
            actor: Some(Actor::Assistant),
            via: "assistant",
            assistant: Some(session),
        })
    }

    pub fn open(&self, start: &Path) -> Result<Ctx> {
        if self.via == "tui" {
            return corral_tui::open(&Store::find_root(start)?);
        }
        let store = Store::open_with(&Store::find_root(start)?, |store| self.apply(store))?;
        Ok(Ctx::from_store(store))
    }

    pub fn init(&self, root: &Path) -> Result<Store> {
        Store::init_with(root, |store| self.apply(store))
    }

    pub fn apply(&self, store: &Store) -> Result<()> {
        let (actor, via) = self.resolve(store)?;
        store.set_attribution(actor, &via)
    }

    fn resolve(&self, store: &Store) -> Result<(Actor, String)> {
        if let Some(session) = &self.assistant {
            return Ok((Actor::Assistant, format!("assistant:{session}")));
        }
        if self.via != "cli" {
            return Ok((self.actor.unwrap_or(Actor::Owner), self.via.into()));
        }
        if let Some(run) = run_from_cwd(store, &std::env::current_dir().context("reading cwd")?)? {
            return Ok((self.actor.unwrap_or(Actor::Agent), format!("run:{run}")));
        }
        Ok((self.actor.unwrap_or(Actor::Owner), "cli".into()))
    }

    /// The PM inbox author this invocation acts as: `owner`, `assistant:A-<n>` or its actor.
    pub fn inbox_author(&self, store: &Store) -> Result<String> {
        let (actor, via) = self.resolve(store)?;
        Ok(match actor {
            Actor::Assistant => via,
            _ => self.decision_author(store)?.into(),
        })
    }

    pub fn decision_author(&self, store: &Store) -> Result<&'static str> {
        Ok(match self.resolve(store)?.0 {
            Actor::Owner => "owner",
            Actor::Pm => "pm",
            Actor::Agent => "agent",
            Actor::Assistant => "assistant",
            Actor::Autopilot => "autopilot",
            Actor::Reviewer => "reviewer",
            Actor::System => "system",
        })
    }
}

fn command_path(matches: &ArgMatches) -> String {
    if matches.subcommand().is_none() {
        return "corral up".into();
    }
    let mut path = String::from("corral");
    let mut current = matches;
    while let Some((name, sub)) = current.subcommand() {
        path.push(' ');
        path.push_str(name);
        current = sub;
    }
    path
}

fn run_from_cwd(store: &Store, cwd: &Path) -> Result<Option<String>> {
    // launch.rs/render_command export no run identity. Like infer_project, use
    // canonical cwd containment, but match the persisted current run's worktree
    // rather than guessing R1 from a .worktrees/<task> name. This also recognizes
    // nested directories and retries, and does not mislabel unrelated worktrees.
    // Launch always uses .worktrees/<task>; avoid enumerating task files for
    // ordinary shell commands, including while a task is being published.
    let cwd = cwd.canonicalize().context("resolving current directory")?;
    if !cwd
        .components()
        .any(|component| component.as_os_str() == ".worktrees")
    {
        return Ok(None);
    }
    Ok(store
        .list()?
        .into_iter()
        .filter_map(|task| task.current_run().cloned())
        .filter_map(|run| {
            let worktree = run.worktree.canonicalize().ok()?;
            cwd.starts_with(&worktree)
                .then_some((worktree.components().count(), run.id))
        })
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, id)| id))
}
