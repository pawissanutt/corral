//! `corral pi`: start the Pi-native host from nothing (T-490), as `pi` starts plain Pi.
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

use super::bridge::assets::{bun_version, find_bun, read_compat, version_satisfies};
use super::bridge::liveness::{bound_host, gone};
use super::hooks::pm::Overrides;
use super::launch::Prepared;
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

fn platform() -> Result<()> {
    if cfg!(unix) {
        Ok(())
    } else {
        Err(refuse(
            "unsupported_platform",
            "the Pi-native host runs on Unix",
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

fn host_alive(state: &AgentRunState) -> Result<bool> {
    Ok(bound_host(state).as_ref().map(gone).transpose()? == Some(false))
}

/// True when `pane` is the saved PM pane `corral pi` adopted.
pub(crate) fn adopted(ctx: &Ctx, container: &ContainerRef, pane: &str) -> Result<bool> {
    Ok(matches!(container, ContainerRef::Pm)
        && pm_state::load(&ctx.store)?.is_some_and(|pm| pm.adopted() && pm.pane == pane))
}

/// For the adopted PM pane, whether its bound host is verifiably running; `None` for any other
/// pane. An adopted pane is the owner's, so only a live host makes it the PM's.
pub(crate) fn adopted_alive(ctx: &Ctx, pane: &str) -> Result<Option<bool>> {
    match pm_state::load(&ctx.store)? {
        Some(pm) if pm.adopted() && pm.pane == pane => host_alive(&pm.state).map(Some),
        _ => Ok(None),
    }
}

/// Bound mode up to the exec: every check, then generation n+1 in `pane` and its bootstrap.
pub fn adopt(ctx: &Ctx, pane: &str, overrides: &Overrides) -> Result<Prepared> {
    platform()?;
    let resolved = super::hooks::pm::profile_with(ctx, overrides)?;
    let project = super::hooks::pm::project(ctx)?;
    if ctx.herdr.pane_get(pane)?.is_none() {
        return Err(refuse(
            "pane_unknown",
            format!("{pane} is not a pane of this store's herdr server"),
        ));
    }
    crate::pm::adopt_pm(ctx, pane, &resolved, || {
        let bound = pm_state::load(&ctx.store)?.filter(|pm| pm.state.binding.generation > 0);
        if let Some(pm) = &bound
            && host_alive(&pm.state)?
        {
            return Err(refuse(
                "pm_alive",
                format!(
                    "a live Pi-native PM (generation {}) is bound to pane {}; `corral agent \
                     resume --pm --replace` replaces it",
                    pm.state.binding.generation, pm.pane
                ),
            ));
        }
        super::admission::adopt_pm_pane(&ctx.store, &project, pane)?;
        match bound {
            // A dead PM is taken over with resume's fencing, drain and notice.
            Some(_) => Ok(super::recovery::recover(
                ctx,
                &ContainerRef::Pm,
                false,
                Some((pane, &resolved)),
            )?
            .1),
            None => super::hooks::pm::prepared(ctx, pane, &resolved),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BareArgs {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub effort: Option<String>,
}

/// The checked bare launch: `bun --no-env-file <assets>/src/host/bare.ts --cwd <dir> --provider
/// <p> [--model M] [--effort E]`, run in the asset directory.
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
            "no pi-native asset installation; run `corral agent install`",
        )
    })?;
    for module in [BARE_ENTRY, AUTH_MODULE] {
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
    let bun = bun.ok_or_else(|| refuse("bun_unavailable", "no executable `bun` on PATH"))?;
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
    super::hooks::preflight::credential(data_root, Some(provider.vendor))?;
    let cwd = std::path::absolute(cwd).with_context(|| format!("resolving {}", cwd.display()))?;
    let mut command = std::process::Command::new(&bun);
    command
        .arg("--no-env-file")
        .arg(assets.join(BARE_ENTRY))
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
