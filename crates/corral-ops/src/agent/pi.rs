//! `corral pi`: start the corral-pi host from nothing (T-490), as `pi` starts plain Pi.
//!
//! In a workspace `corral pi --assistant` (or the launcher's Assistant row) opens a new assistant
//! session in the owner's herdr pane and `--resume A-<n>` continues one there (T-544); both run the
//! host as a child so a clean exit suspends the session for later resume. `--pm` likewise
//! supervises its bound child for generation-fenced profile changes.
//!
//! Bound mode adopts the herdr pane it runs in as the PM pane. It resolves the PM profile through
//! the `corral pm` and resume checks, opens the next PM generation in that pane (taking over a dead
//! PM with resume's fencing, never a rival generation), writes the one-shot bootstrap and launches
//! a supervised child through the same consume and check path as `corral agent launch`.
//! The adopted pane is recorded (`AgentPm::adopted_pane`): corral never
//! closes it, and types into it only after verifying its live host.
//!
//! `--bare` runs the build host without any corral workspace binding or state commands.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::agent::{AgentRunState, ContainerRef, pm_state};

use super::bridge::liveness::{bound_host, gone};
use super::hooks::pm::Overrides;
use super::launch::Prepared;
use super::ops::assistant::Claim;
use crate::Ctx;

#[path = "pi_profile.rs"]
pub mod profile;

/// The variable herdr sets in every pane: the pane bound mode adopts.
pub const PANE_ENV: &str = "HERDR_PANE_ID";
pub const DEFAULT_PROVIDER: &str = "openai-codex";

/// A `corral pi` refusal before anything is opened or executed: a stable code and the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiRefusal {
    pub code: &'static str,
    pub detail: String,
}

impl std::fmt::Display for PiRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "corral pi refused: {}: {}", self.code, self.detail)
    }
}

impl std::error::Error for PiRefusal {}

pub(crate) fn refuse(code: &'static str, detail: impl Into<String>) -> anyhow::Error {
    PiRefusal {
        code,
        detail: detail.into(),
    }
    .into()
}

pub(crate) fn platform() -> Result<()> {
    if cfg!(unix) {
        Ok(())
    } else {
        Err(refuse(
            "unsupported_platform",
            "the corral-pi host runs on Unix",
        ))
    }
}

/// The pane bound mode adopts, from `HERDR_PANE_ID`.
pub fn current_pane(value: Option<&str>) -> Result<String> {
    value
        .filter(|pane| !pane.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            refuse(
                "not_in_herdr",
                "bound mode adopts the herdr pane it runs in, and HERDR_PANE_ID is unset; run \
                 `corral pi` in a herdr pane, or `corral pi --bare` for plain unbound Pi",
            )
        })
}

pub(crate) fn host_alive(state: &AgentRunState) -> Result<bool> {
    Ok(bound_host(state).as_ref().map(gone).transpose()? == Some(false))
}

/// True when `pane` is the saved PM pane `corral pi` adopted.
pub(crate) fn adopted(ctx: &Ctx, container: &ContainerRef, pane: &str) -> Result<bool> {
    Ok(matches!(container, ContainerRef::Pm)
        && pm_state::load_current(&ctx.store)?.is_some_and(|pm| pm.adopted() && pm.pane == pane))
}

/// For the adopted PM pane, whether its bound host is verifiably running; `None` for any other
/// pane. An adopted pane is the owner's, so only a live host makes it the PM's.
pub(crate) fn adopted_alive(ctx: &Ctx, pane: &str) -> Result<Option<bool>> {
    match pm_state::load_current(&ctx.store)? {
        Some(pm) if pm.adopted() && pm.pane == pane => host_alive(&pm.state).map(Some),
        _ => Ok(None),
    }
}

/// Recovery never types into an adopted pane whose host is gone: the shell there is the owner's.
pub(crate) fn refuse_dead_adopted(adopted: bool, alive: bool, pane: &str) -> Result<()> {
    anyhow::ensure!(
        !adopted || alive,
        "pane_adopted: {pane} is the owner's own pane adopted by `corral pi` and its host is gone; \
         corral never types into it. Run `corral pi` in that pane, or `corral pm`"
    );
    Ok(())
}

