//! Attribution established before a command starts producing events.
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use clap::{ArgMatches, parser::ValueSource};
use corral_core::{Actor, Store};
use corral_ops::Ctx;
use corral_ops::assistant::owner_action::ViaAssistant;
use corral_ops::command::Invoker;
use corral_protocol::cmd::{ClientKind, Via};

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
            "assistant_refused: {} needs the owner; route with corral pm inbox post, or use corral_owner_action only for a supported kind explicitly requested by the owner",
            self.0
        )
    }
}

impl std::error::Error for AssistantRefused {}

pub struct Entry {
    via: &'static str,
    invoker: Invoker,
}

impl Entry {
    /// `start` locates the store only when this process is an owner action's command child,
    /// which claims its reservation here, before the command runs.
    pub fn new(cmd: &Cmd, matches: &ArgMatches, start: &Path) -> Result<Self> {
        if let Some(via) = corral_ops::assistant::owner_action::claim_from_env(start)? {
            return Ok(Self::build(
                Some(Actor::Owner),
                "owner_action",
                None,
                Some(via),
                false,
            ));
        }
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
        Ok(Self::build(actor, via, None, None, false))
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
        let planning_safe = class::planning_safe(cmd);
        Ok(Self::build(
            Some(Actor::Assistant),
            "assistant",
            Some(session),
            None,
            planning_safe,
        ))
    }

    /// The caller classification travels in the `Invoker`; `via` keeps the board open path.
    /// An owner action resolves before `via` is consulted, so its wire `via` is `cli`.
    fn build(
        actor: Option<Actor>,
        via: &'static str,
        assistant: Option<String>,
        owner_action: Option<ViaAssistant>,
        planning_safe: bool,
    ) -> Self {
        let wire_via = match via {
            "hook" => Via::Hook,
            "pm" => Via::Pm,
            "tui" => Via::Tui,
            "assistant" => Via::Assistant,
            _ => Via::Cli,
        };
        let invoker = Invoker::from_process(
            ClientKind::Cli,
            actor,
            wire_via,
            assistant,
            planning_safe,
            owner_action,
        );
        Self { via, invoker }
    }

    /// The caller identity every store opening and routed command applies.
    pub fn invoker(&self) -> &Invoker {
        &self.invoker
    }

    /// Whether this process executes an assistant's receipted owner action.
    pub fn owner_action(&self) -> bool {
        self.invoker.owner_action.is_some()
    }

    pub fn open(&self, start: &Path) -> Result<Ctx> {
        if self.via == "tui" {
            return corral_tui::open(&Store::find_root(start)?);
        }
        let store = Store::open_with(&Store::find_root(start)?, |store| self.invoker.apply(store))?;
        Ok(Ctx::from_store(store))
    }

    /// A validated capacity edit may be the operation that installs its compatibility fence.
    /// Apply entry authority first, edit under normal config locks, then load effective config.
    pub fn open_for_capacity_config(
        &self,
        start: &Path,
        edit: impl FnOnce(&Store) -> Result<()>,
    ) -> Result<Ctx> {
        let store = Store::open_with(&Store::find_root(start)?, |store| {
            self.invoker.apply(store)?;
            edit(store)
        })?;
        Ok(Ctx::from_store(store))
    }

    pub fn init(&self, root: &Path) -> Result<Store> {
        Store::init_with(root, |store| self.invoker.apply(store))
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
