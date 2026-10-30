//! Assistant sessions on the Pi-native host (T-544, member assistant-launch): the soft live cap
//! and machine-load admission, a new session in the owner's pane, generation-fenced resume and
//! host shutdown when a session ends. An assistant takes no coding slot and names no project;
//! its pane is the owner's, adopted like the PM's: corral never closes it or types into it.
use anyhow::{Context, Result};
use corral_core::agent::{AgentRole, ContainerRef};
use corral_core::assistant::AssistantSession;
use corral_core::{ProfileChoice, ResolvedProfile, Store};
use serde_json::json;

use crate::Ctx;
use crate::agent::admission::{BindTarget, fence_generation};
use crate::agent::bridge::BridgeClient;
use crate::agent::bridge::liveness::bound_host;
use crate::agent::hooks::pm::Overrides;
use crate::agent::launch::{self, CTL_CAP_FILE, HostSpec, Prepared, SOCKET_FILE};
use crate::agent::pi::{host_alive, refuse};

/// An open session whose current generation has a verifiably running host, and its pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveAssistant {
    pub id: String,
    pub pane: Option<String>,
}

/// Every open session with a running host, oldest first.
pub fn live(store: &Store) -> Result<Vec<LiveAssistant>> {
    let mut live = Vec::new();
    for session in store.list_assistants()? {
        if session.ended.is_none() {
            let container = store.get_assistant_container(&session.id)?;
            if host_alive(&container.state)? {
                live.push(LiveAssistant {
                    id: session.id,
                    pane: container.pane,
                });
            }
        }
    }
    Ok(live)
}

/// The profile an assistant runs: `name` (a resumed session's), else `assistant.agent`, else the
/// PM's profile, with this launch's overrides and the role's asset and credential preflight.
pub fn profile(ctx: &Ctx, name: Option<&str>, overrides: &Overrides) -> Result<ResolvedProfile> {
    let config = ctx.store.config_layers()?.config()?;
    let name = name
        .map(str::to_owned)
        .or_else(|| config.assistant.agent.clone())
        .unwrap_or_else(|| config.pm.agent.clone());
    let choice = ProfileChoice {
        profile: Some(name.clone()),
        ..ProfileChoice::default()
    };
    let mut resolved = config.resolve_profile(&choice, None, None)?;
    if !crate::agent::hooks::is_pi_native(&resolved.harness) {
        let profiles: Vec<&str> = config
            .profiles
            .iter()
            .filter(|(_, profile)| crate::agent::hooks::is_pi_native(&profile.harness))
            .map(|(name, _)| name.as_str())
            .collect();
        let fix = if profiles.is_empty() {
            "no Pi-native profile exists; add one with `corral config set profiles.<name> ...` or use setup".to_owned()
        } else {
            let preferred = if profiles.contains(&"pi-sol") {
                "pi-sol"
            } else {
                profiles[0]
            };
            format!(
                "run `corral config set assistant.agent {preferred}`; Pi-native profiles: {}",
                profiles.join(", ")
            )
        };
        return Err(refuse(
            "assistant_profile",
            format!(
                "profile '{name}' runs harness '{}', and assistants are Pi-native only; {fix}",
                resolved.harness
            ),
        ));
    }
    crate::agent::hooks::pm::apply(&mut resolved, overrides)?;
    if resolved.model.is_none() {
        return Err(refuse(
            "assistant_profile",
            format!(
                "profile '{name}' names no model; pass `--model <model>` or run `corral config set profiles.{name}.model <model>`"
            ),
        ));
    }
    crate::agent::hooks::preflight::check(ctx, AgentRole::Assistant, &resolved)?;
    Ok(resolved)
}

/// Admission before any record exists: above `assistant.max_live` live hosts only `over_cap`
/// opens another (returned as a warning), and a critical machine refuses with the probe values.
pub fn admit(ctx: &Ctx, over_cap: bool) -> Result<Option<String>> {
    let max = ctx.store.config.assistant.max_live;
    let live = live(&ctx.store)?;
    let warning = if live.len() >= max as usize {
        let ids: Vec<&str> = live.iter().map(|l| l.id.as_str()).collect();
        let detail = format!(
            "{} assistants are live ({}) and assistant.max_live is {max}",
            live.len(),
            ids.join(", ")
        );
        if !over_cap {
            return Err(refuse(
                "assistant_cap",
                format!(
                    "{detail}; rerun with `corral pi --over-cap` to open one more, or end one \
                     with `corral assistant end <id>`"
                ),
            ));
        }
        Some(format!("warning: {detail}; opening one more (--over-cap)"))
    } else {
        None
    };
    let state = crate::resources::read_status(ctx)?;
    if state.severe {
        return Err(refuse(
            "machine_load",
            format!(
                "the machine is critically loaded ({}); assistants open once it recovers",
                state
                    .reason
                    .as_deref()
                    .unwrap_or("memory or disk below its minimum")
            ),
        ));
    }
    Ok(warning)
}