/// Bound mode up to child launch: every check, then generation n+1 in `pane` and its bootstrap.
pub fn adopt(ctx: &Ctx, pane: &str, overrides: &Overrides) -> Result<Prepared> {
    platform()?;
    let current = pm_state::load_current(&ctx.store)?;
    let resolved = if let Some(pm) = current.as_ref()
        && profile::lineage(ctx, &ContainerRef::Pm, &pm.state)?
    {
        let retained = profile::pm_current(ctx)?;
        anyhow::ensure!(
            overrides
                .profile
                .as_ref()
                .is_none_or(|name| name == &retained)
                && overrides.model.is_none()
                && overrides.provider.is_none()
                && overrides.effort.is_none(),
            "paid route is sticky; start a new session to change profiles"
        );
        anyhow::ensure!(
            profile::paid(&pm.state),
            "paid PM preparation remains unresolved; resume its original paid binding explicitly"
        );
        crate::pm::continuation::recovery_profile(ctx, &pm.state)?
    } else {
        super::hooks::pm::profile_with(ctx, overrides)?
    };
    if ctx.herdr.pane_get(pane)?.is_none() {
        return Err(refuse(
            "pane_unknown",
            format!("{pane} is not a pane of this store's herdr server"),
        ));
    }
    crate::pm::adopt_pm(ctx, pane, &resolved, || {
        let bound =
            pm_state::load_current(&ctx.store)?.filter(|pm| pm.state.binding.generation > 0);
        // The adoption is recorded only with the generation move, so a refusal changes nothing.
        match bound {
            // A dead PM is taken over with resume's fencing, drain and notice.
            Some(_) => Ok(super::recovery::recover(
                ctx,
                &ContainerRef::Pm,
                false,
                Some((pane, &resolved, overrides.continue_session)),
            )?
            .1),
            None => {
                super::hooks::pm::prepared(ctx, pane, &resolved, true, overrides.continue_session)
            }
        }
    })
}

/// `corral pi [--profile NAME]`: adopt this pane and supervise the PM host.
pub fn bound(_start: &Path, ctx: &Ctx, overrides: &Overrides) -> Result<()> {
    platform()?;
    let pane = current_pane(std::env::var(PANE_ENV).ok().as_deref())?;
    let prepared = adopt(ctx, &pane, overrides)?;
    profile::supervise_pm(ctx, &pane, prepared, super::ops::assistant::run_host)
}

/// `corral pi [--over-cap]` in a workspace: a new assistant session in this pane.
pub fn assistant(start: &Path, ctx: &Ctx, overrides: &Overrides, over_cap: bool) -> Result<()> {
    assistant_focused(start, ctx, overrides, over_cap, None)
}

/// Task focus supplies context only; admission and authority are unchanged.
pub fn assistant_focused(
    start: &Path,
    ctx: &Ctx,
    overrides: &Overrides,
    over_cap: bool,
    focus: Option<&str>,
) -> Result<()> {
    platform()?;
    let pane = current_pane(std::env::var(PANE_ENV).ok().as_deref())?;
    // Claimed before the record is published, so no concurrent resume can take it.
    let (session, prepared, warning, claim) =
        super::ops::assistant::open_claimed(ctx, start, &pane, overrides, over_cap, focus)?;
    if let Some(warning) = warning {
        eprintln!("corral pi: {warning}");
    }
    eprintln!(
        "corral pi: assistant {} (role assistant) in pane {pane}; `corral pi --pm` opens the PM",
        session.id
    );
    supervise_assistant(ctx, &pane, claim, prepared, overrides, over_cap)
}

/// `corral pi --resume A-<n>`: claim the session, fence its generation and open the next here.
pub fn resume_assistant(ctx: &Ctx, id: &str, overrides: &Overrides) -> Result<()> {
    platform()?;
    let pane = current_pane(std::env::var(PANE_ENV).ok().as_deref())?;
    let claim = super::ops::assistant::claim(&ctx.store, id)?;
    let (fenced, prepared) = super::ops::assistant::resume(ctx, id, &pane, overrides)?;
    eprintln!(
        "corral pi: assistant {id} resumed in pane {pane} (generation {fenced} fenced, \
         generation {} opened)",
        fenced + 1
    );
    supervise_assistant(ctx, &pane, claim, prepared, overrides, false)
}

