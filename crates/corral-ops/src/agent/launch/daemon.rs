//! Pane-less assistant launches for a daemon supervisor. Admission, profile/credential
//! preflight, scratch leasing, focus prompts, transcript continuation and generation fencing
//! are shared with `corral pi`; only terminal ownership and child I/O differ. The caller must
//! retain the returned claim until its child exits, then call `ops::assistant::suspend`.
use std::path::Path;

use anyhow::{Context, Result, ensure};
use corral_core::agent::ContainerRef;
use corral_core::assistant::AssistantSession;
use serde_json::json;

use super::Prepared;
use crate::Ctx;
use crate::agent::admission::fence_generation;
use crate::agent::hooks::pm::Overrides;
use crate::agent::ops::assistant::{self, Claim};

/// Preflight, reserve and claim a new focused session before publishing it. No herdr pane
/// lookup or socket is needed; an unavailable provider fails before creating a session.
pub fn open_claimed(
    ctx: &Ctx,
    start: &Path,
    overrides: &Overrides,
    over_cap: bool,
    focus: Option<&str>,
) -> Result<(AssistantSession, Prepared, Option<String>, Claim)> {
    if let Some(task) = focus {
        ctx.store.get(task)?;
    }
    let project = crate::assistant::project_at(&ctx.store, start)?;
    let resolved = assistant::profile(ctx, None, overrides)?;
    let warning = assistant::admit(ctx, over_cap)?;
    let (created, claim) =
        ctx.store
            .create_assistant_with(&resolved.profile, focus, None, None, |id| {
                assistant::claim(&ctx.store, id)
            })?;
    let result: Result<_> = (|| {
        let session = ctx.store.update_assistant(&created.id, |session| {
            session.project = project;
            Ok(())
        })?;
        let prepared = assistant::prepare(ctx, &session.id, None, &resolved)?;
        ctx.store.log_event(
            None,
            "assistant_launched",
            json!({
                "session": session.id, "generation": 1, "daemon_hosted": true,
                "profile": resolved.profile, "over_cap": warning.is_some(),
            }),
        )?;
        Ok((session, prepared))
    })();
    match result {
        Ok((session, prepared)) => Ok((session, prepared, warning, claim)),
        Err(error) => {
            ctx.store.end_assistant(&created.id)?;
            Err(error)
        }
    }
}

/// Claim and resume an existing session as a daemon host, without stealing a terminal host
/// or an unbound launch. Ended sessions cannot be revived. A profile change is resolved by
/// the caller through the canonical session service before resuming.
pub fn resume_claimed(ctx: &Ctx, id: &str, overrides: &Overrides) -> Result<(Prepared, Claim)> {
    let claim = assistant::claim(&ctx.store, id)?;
    let _lease = ctx.store.assistant_scratch_lease(id)?;
    let session = ctx.store.get_assistant(id)?;
    ensure!(session.ended.is_none(), "assistant_ended: {id} has ended");
    let container = ctx.store.get_assistant_container(id)?;
    crate::assistant::workflow::boundary::require_resume(&ctx.store, id, &container)?;
    ensure!(
        !crate::agent::pi::host_alive(&container.state)?,
        "assistant_alive: {id} is running"
    );
    let resolved = if let Some(binding) = container
        .state
        .current_generation()
        .and_then(|g| g.connection.as_ref())
        .filter(|b| b.connection.api_key())
    {
        let config = ctx.store.config_layers()?.config()?;
        let candidate = config.profiles.get(&session.profile).and_then(|p| p.metadata.as_ref())
            .and_then(|m| m.candidates.iter().find(|c| Some(&c.id) == binding.candidate.as_ref()))
            .context("PolicyNotApproved: frozen API candidate missing; start a new session to change routes")?;
        let mut resolved = config.resolve_candidate(&session.profile, candidate)?;
        crate::agent::hooks::pm::apply(&mut resolved, overrides)?;
        ensure!(
            resolved.connection_binding()?.as_ref() == Some(binding),
            "paid route is sticky; start a new session to change connection/model"
        );
        crate::agent::hooks::preflight::check(
            ctx,
            corral_core::agent::AgentRole::Assistant,
            &resolved,
        )?;
        resolved
    } else {
        assistant::profile(ctx, Some(&session.profile), overrides)?
    };
    assistant::admit(ctx, false)?;
    let previous = container.state.binding.generation;
    let result: Result<_> = (|| {
        if previous > 0 && container.state.paid_episode.is_some() {
            fence_generation(
                &ctx.store,
                &ContainerRef::Assistant { session: id.into() },
                previous,
                "paid daemon recovery",
            )?;
        }
        let prepared = assistant::prepare(ctx, id, None, &resolved)?;
        if previous > 0 {
            assistant::remove_control(&ctx.store, id, previous)?;
        }
        ctx.store.log_event(None, "assistant_resumed", json!({
            "session": id, "fenced": previous, "generation": previous + 1, "daemon_hosted": true,
        }))?;
        Ok(prepared)
    })();
    match result {
        Ok(prepared) => Ok((prepared, claim)),
        Err(error) => {
            let current = ctx
                .store
                .get_assistant_container(id)?
                .state
                .binding
                .generation;
            if current > previous {
                fence_generation(
                    &ctx.store,
                    &ContainerRef::Assistant { session: id.into() },
                    current,
                    "daemon prepare failed",
                )?;
                assistant::remove_control(&ctx.store, id, current)?;
            }
            Err(error)
        }
    }
}

