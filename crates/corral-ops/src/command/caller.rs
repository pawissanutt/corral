//! Who runs a command: the wire `Caller` plus local-only owner-action authority.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use corral_core::{Actor, Store};
use corral_protocol::cmd::{ActorName, Caller, CallerEnv, ClientKind, Principal, Via};

use crate::assistant::owner_action::{self, OwnerAuthority, ViaAssistant};

#[cfg(test)]
#[path = "caller_tests.rs"]
mod tests;

/// The caller record every caller-dependent check reads instead of this process's environment.
#[derive(Debug, Clone)]
pub struct Invoker {
    pub caller: Caller,
    /// Local only (`Entry::new` claim). Never serialized; forces local routing.
    pub owner_action: Option<ViaAssistant>,
    // Never deserialized from Caller. Only the authenticated daemon adapter supplies this.
    device: Option<crate::agent::app_runtime::AuthenticatedDevice>,
    admin: bool,
}

impl Invoker {
    /// An admitted CLI caller. App claims remain untrusted and are refused by `resolve`.
    pub fn from_caller(caller: Caller) -> Self {
        Self {
            caller,
            owner_action: None,
            device: None,
            admin: false,
        }
    }

    /// The daemon calls this only after authenticating and rechecking the device registry.
    /// Neither the wire Caller nor its environment can grant device authority.
    pub fn authenticated_device(
        caller: Caller,
        device: crate::agent::app_runtime::AuthenticatedDevice,
        admin: bool,
    ) -> Result<Self> {
        let invoker = Self {
            caller,
            owner_action: None,
            device: Some(device),
            admin,
        };
        invoker.device()?;
        Ok(invoker)
    }

    pub fn device(&self) -> Result<&crate::agent::app_runtime::AuthenticatedDevice> {
        let device = self
            .device
            .as_ref()
            .context("not_app_safe: authenticated device required")?;
        let env = &self.caller.env;
        ensure!(
            matches!(&self.caller.principal, Principal::Device { id } if id == &device.id)
                && !device.id.is_empty()
                && self.caller.client == ClientKind::App
                && self.caller.via == Via::App
                && self.caller.actor == Some(ActorName::Owner)
                && self.caller.assistant.is_none()
                && self.caller.cwd.is_none()
                && !self.caller.planning_safe
                && self.owner_action.is_none()
                && [
                    &env.corral_pm,
                    &env.corral_assistant,
                    &env.ai_agent,
                    &env.pi_coding_agent,
                    &env.corral_agent,
                    &env.corral_reviewer,
                    &env.corral_agent_task,
                    &env.corral_agent_bootstrap,
                    &env.corral_task
                ]
                .iter()
                .all(|v| v.is_none()),
            "not_app_safe: invalid authenticated device caller"
        );
        Ok(device)
    }

    pub(crate) fn device_admin(&self) -> Result<bool> {
        self.device()?;
        Ok(self.admin)
    }

    pub(crate) fn is_app(&self) -> bool {
        self.device.is_some()
            || matches!(self.caller.principal, Principal::Device { .. })
            || self.caller.client == ClientKind::App
            || self.caller.via == Via::App
    }

    /// Records this process's caller-identity variables and raw working directory.
    pub fn from_process(
        client: ClientKind,
        actor: Option<Actor>,
        via: Via,
        assistant: Option<String>,
        planning_safe: bool,
        owner_action: Option<ViaAssistant>,
    ) -> Invoker {
        let cwd = std::env::current_dir()
            .ok()
            .and_then(|cwd| cwd.into_os_string().into_string().ok());
        Invoker {
            caller: Caller {
                principal: Principal::Unverified,
                client,
                actor: actor.map(actor_name),
                via,
                assistant,
                planning_safe,
                cwd,
                env: env_from_process(),
            },
            owner_action,
            device: None,
            admin: false,
        }
    }

    /// The classic board: the owner through the TUI, with this process's markers and cwd.
    pub fn board() -> Invoker {
        Self::from_process(
            ClientKind::Board,
            Some(Actor::Owner),
            Via::Tui,
            None,
            false,
            claimed_owner_action(),
        )
    }

    /// This process as a plain CLI caller (markers, cwd and any claimed owner action), for
    /// operations that are not handed a caller record.
    pub fn process() -> Invoker {
        Self::from_process(
            ClientKind::Cli,
            None,
            Via::Cli,
            None,
            false,
            claimed_owner_action(),
        )
    }

    /// Checks an assistant's planning boundary and sets the store's event attribution.
    pub fn apply(&self, store: &Store) -> Result<()> {
        if let Some(session) = &self.caller.assistant
            && !self.caller.planning_safe
        {
            store.inspect_assistant_container(session, |container, _| {
                crate::assistant::workflow::boundary::require_ordinary(store, session, container)
            })?;
        }
        let (actor, via) = self.resolve(store)?;
        store.set_attribution(actor, &via)?;
        if let Some(authority) = &self.owner_action {
            store.set_attribution_detail("owner_authority", serde_json::to_value(authority)?)?;
        } else if self.is_app() {
            let device = self.device()?;
            store.set_attribution_detail(
                "device",
                serde_json::json!({"device": device.id, "name": device.name}),
            )?;
        }
        Ok(())
    }

