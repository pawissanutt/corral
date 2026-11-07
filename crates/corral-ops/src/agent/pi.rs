//! `corral pi`: start the corral-pi host from nothing (T-490), as `pi` starts plain Pi.
//!
//! In a workspace `corral pi` opens a new assistant session in the owner's herdr pane and
//! `--resume A-<n>` continues one there (T-544); both run the host as a child so a clean exit
//! suspends the session for later resume. `--pm` is the PM's bound mode below, unchanged.
//!
//! Bound mode adopts the herdr pane it runs in as the PM pane. It resolves the PM profile through
//! the `corral pm` and resume checks, opens the next PM generation in that pane (taking over a dead
//! PM with resume's fencing, never a rival generation), writes the one-shot bootstrap and launches
//! it in-process: the same consume, check and exec `corral agent launch` performs, so no bootstrap
//! path is shown or typed. The adopted pane is recorded (`AgentPm::adopted_pane`): corral never
//! closes it, and types into it only after verifying its live host.
//!
//! `--bare` execs plain unbound Pi from the installed assets: no store, bootstrap, generation,
//! nonce, bridge, hello or role modules, only the corral-owned login and the guarded fetch.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::agent::{AgentRunState, ContainerRef, pm_state};

use super::bridge::assets::{bun_version, compiled_host, find_bun, read_compat, version_satisfies};
use super::bridge::liveness::{bound_host, gone};
use super::hooks::pm::Overrides;
use super::launch::Prepared;
use super::ops::assistant::Claim;
use crate::Ctx;

/// The variable herdr sets in every pane: the pane bound mode adopts.
pub const PANE_ENV: &str = "HERDR_PANE_ID";
/// The bare entry point under an asset root.
pub const BARE_ENTRY: &str = "src/host/bare.ts";
pub const AUTH_MODULE: &str = "src/auth/index.ts";
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

/// Bound mode up to the exec: every check, then generation n+1 in `pane` and its bootstrap.
pub fn adopt(ctx: &Ctx, pane: &str, overrides: &Overrides) -> Result<Prepared> {
    platform()?;
    let resolved = super::hooks::pm::profile_with(ctx, overrides)?;
    if ctx.herdr.pane_get(pane)?.is_none() {
        return Err(refuse(
            "pane_unknown",
            format!("{pane} is not a pane of this store's herdr server"),
        ));
    }
    crate::pm::adopt_pm(ctx, pane, &resolved, || {
        let bound =
            pm_state::load_current(&ctx.store)?.filter(|pm| pm.state.binding.generation > 0);
        if let Some(pm) = &bound
            && host_alive(&pm.state)?
        {
            return Err(refuse(
                "pm_alive",
                format!(
                    "a live corral-pi PM (generation {}) is bound to pane {}; `corral agent \
                     resume --pm --replace` replaces it",
                    pm.state.binding.generation, pm.pane
                ),
            ));
        }
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

/// `corral pi [--model M] [--provider P] [--effort E]`: adopt this pane and exec the host.
pub fn bound(start: &Path, ctx: &Ctx, overrides: &Overrides) -> Result<()> {
    platform()?;
    let pane = current_pane(std::env::var(PANE_ENV).ok().as_deref())?;
    let prepared = adopt(ctx, &pane, overrides)?;
    super::launch::launch(start, &prepared.launch_args())
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
fn supervise_with(
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
        let generation = ctx
            .store
            .get_assistant_container(&id)?
            .state
            .binding
            .generation;
        let status = match run(&prepared) {
            Ok(status) => status,
            Err(error) => {
                let Some(source) = previous.take() else {
                    // A fresh host that could not start is not left half-open. A resumed
                    // conversation remains recoverable, including when /new failed while
                    // shutting it down.
                    if generation == 1 {
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
        match status.code() {
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
/// crash leaves it resumable. Only a session that never recorded a conversation ends here;
/// `corral assistant end` remains the explicit, permanent end.
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
    if super::ops::assistant::continued_session(ctx, id)?.is_none() {
        end_assistant(ctx, id)?;
        eprintln!("corral pi: assistant {id} ended (no conversation to return to)");
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
    pub model: Option<String>,
    pub provider: Option<String>,
    pub effort: Option<String>,
}

/// The checked bare launch, run in the asset directory: `bun --no-env-file <assets>/src/host/bare.ts
/// --cwd <dir> --provider <p> [--model M] [--effort E]`, or `<assets>/corral-pi-host bare ...` for a
/// compiled installation, whose host bundles the auth module and checks its embedded Bun.
pub fn bare_command(
    data_root: &Path,
    args: &BareArgs,
    cwd: &Path,
    bun: Option<PathBuf>,
) -> Result<std::process::Command> {
    platform()?;
    let id = args.provider.as_deref().unwrap_or(DEFAULT_PROVIDER);
    let provider = super::credentials::provider(id)
        .ok_or_else(|| refuse("usage_error", format!("unknown provider {id}")))?;
    let assets = super::install::installed_in(data_root)?.ok_or_else(|| {
        refuse(
            "assets_unavailable",
            "no corral-pi asset installation; run `corral agent install`",
        )
    })?;
    let compiled = compiled_host(&assets);
    for module in [BARE_ENTRY, AUTH_MODULE]
        .iter()
        .filter(|_| compiled.is_none())
    {
        if !assets.join(module).is_file() {
            return Err(refuse(
                "bare_unavailable",
                format!(
                    "{} has no {module}; reinstall with `corral agent install`",
                    assets.display()
                ),
            ));
        }
    }
    let compat = read_compat(&assets)?;
    let mut command = match compiled {
        Some(host) => {
            let mut command = std::process::Command::new(host);
            command.arg("bare");
            command
        }
        None => {
            let bun =
                bun.ok_or_else(|| refuse("bun_unavailable", "no executable `bun` on PATH"))?;
            let version = bun_version(&bun)?;
            if !version_satisfies(&version, &compat.range) {
                return Err(refuse(
                    "bun_version_unsupported",
                    format!(
                        "bun {version} at {} is outside the GO manifest's tested range {}",
                        bun.display(),
                        compat.range
                    ),
                ));
            }
            let mut command = std::process::Command::new(&bun);
            command.arg("--no-env-file").arg(assets.join(BARE_ENTRY));
            command
        }
    };
    super::hooks::preflight::credential(data_root, Some(provider.vendor))?;
    let cwd = std::path::absolute(cwd).with_context(|| format!("resolving {}", cwd.display()))?;
    command
        .arg("--cwd")
        .arg(cwd)
        .args(["--provider", provider.id]);
    for (flag, value) in [("--model", &args.model), ("--effort", &args.effort)] {
        if let Some(value) = value {
            command.arg(flag).arg(value);
        }
    }
    command.current_dir(&assets);
    Ok(command)
}

/// `corral pi --bare`: plain unbound Pi in the current directory, inside or outside herdr.
#[cfg(unix)]
pub fn bare(args: &BareArgs) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let cwd = std::env::current_dir().context("reading the current directory")?;
    let mut command = bare_command(&super::install::data_root()?, args, &cwd, find_bun())?;
    let error = command.exec();
    Err(error).context("executing the bare Pi host")
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
