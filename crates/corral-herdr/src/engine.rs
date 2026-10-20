//! Engine preflight and the shared user-facing error vocabulary.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::process::Command;

use crate::{Herdr, HerdrError};

pub const MIN_HERDR_VERSION: &str = "0.9.3";
pub const INSTALL_HERDR: &str = "curl -fsSL https://herdr.dev/install.sh | sh";
pub const ENGINE_DOWN: &str = "corral needs its terminal engine (herdr) running. Run `corral up`.";

#[derive(Debug)]
pub enum EngineProblem {
    Missing,
    TooOld(String),
    Down,
}

impl std::fmt::Display for EngineProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => write!(
                f,
                "corral needs its terminal engine (herdr {MIN_HERDR_VERSION}+). Install it: `{INSTALL_HERDR}`; then run `corral up`."
            ),
            Self::TooOld(version) => write!(
                f,
                "corral needs herdr {MIN_HERDR_VERSION} or newer (found {version}). Update it: `{INSTALL_HERDR}`; then run `corral up`."
            ),
            Self::Down => f.write_str(ENGINE_DOWN),
        }
    }
}

impl std::error::Error for EngineProblem {}

/// Preserves useful non-engine errors, but does not leak raw engine transport errors to users.
pub fn friendly_error(error: &anyhow::Error) -> String {
    if let Some(problem) = error.downcast_ref::<EngineProblem>() {
        return problem.to_string();
    }
    if let Some(problem) = error.downcast_ref::<HerdrError>()
        && matches!(
            problem.code.as_str(),
            "server_not_running" | "connection_refused" | "socket_not_found"
        )
    {
        return EngineProblem::Down.to_string();
    }
    format!("{error:#}")
}

fn version_numbers(text: &str) -> Option<(u64, u64, u64, bool)> {
    let word = text.split_whitespace().find(|word| {
        word.trim_start_matches('v')
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
    })?;
    let version = word.trim_start_matches('v').split('+').next()?;
    let (release, stable) = version
        .split_once('-')
        .map_or((version, true), |(release, _)| (release, false));
    let mut parts = release.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch.parse().ok()?, stable))
}

impl Herdr {
    /// A command targeting this client; explicit sessions cannot inherit another pane's socket.
    pub fn command(&self) -> Command {
        crate::work::record();
        let mut command = Command::new(&self.bin);
        if let Some(session) = &self.session {
            command.args(["--session", session]);
            for name in [
                "HERDR_ENV",
                "HERDR_SOCKET_PATH",
                "HERDR_PANE_ID",
                "HERDR_TAB_ID",
                "HERDR_WORKSPACE_ID",
            ] {
                command.env_remove(name);
            }
        }
        command
    }

    /// Probes the executable without contacting or starting a server.
    pub fn version(&self) -> Result<String> {
        crate::work::record();
        let output = Command::new(&self.bin)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    anyhow::Error::new(EngineProblem::Missing)
                } else {
                    anyhow::Error::new(error).context("checking the herdr executable")
                }
            })?;
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !output.status.success() || version_numbers(&version).is_none() {
            bail!("could not read the herdr version. Reinstall it: `{INSTALL_HERDR}`");
        }
        if version_numbers(&version) < version_numbers(MIN_HERDR_VERSION) {
            return Err(EngineProblem::TooOld(version).into());
        }
        Ok(version)
    }

    pub fn require_running(&self) -> Result<()> {
        self.version()?;
        self.workspace_list().map(|_| ())
    }

    pub fn plugin_linked(&self, manifest: &std::path::Path) -> Result<bool> {
        #[derive(Deserialize)]
        struct Plugins {
            plugins: Vec<Plugin>,
        }
        #[derive(Deserialize)]
        struct Plugin {
            plugin_id: String,
            enabled: bool,
            manifest_path: std::path::PathBuf,
        }
        let result = self.call(&["plugin", "list", "--plugin", "corral", "--json"])?;
        let plugins: Plugins = serde_json::from_value(result).context("reading herdr plugins")?;
        Ok(plugins
            .plugins
            .iter()
            .any(|p| p.plugin_id == "corral" && p.enabled && p.manifest_path == manifest))
    }
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