fn pane_known(ctx: &Ctx, pane: &str) -> Result<()> {
    if ctx.herdr.pane_get(pane)?.is_none() {
        return Err(refuse(
            "pane_unknown",
            format!("{pane} is not a pane of this store's herdr server"),
        ));
    }
    Ok(())
}

/// The Pi session file a resumed generation continues: the latest hello-bound one, once Pi has
/// written it. Pi writes a session file only with its first message, so a session that was never
/// prompted has nothing to continue and its next generation starts a fresh file beside it.
pub(crate) fn continued_session(ctx: &Ctx, id: &str) -> Result<Option<std::path::PathBuf>> {
    let state = ctx.store.get_assistant_container(id)?.state;
    let latest = state
        .generations
        .iter()
        .rev()
        .find_map(|generation| generation.bound.as_ref())
        .map(|bound| bound.session_file.clone());
    Ok(latest
        .filter(|file| std::fs::metadata(file).is_ok_and(|meta| meta.is_file() && meta.len() > 0)))
}

/// Opens session `id`'s next generation in `pane` and writes its one-shot bootstrap.
fn prepare(ctx: &Ctx, id: &str, pane: &str, resolved: &ResolvedProfile) -> Result<Prepared> {
    let model = resolved
        .model
        .clone()
        .context("an assistant needs a model")?;
    let spec = HostSpec {
        cwd: ctx.store.root().to_path_buf(),
        provider: launch::host_provider(resolved.vendor.as_deref(), &model),
        model,
        effort: resolved.effort.clone(),
        prompt_file: {
            let session = ctx.store.get_assistant(id)?;
            if session.focus != "workspace" {
                let brief = crate::assistant::task_brief(&ctx.store, &session.focus)?;
                let path = ctx.store.prompts_dir().join(format!("{id}-focus.md"));
                std::fs::write(&path, brief)
                    .with_context(|| format!("writing {}", path.display()))?;
                Some(path)
            } else {
                None
            }
        },
        adopt: true,
    };
    let target = BindTarget::Assistant { session: id.into() };
    let prepared = launch::prepare_bootstrap(ctx, &target, pane, &spec)?;
    if resolved.vendor.as_deref() == Some("amazon-bedrock") {
        let binding =
            crate::agent::aws_profile::read_binding(&crate::agent::install::data_root()?)?
                .context("credentials_unavailable: Amazon Bedrock login binding is missing")?;
        if let Some(text) =
            crate::agent::hooks::preflight::metered_notice("assistant", resolved, &binding)
        {
            corral_core::messages::report(None, "metered", text);
        }
    }
    Ok(prepared)
}

/// A new assistant session in the owner's pane `pane`: every check, then record and generation 1.
/// A failure after the record exists ends it again, so no half-open session is left behind.
pub fn open(
    ctx: &Ctx,
    pane: &str,
    overrides: &Overrides,
    over_cap: bool,
) -> Result<(AssistantSession, Prepared, Option<String>)> {
    open_focused(ctx, pane, overrides, over_cap, None)
}

/// Open an assistant with optional read-only task context.
pub fn open_focused(
    ctx: &Ctx,
    pane: &str,
    overrides: &Overrides,
    over_cap: bool,
    focus: Option<&str>,
) -> Result<(AssistantSession, Prepared, Option<String>)> {
    if let Some(task) = focus {
        ctx.store.get(task)?;
    }
    pane_known(ctx, pane)?;
    let resolved = profile(ctx, None, overrides)?;
    let warning = admit(ctx, over_cap)?;
    let socket = ctx.herdr.socket_path().ok();
    let session = ctx
        .store
        .create_assistant(&resolved.profile, focus, Some(pane), socket)?;
    match prepare(ctx, &session.id, pane, &resolved) {
        Ok(prepared) => {
            ctx.store.log_event(
                None,
                "assistant_launched",
                json!({"session": session.id, "generation": 1, "pane": pane,
                    "profile": resolved.profile, "over_cap": warning.is_some()}),
            )?;
            Ok((session, prepared, warning))
        }
        Err(error) => {
            ctx.store.end_assistant(&session.id)?;
            Err(error)
        }
    }
}