/// Matched by the managed host's /new handoff; ordinary quit still exits 0.
const NEW_ASSISTANT_EXIT: i32 = 75;
/// The managed host settled its turn and requests the same conversation in its linked directory.
const RELINK_ASSISTANT_EXIT: i32 = 76;
/// The managed host's `/resume` picker chose another session (named in its switch request).
const RESUME_ASSISTANT_EXIT: i32 = 77;
/// Only native quota evidence and the drained boundary authorize this request.
pub(crate) const CONTINUE_ASSISTANT_EXIT: i32 = 78;

fn supervise_assistant(
    ctx: &Ctx,
    pane: &str,
    claim: Claim,
    prepared: Prepared,
    overrides: &Overrides,
    over_cap: bool,
) -> Result<()> {
    supervise_with(
        ctx,
        pane,
        claim,
        prepared,
        overrides,
        over_cap,
        super::ops::assistant::run_host,
    )
}

/// /new, /resume and relink happen between hosts, never by replacing the bound Pi file
/// in-process. The supervisor holds the claim of the session it runs for the host's lifetime.
pub(crate) fn supervise_with(
    ctx: &Ctx,
    pane: &str,
    mut claim: Claim,
    mut prepared: Prepared,
    overrides: &Overrides,
    over_cap: bool,
    mut run: impl FnMut(&Prepared) -> Result<std::process::ExitStatus>,
) -> Result<()> {
    use super::ops::assistant as ops;
    // Launch overrides belong to the sessions this command opened; a session selected with
    // /resume runs on its own recorded profile.
    let mut opened = vec![claim.id.clone()];
    let defaults = Overrides::default();
    let overrides_for = |opened: &[String], id: &str| -> Overrides {
        if opened.iter().any(|o| o == id) {
            overrides.clone()
        } else {
            defaults.clone()
        }
    };
    // The session a /resume switched away from, while its target has not started yet.
    let mut previous: Option<String> = None;
    loop {
        let id = claim.id.clone();
        let (generation, paid) = {
            let container = ctx.store.get_assistant_container(&id)?;
            let paid = container.state.paid_episode.is_some()
                || container
                    .state
                    .current_generation()
                    .and_then(|g| g.connection.as_ref())
                    .is_some_and(|binding| binding.connection.api_key());
            (container.state.binding.generation, paid)
        };
        let status = match run(&prepared) {
            Ok(status) => status,
            Err(error) => {
                if paid {
                    crate::assistant::continuation::failed_start(ctx, &id, generation)?;
                }
                let Some(source) = previous.take() else {
                    // A fresh host that could not start is not left half-open. A resumed
                    // conversation remains recoverable, including when /new failed while
                    // shutting it down.
                    if generation == 1 && !paid {
                        ctx.store.end_assistant(&id)?;
                    }
                    return Err(error);
                };
                // A selected session that could not start stays resumable; the owner returns.
                let text = format!(
                    "corral: {id} could not start ({error:#}); it stays resumable with /resume \
                     or `corral pi --resume {id}`. Returned to {source}."
                );
                let returned = ops::claim(&ctx.store, &source).and_then(|next| {
                    let (_, next_prepared) =
                        ops::resume(ctx, &source, pane, &overrides_for(&opened, &source))?;
                    Ok((next, next_prepared))
                });
                let (next, next_prepared) = returned.with_context(|| {
                    format!(
                        "{id} could not start and {source} could not be resumed; \
                         `corral pi --resume {source}` or `corral pi --resume {id}` resumes one"
                    )
                })?;
                ops::write_switch_notice(&next_prepared, &text)?;
                eprintln!("{text}");
                claim = next;
                prepared = next_prepared;
                continue;
            }
        };
        previous = None;
        if paid {
            crate::assistant::continuation::failed_start(ctx, &id, generation)?;
        }
        match status.code() {
            Some(profile::PROFILE_EXIT) => {
                match profile::switch_assistant(
                    ctx,
                    pane,
                    &id,
                    generation,
                    &prepared,
                    &overrides_for(&opened, &id),
                    over_cap,
                )? {
                    profile::AssistantSwitch::Resumed(next) => prepared = next,
                    profile::AssistantSwitch::New {
                        claim: next,
                        prepared: next_prepared,
                    } => {
                        claim = next;
                        prepared = next_prepared;
                    }
                }
            }
            Some(CONTINUE_ASSISTANT_EXIT) => {
                let Some(next) =
                    crate::assistant::continuation::resume(ctx, &id, pane, generation)?
                else {
                    return Ok(());
                };
                prepared = next;
            }
            Some(RELINK_ASSISTANT_EXIT) => {
                anyhow::ensure!(
                    ctx.store
                        .get_assistant_container(&id)?
                        .state
                        .binding
                        .generation
                        == generation,
                    "assistant_changed: {id} no longer has the stopped generation {generation}"
                );
                let (_, next_prepared) = ops::resume(ctx, &id, pane, &overrides_for(&opened, &id))?;
                ctx.store.log_event(None, "assistant_relinked", serde_json::json!({
                    "session": id, "fenced": generation, "generation": generation + 1, "pane": pane,
                    "directory": crate::assistant::directory(&ctx.store, &ctx.store.get_assistant(&id)?)?,
                }))?;
                prepared = next_prepared;
            }
            Some(RESUME_ASSISTANT_EXIT) => {
                let target = ops::take_switch_request(ctx, &id, generation);
                ops::suspend(ctx, &id, generation, "resume another assistant")?;
                // Eligibility is decided here, at selection time, under the target's claim.
                let switched = target.and_then(|target| {
                    let next = ops::claim(&ctx.store, &target)?;
                    let (fenced, next_prepared) =
                        ops::resume(ctx, &target, pane, &overrides_for(&opened, &target))?;
                    Ok((next, next_prepared, fenced))
                });
                match switched {
                    Ok((next, next_prepared, fenced)) => {
                        ctx.store.log_event(
                            None,
                            "assistant_switched",
                            serde_json::json!({"from": id, "from_generation": generation,
                                "to": next.id, "fenced": fenced, "generation": fenced + 1, "pane": pane}),
                        )?;
                        eprintln!(
                            "corral pi: assistant {id} suspended; resumed {} in pane {pane}",
                            next.id
                        );
                        previous = Some(id);
                        claim = next;
                        prepared = next_prepared;
                    }
                    Err(error) => {
                        // The source's claim was never released: return to it unchanged.
                        let text =
                            format!("corral: /resume did not switch: {error:#}. Continuing {id}.");
                        let (_, next_prepared) =
                            ops::resume(ctx, &id, pane, &overrides_for(&opened, &id))
                                .with_context(|| {
                                    format!("{text} {id} could not be resumed; `corral pi --resume {id}` resumes it")
                                })?;
                        ops::write_switch_notice(&next_prepared, &text)?;
                        eprintln!("{text}");
                        prepared = next_prepared;
                    }
                }
            }
            Some(NEW_ASSISTANT_EXIT) => {
                ops::suspend(ctx, &id, generation, "new assistant")?;
                eprintln!(
                    "corral pi: assistant {id} suspended; `corral pi --resume {id}` resumes it"
                );
                // Admission runs again after the old child stopped, so /new also works at the live cap.
                let directory =
                    crate::assistant::directory(&ctx.store, &ctx.store.get_assistant(&id)?)?;
                let (next, next_prepared, warning, next_claim) =
                    ops::open_claimed(ctx, &directory.path, pane, overrides, over_cap, None)
                        .with_context(|| {
                            format!(
                                "/new could not open an assistant; resume {id} with `corral pi --resume {id}`"
                            )
                        })?;
                if let Some(warning) = warning {
                    eprintln!("corral pi: {warning}");
                }
                eprintln!(
                    "corral pi: assistant {} (role assistant) in pane {pane}",
                    next.id
                );
                opened.push(next.id);
                claim = next_claim;
                prepared = next_prepared;
            }
            _ => return finish(ctx, &id, generation, status),
        }
    }
}

