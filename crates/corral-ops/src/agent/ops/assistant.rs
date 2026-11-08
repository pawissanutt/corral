//! Assistant sessions on the corral-pi host (T-544, member assistant-launch): the soft live cap
//! and machine-load admission, a new session in the owner's pane, generation-fenced resume and
//! host shutdown when a session ends. An assistant takes no coding slot, optionally binds the
//! project it opened in, and follows its linked scratch through generation-fenced restarts;
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
    if !crate::agent::hooks::is_corral_pi(&resolved.harness) {
        let profiles: Vec<&str> = config
            .profiles
            .iter()
            .filter(|(_, profile)| crate::agent::hooks::is_corral_pi(&profile.harness))
            .map(|(name, _)| name.as_str())
            .collect();
        let fix = if profiles.is_empty() {
            "no corral-pi profile exists; add one with `corral config set profiles.<name> ...` or use setup".to_owned()
        } else {
            let preferred = if profiles.contains(&"pi-sol") {
                "pi-sol"
            } else {
                profiles[0]
            };
            format!(
                "run `corral config set assistant.agent {preferred}`; corral-pi profiles: {}",
                profiles.join(", ")
            )
        };
        return Err(refuse(
            "assistant_profile",
            format!(
                "profile '{name}' runs harness '{}', and assistants are corral-pi only; {fix}",
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

/// The project session `id` is bound to; empty for a project-less session.
pub(crate) fn session_project(ctx: &Ctx, id: &str) -> Result<String> {
    Ok(ctx.store.get_assistant(id)?.project.unwrap_or_default())
}

/// Opens session `id`'s next generation in `pane` and writes its one-shot bootstrap.
fn prepare(ctx: &Ctx, id: &str, pane: &str, resolved: &ResolvedProfile) -> Result<Prepared> {
    let _lease = ctx.store.assistant_scratch_lease(id)?;
    let session = ctx.store.get_assistant(id)?;
    let model = resolved
        .model
        .clone()
        .context("an assistant needs a model")?;
    let spec = HostSpec {
        binding: crate::agent::connections::explicit(resolved)?,
        cwd: crate::assistant::directory(&ctx.store, &session)?.path,
        provider: crate::agent::connections::provider(resolved)?,
        model,
        effort: resolved.effort.clone(),
        prompt_file: {
            if session.focus != "workspace" {
                let brief = crate::assistant::task_brief(&ctx.store, &session.focus)?;
                let dir = ctx.store.scratch_spool(
                    corral_core::backend::ScratchKind::TmpStaging,
                    "assistant-focus",
                )?;
                std::fs::create_dir_all(&dir).context("creating assistant focus staging")?;
                let path = dir.join(format!("{id}-focus.md"));
                std::fs::write(&path, brief)
                    .with_context(|| format!("writing {}", path.display()))?;
                Some(path)
            } else {
                None
            }
        },
        adopt: true,
        fresh_session: false,
    };
    let target = BindTarget::Assistant { session: id.into() };
    let prepared = launch::prepare_bootstrap(ctx, &target, pane, &spec)?;
    record_assets(ctx, id, &prepared)?;
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

/// Records on the new generation the asset digest its bootstrap names, so a readback can tell a
/// generation whose contract and tool descriptions predate the installed assets.
fn record_assets(ctx: &Ctx, id: &str, prepared: &Prepared) -> Result<()> {
    let path = &prepared.bootstrap;
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let bootstrap: launch::Bootstrap =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    ctx.store.update_assistant_container(id, |container| {
        let generation = (container.state.generations.iter_mut())
            .find(|generation| generation.n == bootstrap.generation)
            .with_context(|| format!("{id} has no generation {}", bootstrap.generation))?;
        generation.assets_sha256 = Some(bootstrap.asset_sha256.clone());
        Ok(())
    })
}

/// A new assistant session in the owner's pane `pane`: every check, then record and generation 1.
/// A failure after the record exists ends it again, so no half-open session is left behind.
#[cfg(test)]
pub fn open(
    ctx: &Ctx,
    start: &std::path::Path,
    pane: &str,
    overrides: &Overrides,
    over_cap: bool,
) -> Result<(AssistantSession, Prepared, Option<String>)> {
    open_focused(ctx, start, pane, overrides, over_cap, None)
}

/// Open an assistant with optional read-only task context.
#[cfg(test)]
pub fn open_focused(
    ctx: &Ctx,
    start: &std::path::Path,
    pane: &str,
    overrides: &Overrides,
    over_cap: bool,
    focus: Option<&str>,
) -> Result<(AssistantSession, Prepared, Option<String>)> {
    open_claimed(ctx, start, pane, overrides, over_cap, focus)
        .map(|(session, prepared, warning, _)| (session, prepared, warning))
}

/// `open_focused` that also returns this supervisor's claim on the new session. The claim is
/// taken before the record is published, so no concurrent resume can ever see it unclaimed.
pub fn open_claimed(
    ctx: &Ctx,
    start: &std::path::Path,
    pane: &str,
    overrides: &Overrides,
    over_cap: bool,
    focus: Option<&str>,
) -> Result<(AssistantSession, Prepared, Option<String>, Claim)> {
    open_observed(ctx, start, pane, overrides, over_cap, focus, |_| {})
}

/// `observe` runs with the reserved ID after the claim and before publication (test seam).
pub(crate) fn open_observed(
    ctx: &Ctx,
    start: &std::path::Path,
    pane: &str,
    overrides: &Overrides,
    over_cap: bool,
    focus: Option<&str>,
    observe: impl FnOnce(&str),
) -> Result<(AssistantSession, Prepared, Option<String>, Claim)> {
    if let Some(task) = focus {
        ctx.store.get(task)?;
    }
    pane_known(ctx, pane)?;
    let project = crate::assistant::project_at(&ctx.store, start)?;
    let resolved = profile(ctx, None, overrides)?;
    let warning = admit(ctx, over_cap)?;
    let socket = ctx.herdr.socket_path().ok();
    let (created, claim) =
        ctx.store
            .create_assistant_with(&resolved.profile, focus, Some(pane), socket, |id| {
                let claim = claim(&ctx.store, id)?;
                observe(id);
                Ok(claim)
            })?;
    let prepared: Result<_> = (|| {
        let session = ctx.store.update_assistant(&created.id, |session| {
            session.project = project;
            Ok(())
        })?;
        Ok((session, prepare(ctx, &created.id, pane, &resolved)?))
    })();
    match prepared {
        Ok((session, prepared)) => {
            ctx.store.log_event(
                None,
                "assistant_launched",
                json!({"session": session.id, "generation": 1, "pane": pane,
                    "profile": resolved.profile, "over_cap": warning.is_some()}),
            )?;
            Ok((session, prepared, warning, claim))
        }
        Err(error) => {
            // Still claimed by this supervisor: nobody else can own what is ended here.
            ctx.store.end_assistant(&created.id)?;
            Err(error)
        }
    }
}

/// `corral pi --resume A-<n>` in `pane`: refuses an ended session or a running host, fences the
/// current generation and opens the next one here on the same Pi session file.
pub fn resume(ctx: &Ctx, id: &str, pane: &str, overrides: &Overrides) -> Result<(u32, Prepared)> {
    corral_core::assistant::validate_id(id).map_err(|e| refuse("usage_error", format!("{e:#}")))?;
    let _lease = ctx.store.assistant_scratch_lease(id)?;
    if !ctx.store.assistant_exists(id)? {
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
    crate::assistant::workflow::boundary::require_resume(&ctx.store, id, &container)?;
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

/// `/new`, `/resume` and a clean quit leave the outgoing conversation open, but its stopped host
/// has no remaining authority. Only the supervisor calls this, after waiting for the child to exit.
pub(crate) fn suspend(ctx: &Ctx, id: &str, generation: u32, reason: &str) -> Result<()> {
    let container = ctx.store.get_assistant_container(id)?;
    anyhow::ensure!(
        container.state.binding.generation == generation && !host_alive(&container.state)?,
        "assistant_changed: {id} no longer has the stopped generation {generation}"
    );
    fence_generation(
        &ctx.store,
        &ContainerRef::Assistant { session: id.into() },
        generation,
        reason,
    )?;
    remove_control(&ctx.store, id, generation)?;
    ctx.store.log_event(
        None,
        "assistant_suspended",
        json!({"session": id, "generation": generation, "reason": reason}),
    )?;
    Ok(())
}

/// A supervisor's exclusive claim on one session: while it is held no other `corral pi` can open
/// a generation of that session, so a pending launch that has not bound yet cannot be stolen.
/// The kernel releases the lock when the supervisor exits, so a crash never strands a session.
#[derive(Debug)]
pub struct Claim {
    pub id: String,
    _file: std::fs::File,
}

/// Claims session `id` for this supervisor, or refuses `assistant_claimed` when another holds it.
pub fn claim(store: &Store, id: &str) -> Result<Claim> {
    corral_core::assistant::validate_id(id).map_err(|e| refuse("usage_error", format!("{e:#}")))?;
    let dir = launch::runtime_root()
        .join(corral_core::agent::paths::store_id(store))
        .join("assistant-claims");
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .with_context(|| format!("creating {}", dir.display()))?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(format!("{id}.lock"));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Claim {
            id: id.into(),
            _file: file,
        }),
        Err(std::fs::TryLockError::WouldBlock) => Err(refuse(
            "assistant_claimed",
            format!(
                "another `corral pi` is opening or running {id}; continue in its pane, or retry \
                 once it exits"
            ),
        )),
        Err(std::fs::TryLockError::Error(error)) => {
            Err(error).with_context(|| format!("locking {}", path.display()))
        }
    }
}

/// The file a host writes in its generation directory to ask its supervisor for `/resume`.
pub const SWITCH_REQUEST_FILE: &str = "resume-request.json";
/// The notice a supervisor leaves for the next generation's host to show its owner once.
pub const SWITCH_NOTICE_FILE: &str = "resume-notice.txt";

/// Leaves `text` beside `prepared`'s bootstrap, in the generation directory its host reads.
pub(crate) fn write_switch_notice(prepared: &Prepared, text: &str) -> Result<()> {
    let dir = (prepared.bootstrap.parent()).context("the bootstrap has no directory")?;
    let path = dir.join(SWITCH_NOTICE_FILE);
    let text: String = text.chars().take(2048).collect();
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SwitchRequest {
    version: u32,
    assistant: String,
    generation: u32,
    target: String,
}

/// Takes (reads and removes) generation `generation`'s switch request: the target session ID.
/// Only a request naming this very session and generation counts; it is a request, not authority.
pub(crate) fn take_switch_request(ctx: &Ctx, id: &str, generation: u32) -> Result<String> {
    let path = launch::assistant_runtime_dir(&ctx.store, id)
        .join(format!("g{generation}"))
        .join(SWITCH_REQUEST_FILE);
    let bytes = std::fs::read(&path).map_err(|error| {
        refuse(
            "switch_request_missing",
            format!("{id}'s host asked to resume another session but left no request ({error})"),
        )
    })?;
    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    let request: SwitchRequest = serde_json::from_slice(&bytes)
        .map_err(|_| refuse("switch_request_invalid", "the resume request is malformed"))?;
    if request.version != 1 || request.assistant != id || request.generation != generation {
        return Err(refuse(
            "switch_request_invalid",
            format!("the resume request does not name {id} generation {generation}"),
        ));
    }
    corral_core::assistant::validate_id(&request.target)
        .map_err(|e| refuse("usage_error", format!("{e:#}")))?;
    if request.target == id {
        return Err(refuse(
            "assistant_current",
            format!("{id} is the current session"),
        ));
    }
    Ok(request.target)
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
    if let Err(error) = crate::assistant::permissions::drain_permission_audit(&ctx.store, id) {
        let queued =
            format!("warning: {id}'s write-permission audit lines stay queued ({error:#})");
        warning = Some(match warning {
            Some(first) => format!("{first}\n{queued}"),
            None => queued,
        });
    }
    let dir = launch::assistant_runtime_dir(&ctx.store, id);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("removing {}", dir.display())),
    }
    Ok((session, warning))
}

/// Revalidate a prepared generation under the admission lease, including the unbound interval.
/// GC may fence an abandoned bootstrap; it must never launch into a removed scratch later.
pub(crate) fn startup_lease(
    bootstrap: &launch::Bootstrap,
) -> Result<Option<corral_core::StoreLock>> {
    let Some(id) = bootstrap.assistant.as_deref() else {
        return Ok(None);
    };
    let store = Store::open(&bootstrap.root)?;
    let lease = store.assistant_scratch_lease(id)?;
    store.inspect_assistant_container(id, |container, ended| {
        anyhow::ensure!(
            !ended
                && !container.state.binding.closing
                && container.state.binding.generation == bootstrap.generation
                && container
                    .state
                    .current_generation()
                    .is_some_and(|g| { g.fenced.is_none() && g.bound.is_none() }),
            "assistant_changed: {id}'s prepared generation is no longer launchable"
        );
        Ok(())
    })?;
    Ok(Some(lease))
}

/// Runs the prepared host as a child in this pane and waits: the checks and launch message of
/// `corral agent launch`, without exec, so the supervisor can suspend the stopped host.
#[cfg(unix)]
pub fn run_host(prepared: &Prepared) -> Result<std::process::ExitStatus> {
    let args = prepared.launch_args();
    let bootstrap = launch::consume_bootstrap(&args.bootstrap)?;
    let _lease = startup_lease(&bootstrap)?;
    let dir = args
        .bootstrap
        .parent()
        .context("the bootstrap has no directory")?;
    let ready = launch::check(&bootstrap, dir, args.assets.as_deref())?;
    let secrets = launch::Secrets::generate()?;
    launch::write_ctl_cap(&ready.generation_dir, &secrets.ctl_cap)?;
    let (mut command, inherited) = launch::host_command(&bootstrap, &ready, &secrets)?;
    // This child has a `corral pi` supervisor handling /new (75) and idle scratch relink (76).
    command.arg("--assistant-new");
    wait_host(command, inherited)
}

#[cfg(unix)]
pub(crate) fn wait_host_checked(
    command: std::process::Command,
    inherited: std::io::PipeReader,
) -> Result<()> {
    anyhow::ensure!(
        wait_host(command, inherited)?.success(),
        "assistant host failed"
    );
    Ok(())
}

#[cfg(unix)]
pub(crate) fn wait_host(
    mut command: std::process::Command,
    inherited: std::io::PipeReader,
) -> Result<std::process::ExitStatus> {
    let mut child = command.spawn().context("starting assistant host")?;
    drop(inherited);
    child.wait().context("waiting for the assistant host")
}

#[cfg(not(unix))]
pub fn run_host(_prepared: &Prepared) -> Result<std::process::ExitStatus> {
    Err(refuse(
        "unsupported_platform",
        "the corral-pi host runs on Unix",
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
