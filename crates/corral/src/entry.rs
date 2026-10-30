//! Attribution established before a command starts producing events.
use std::path::Path;

use anyhow::{Context, Result};
use clap::{ArgMatches, parser::ValueSource};
use corral_core::{Actor, Store};
use corral_ops::Ctx;

use crate::Cmd;

pub struct Entry {
    actor: Option<Actor>,
    via: &'static str,
}

impl Entry {
    pub fn new(cmd: &Cmd, matches: &ArgMatches) -> Result<Self> {
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
            Cmd::Pm { .. } => (Some(Actor::Pm), "pm"),
            Cmd::Board { .. }
            | Cmd::Capture { .. }
            | Cmd::Artifacts { .. }
            | Cmd::Settings { .. } => (Some(Actor::Owner), "tui"),
            _ if std::env::var("CORRAL_PM").as_deref() == Ok("1") => {
                (actor.or(Some(Actor::Pm)), "pm")
            }
            _ => (actor, "cli"),
        };
        Ok(Self { actor, via })
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
        if self.via != "cli" {
            return Ok((self.actor.unwrap_or(Actor::Owner), self.via.into()));
        }
        if let Some(run) = run_from_cwd(store, &std::env::current_dir().context("reading cwd")?)? {
            return Ok((self.actor.unwrap_or(Actor::Agent), format!("run:{run}")));
        }
        Ok((self.actor.unwrap_or(Actor::Owner), "cli".into()))
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