/// `corral pi --resume A-<n>` in `pane`: refuses an ended session or a running host, fences the
/// current generation and opens the next one here on the same Pi session file.
pub fn resume(ctx: &Ctx, id: &str, pane: &str, overrides: &Overrides) -> Result<(u32, Prepared)> {
    corral_core::assistant::validate_id(id).map_err(|e| refuse("usage_error", format!("{e:#}")))?;
    if !ctx.store.assistant_path(id)?.exists() {
        return Err(refuse(
            "assistant_unknown",
            format!("no assistant session {id}"),
        ));
    }
    let session = ctx.store.get_assistant(id)?;
    if let Some(ended) = session.ended {
        return Err(refuse(
            "assistant_ended",
            format!("{id} ended at {ended}; `corral pi` opens a new assistant"),
        ));
    }
    pane_known(ctx, pane)?;
    let container = ctx.store.get_assistant_container(id)?;
    if host_alive(&container.state)? {
        let pid = bound_host(&container.state).map_or(0, |host| host.pid);
        return Err(refuse(
            "assistant_alive",
            format!(
                "{id}'s host (pid {pid}) is running in pane {}; continue there, or end it with \
                 `corral assistant end {id}`",
                container.pane.as_deref().unwrap_or("-")
            ),
        ));
    }
    let resolved = profile(ctx, Some(&session.profile), overrides)?;
    let fenced = container.state.binding.generation;
    let reference = ContainerRef::Assistant { session: id.into() };
    if fenced > 0 {
        fence_generation(&ctx.store, &reference, fenced, "resume")?;
        remove_control(&ctx.store, id, fenced)?;
    }
    let prepared = prepare(ctx, id, pane, &resolved)?;
    ctx.store.log_event(
        None,
        "assistant_resumed",
        json!({"session": id, "fenced": fenced, "generation": fenced + 1, "pane": pane}),
    )?;
    Ok((fenced, prepared))
}

/// A fenced generation's control credential and socket go: nobody reaches it again.
fn remove_control(store: &Store, id: &str, generation: u32) -> Result<()> {
    let dir = launch::assistant_runtime_dir(store, id).join(format!("g{generation}"));
    for name in [CTL_CAP_FILE, SOCKET_FILE] {
        let path = dir.join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("removing {}", path.display()));
            }
        }
    }
    Ok(())
}

/// Ends a session: asks a running host to shut down, closes the record (idempotent) and removes
/// its runtime directory. Transcripts stay. A host that cannot be reached is reported; its
/// generations are fenced either way, so it can no longer change anything.
pub fn end(ctx: &Ctx, id: &str) -> Result<(AssistantSession, Option<String>)> {
    let container = ctx.store.get_assistant_container(id)?;
    let n = container.state.binding.generation;
    let mut warning = None;
    if ctx.store.get_assistant(id)?.ended.is_none() && host_alive(&container.state)? {
        let dir = launch::assistant_runtime_dir(&ctx.store, id).join(format!("g{n}"));
        if let Err(error) = BridgeClient::connect_dir(&dir, n).and_then(|mut c| c.shutdown()) {
            warning = Some(format!(
                "warning: {id}'s host did not take the shutdown request ({error:#}); it is fenced"
            ));
        }
    }
    let session = ctx.store.end_assistant(id)?;
    let dir = launch::assistant_runtime_dir(&ctx.store, id);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("removing {}", dir.display())),
    }
    Ok((session, warning))
}

/// Runs the prepared host as a child in this pane and waits: the checks and launch message of
/// `corral agent launch`, without exec, so the session can end when the host exits cleanly.
#[cfg(unix)]
pub fn run_host(prepared: &Prepared) -> Result<std::process::ExitStatus> {
    let args = prepared.launch_args();
    let bootstrap = launch::consume_bootstrap(&args.bootstrap)?;
    let dir = args
        .bootstrap
        .parent()
        .context("the bootstrap has no directory")?;
    let ready = launch::check(&bootstrap, dir, args.assets.as_deref())?;
    let secrets = launch::Secrets::generate()?;
    launch::write_ctl_cap(&ready.generation_dir, &secrets.ctl_cap)?;
    let (mut command, inherited) = launch::host_command(&bootstrap, &ready, &secrets)?;
    let mut child = command
        .spawn()
        .with_context(|| format!("starting {}", ready.bun.display()))?;
    drop(inherited);
    child.wait().context("waiting for the assistant host")
}

#[cfg(not(unix))]
pub fn run_host(_prepared: &Prepared) -> Result<std::process::ExitStatus> {
    Err(refuse(
        "unsupported_platform",
        "the Pi-native host runs on Unix",
    ))
}

#[cfg(all(test, target_os = "linux"))]
#[path = "assistant_tests.rs"]
mod tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "assistant_c11_tests.rs"]
mod c11_tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "assistant_owner_live_tests.rs"]
mod owner_live_tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "assistant_cowork_live_tests.rs"]
mod cowork_live_tests;