/// Any host exit leaves a conversation returnable: a clean quit (Pi's `/quit`) suspends it, a
/// crash leaves it resumable (a never-prompted one resumes on a fresh file). Only
/// `corral assistant end` is the explicit, permanent end.
fn finish(ctx: &Ctx, id: &str, generation: u32, status: std::process::ExitStatus) -> Result<()> {
    if !status.success() {
        #[cfg(unix)]
        restore_terminal()?;
        anyhow::bail!("{id}'s host stopped ({status}); `corral pi --resume {id}` resumes it");
    }
    if ctx.store.get_assistant(id)?.ended.is_some() {
        eprintln!("corral pi: assistant {id} ended");
        return Ok(());
    }
    if let Err(error) = super::ops::assistant::suspend(ctx, id, generation, "quit") {
        // `corral assistant end` may have closed it while the host shut down.
        if ctx.store.get_assistant(id)?.ended.is_some() {
            eprintln!("corral pi: assistant {id} ended");
            return Ok(());
        }
        return Err(error);
    }
    eprintln!(
        "corral pi: assistant {id} suspended; `corral pi --resume {id}` or /resume in another \
         assistant returns to it; `corral assistant end {id}` ends it"
    );
    Ok(())
}

/// Pi's TUI turns on mouse, focus, paste and keyboard reporting and hides the cursor. A host that
/// is killed cannot turn them off, so the owner's shell would read the reports as typed input.
const TERMINAL_RESET: &[u8] = b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1004l\x1b[?2004l\
    \x1b[?2031l\x1b[<u\x1b[?7h\x1b[?25h\r\n";

#[cfg(unix)]
fn restore_terminal() -> Result<()> {
    use std::io::{IsTerminal, Write};
    let mut stderr = std::io::stderr();
    if stderr.is_terminal() {
        stderr
            .write_all(TERMINAL_RESET)
            .and_then(|()| stderr.flush())
            .context("restoring the terminal after the host stopped")?;
    }
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        rustix::termios::tcflush(&stdin, rustix::termios::QueueSelector::IFlush)
            .context("discarding the stopped host's stray terminal reports")?;
    }
    Ok(())
}

/// `corral assistant end`: shut a running host down, then close the session record.
pub fn end_assistant(ctx: &Ctx, id: &str) -> Result<corral_core::assistant::AssistantSession> {
    let (session, warning) = super::ops::assistant::end(ctx, id)?;
    if let Some(warning) = warning {
        eprintln!("{warning}");
    }
    Ok(session)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BareArgs {
    pub profile: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub effort: Option<String>,
    pub here: bool,
    pub name: Option<String>,
    pub resume: Option<String>,
}

/// Prepare a bare build launch without discovering or reading any corral workspace.
#[cfg(unix)]
pub fn bare_command(
    data_root: &Path,
    args: &BareArgs,
    cwd: &Path,
    bun: Option<PathBuf>,
) -> Result<(std::process::Command, std::io::PipeReader)> {
    super::pi_build::bare_build_command(
        data_root,
        &super::pi_build::BuildArgs {
            profile: args.profile.clone(),
            model: args.model.clone(),
            provider: args.provider.clone(),
            effort: args.effort.clone(),
            here: args.here,
            name: args.name.clone(),
            resume: args.resume.clone(),
        },
        cwd,
        bun,
    )
}

/// `corral pi --bare`: build in Git, or the current directory outside a repository.
#[cfg(unix)]
pub fn bare(args: &BareArgs) -> Result<()> {
    super::pi_build::bare(&super::pi_build::BuildArgs {
        profile: args.profile.clone(),
        model: args.model.clone(),
        provider: args.provider.clone(),
        effort: args.effort.clone(),
        here: args.here,
        name: args.name.clone(),
        resume: args.resume.clone(),
    })
}

#[cfg(not(unix))]
pub fn bare(_args: &BareArgs) -> Result<()> {
    platform()
}

#[cfg(all(test, target_os = "linux"))]
#[path = "pi_tests.rs"]
mod tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "pi_assistant_tests.rs"]
mod assistant_tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "pi_switch_tests.rs"]
mod switch_tests;