    /// The event actor and `via` this caller's events carry.
    pub fn resolve(&self, store: &Store) -> Result<(Actor, String)> {
        if self.is_app() {
            let device = self.device()?;
            return Ok((Actor::Owner, format!("app:{}", device.id)));
        }
        if let Some(authority) = &self.owner_action {
            let via = authority.device.as_ref().map_or_else(
                || format!("assistant:{}", authority.session),
                |device| format!("app:{device}"),
            );
            return Ok((Actor::Owner, via));
        }
        if let Some(session) = &self.caller.assistant {
            return Ok((Actor::Assistant, format!("assistant:{session}")));
        }
        let actor = self.actor();
        if self.caller.via != Via::Cli {
            return Ok((
                actor.unwrap_or(Actor::Owner),
                via_name(self.caller.via).into(),
            ));
        }
        if let Some(run) = run_from_cwd(store, &self.cwd().context("reading cwd")?)? {
            return Ok((actor.unwrap_or(Actor::Agent), format!("run:{run}")));
        }
        Ok((actor.unwrap_or(Actor::Owner), "cli".into()))
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

    /// `CORRAL_PM=1`.
    pub fn pm_marker(&self) -> bool {
        self.caller.env.corral_pm.as_deref() == Some("1")
    }

    /// Whether the caller runs in an agent or assistant shell (`AI_AGENT`, `PI_CODING_AGENT` or
    /// `CORRAL_ASSISTANT`), unless it claimed a receipted owner action.
    pub fn agent_entry(&self) -> bool {
        if self.owner_action.is_some() {
            return false;
        }
        let env = &self.caller.env;
        [&env.ai_agent, &env.pi_coding_agent, &env.corral_assistant]
            .iter()
            .any(|value| value.is_some())
    }

    /// Whether any managed-worker marker is set (`AI_AGENT`, `PI_CODING_AGENT`, `CORRAL_AGENT`,
    /// `CORRAL_REVIEWER`, `CORRAL_AGENT_TASK`).
    pub fn managed_marker(&self) -> bool {
        let env = &self.caller.env;
        [
            &env.ai_agent,
            &env.pi_coding_agent,
            &env.corral_agent,
            &env.corral_reviewer,
            &env.corral_agent_task,
        ]
        .iter()
        .any(|value| value.is_some())
    }

    /// The caller's working directory, as recorded (not canonicalized).
    pub fn cwd(&self) -> std::io::Result<PathBuf> {
        self.caller
            .cwd
            .as_ref()
            .map(PathBuf::from)
            .ok_or_else(|| std::io::Error::other("current directory unavailable"))
    }

    fn actor(&self) -> Option<Actor> {
        Some(match self.caller.actor? {
            ActorName::Owner => Actor::Owner,
            ActorName::Pm => Actor::Pm,
            ActorName::Agent => Actor::Agent,
            ActorName::Assistant => Actor::Assistant,
            ActorName::Autopilot => Actor::Autopilot,
            ActorName::Reviewer => Actor::Reviewer,
            ActorName::System => Actor::System,
            ActorName::Other => return None,
        })
    }
}

/// Reads this process's caller-identity variables (`to_string_lossy` of each `var_os`).
pub fn env_from_process() -> CallerEnv {
    let var = |key: &str| std::env::var_os(key).map(|value| value.to_string_lossy().into_owned());
    CallerEnv {
        corral_pm: var("CORRAL_PM"),
        corral_assistant: var("CORRAL_ASSISTANT"),
        ai_agent: var("AI_AGENT"),
        pi_coding_agent: var("PI_CODING_AGENT"),
        corral_agent: var("CORRAL_AGENT"),
        corral_reviewer: var("CORRAL_REVIEWER"),
        corral_agent_task: var("CORRAL_AGENT_TASK"),
        corral_agent_bootstrap: var("CORRAL_AGENT_BOOTSTRAP"),
        corral_task: var("CORRAL_TASK"),
        corral_herdr_session: var("CORRAL_HERDR_SESSION"),
        herdr_env: var("HERDR_ENV"),
        herdr_socket_path: var("HERDR_SOCKET_PATH"),
        herdr_session: var("HERDR_SESSION"),
    }
}

/// The owner action this process claimed, if any.
fn claimed_owner_action() -> Option<ViaAssistant> {
    match owner_action::authority() {
        OwnerAuthority::ViaAssistant(via) => Some(via.clone()),
        OwnerAuthority::Shell => None,
    }
}

fn actor_name(actor: Actor) -> ActorName {
    match actor {
        Actor::Owner => ActorName::Owner,
        Actor::Pm => ActorName::Pm,
        Actor::Agent => ActorName::Agent,
        Actor::Assistant => ActorName::Assistant,
        Actor::Autopilot => ActorName::Autopilot,
        Actor::Reviewer => ActorName::Reviewer,
        Actor::System => ActorName::System,
    }
}

/// The `via` attribution string, equal to the variant's serde name.
fn via_name(via: Via) -> &'static str {
    match via {
        Via::Cli => "cli",
        Via::Pm => "pm",
        Via::Hook => "hook",
        Via::Tui => "tui",
        Via::Assistant => "assistant",
        Via::App => "app",
        Via::Other => "other",
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
        .view()?
        .run_worktrees(true)?
        .into_iter()
        .filter_map(|run| {
            let worktree = run.worktree.canonicalize().ok()?;
            cwd.starts_with(&worktree)
                .then_some((worktree.components().count(), run.run))
        })
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, id)| id))
}