/// The supervisor owns the child, and this handle keeps its scratch lease through the
/// unbound interval and the host lifetime. Reap the child before releasing this handle.
#[cfg(unix)]
pub struct DaemonHost {
    pub child: std::process::Child,
    pub session: String,
    pub generation: u32,
    _lease: Option<corral_core::StoreLock>,
    diagnostics: std::sync::Arc<std::sync::Mutex<Diagnostics>>,
}

#[cfg(unix)]
#[derive(Default)]
struct Diagnostics {
    tail: std::collections::VecDeque<u8>,
    complete: bool,
}

#[cfg(unix)]
impl Diagnostics {
    fn append(&mut self, bytes: &[u8]) {
        const MAX: usize = 8 * 1024;
        let retained = MAX.saturating_sub(bytes.len());
        if self.tail.len() > retained {
            self.tail.drain(..self.tail.len() - retained);
        }
        self.tail
            .extend(bytes.iter().rev().take(MAX).rev().copied());
    }
}

#[cfg(unix)]
impl DaemonHost {
    /// Actual bounded host stderr, including failures before the bridge exists.
    pub fn diagnostics(&self) -> String {
        let mut captured = self
            .diagnostics
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        String::from_utf8_lossy(captured.tail.make_contiguous()).into_owned()
    }

    /// EOF has been drained: after child exit the complete startup failure is available.
    pub fn diagnostics_complete(&self) -> bool {
        self.diagnostics
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .complete
    }
}

#[cfg(unix)]
fn capture_stderr(
    mut stderr: std::process::ChildStderr,
) -> Result<std::sync::Arc<std::sync::Mutex<Diagnostics>>> {
    use std::io::Read;
    let captured = std::sync::Arc::new(std::sync::Mutex::new(Diagnostics::default()));
    let target = std::sync::Arc::clone(&captured);
    std::thread::Builder::new()
        .name("assistant-stderr".into())
        .spawn(move || {
            let mut bytes = [0; 4096];
            loop {
                let read = stderr.read(&mut bytes);
                if matches!(&read, Err(error) if error.kind() == std::io::ErrorKind::Interrupted) {
                    continue;
                }
                let mut captured = target.lock().unwrap_or_else(|error| error.into_inner());
                match read {
                    Ok(0) => {
                        captured.complete = true;
                        break;
                    }
                    Ok(count) => captured.append(&bytes[..count]),
                    Err(error) => {
                        captured.append(format!("\nreading host stderr: {error}").as_bytes());
                        captured.complete = true;
                        break;
                    }
                }
            }
        })
        .context("starting bounded assistant stderr drain")?;
    Ok(captured)
}

/// Consume a prepared daemon bootstrap and spawn by argv with no terminal or secret in the
/// environment/argv. Stdin/stdout are null; stderr drains asynchronously into an 8 KiB tail,
/// retaining actual setup/provider failures even before the host bridge exists.
#[cfg(unix)]
pub fn spawn(prepared: &Prepared) -> Result<DaemonHost> {
    use std::process::Stdio;
    let bootstrap = super::consume_bootstrap(&prepared.bootstrap)?;
    ensure!(
        bootstrap.daemon_hosted && bootstrap.role == corral_core::agent::AgentRole::Assistant,
        "only a pane-less assistant bootstrap can be spawned by the daemon"
    );
    let session = bootstrap
        .assistant
        .clone()
        .context("assistant bootstrap lacks session")?;
    let generation = bootstrap.generation;
    let lease = assistant::startup_lease(&bootstrap)?;
    let result = (|| {
        let dir = prepared
            .bootstrap
            .parent()
            .context("bootstrap directory missing")?;
        let ready = super::check(&bootstrap, dir, prepared.assets.as_deref())?;
        let secrets = super::Secrets::generate()?;
        super::write_ctl_cap(&ready.generation_dir, &secrets.ctl_cap)?;
        let (mut command, inherited) = super::host_command(&bootstrap, &ready, &secrets)?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command.spawn().context("starting daemon assistant host")?;
        drop(inherited);
        let stderr = child
            .stderr
            .take()
            .context("assistant stderr pipe missing")?;
        let diagnostics = match capture_stderr(stderr) {
            Ok(captured) => captured,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        Ok(DaemonHost {
            child,
            session: session.clone(),
            generation,
            _lease: lease,
            diagnostics,
        })
    })();
    if result.is_err() {
        let store = corral_core::Store::open(&bootstrap.root)?;
        fence_generation(
            &store,
            &ContainerRef::Assistant { session },
            generation,
            "daemon launch failed",
        )?;
        let dir = prepared
            .bootstrap
            .parent()
            .context("bootstrap directory missing")?;
        for name in [super::CTL_CAP_FILE, super::SOCKET_FILE] {
            match std::fs::remove_file(dir.join(name)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).context("removing failed assistant control endpoint");
                }
            }
        }
    }
    result
}
