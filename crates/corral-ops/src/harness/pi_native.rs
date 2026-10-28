//! pi-native model discovery: the installed host lists Pi's built-in catalog through
//! `bun --no-env-file <assets>/src/auth/models.ts --provider <id>...`, for the providers whose
//! corral-owned login `corral agent doctor` sees (file metadata only) and that the host and the
//! vendor allowlist accept. No credential is read, no token refreshed and no network reached.
//! Anything missing degrades to free-text entry with a note naming the fix.

use std::path::{Path, PathBuf};

use anyhow::anyhow;
use corral_core::{Config, PI_NATIVE_HARNESS};

use super::{
    HarnessInfo, MODELS_TIMEOUT, ModelSource, apply_efforts, parse_omp_models, run_capture,
};
use crate::agent::bridge::assets::find_bun;
use crate::agent::credentials;
use crate::agent::hooks::preflight;
use crate::agent::install;

/// The listing entry point inside an asset installation.
pub const MODELS_ENTRY: &str = "src/auth/models.ts";

pub(crate) fn detect(config: &Config) -> HarnessInfo {
    detect_with(config, install::data_root().ok(), find_bun())
}

pub(crate) fn detect_with(
    config: &Config,
    root: Option<PathBuf>,
    bun: Option<PathBuf>,
) -> HarnessInfo {
    let mut info = HarnessInfo {
        key: PI_NATIVE_HARNESS.into(),
        program: None,
        path: None,
        version: None,
        models: Vec::new(),
        model_source: ModelSource::FreeText,
        efforts: Vec::new(),
        takes_effort: false,
        notes: Vec::new(),
    };
    apply_efforts(&mut info, config);
    if let Err(note) = list(&mut info, config, root.as_deref(), bun) {
        info.notes.push(note);
    }
    info
}

fn list(
    info: &mut HarnessInfo,
    config: &Config,
    root: Option<&Path>,
    bun: Option<PathBuf>,
) -> Result<(), String> {
    let assets = match config.agent.assets_dir.clone() {
        Some(dir) => Ok(Some(dir)),
        None => root
            .ok_or_else(|| anyhow!("no data directory: neither XDG_DATA_HOME nor HOME is absolute"))
            .and_then(install::installed_in),
    };
    let dir = match assets {
        Ok(Some(dir)) => dir,
        Ok(None) => {
            return Err("no pi-native asset installation; run `corral agent install`".into());
        }
        Err(error) => return Err(format!("{error:#}; run `corral agent install`")),
    };
    let entry = dir.join(MODELS_ENTRY);
    if !entry.is_file() {
        return Err(format!(
            "{} predates model listing (no {MODELS_ENTRY}); run `corral agent install`",
            dir.display()
        ));
    }
    let bun = bun.ok_or("no executable `bun` on PATH")?;
    info.path = Some(bun.clone());
    info.version = install::compat_pi_version(&dir).ok();
    let root = root.ok_or("no data directory for pi-native credentials")?;
    let vendors = preflight::host_vendors(&dir);
    let providers: Vec<&str> = credentials::PROVIDERS
        .iter()
        .filter(|p| vendors.iter().any(|v| v == p.vendor))
        .filter(|p| {
            let status = credentials::provider_status(root, p.id);
            status.present && status.problem.is_none()
        })
        .map(|p| p.id)
        .collect();
    if providers.is_empty() {
        return Err("no pi-native provider is logged in; run `corral agent login`".into());
    }
    let entry = entry
        .to_str()
        .ok_or_else(|| format!("{} is not a UTF-8 path", entry.display()))?;
    let mut args = vec!["--no-env-file", entry];
    for provider in &providers {
        args.extend(["--provider", provider]);
    }
    let shown = format!(
        "{MODELS_ENTRY} --provider {}",
        providers.join(" --provider ")
    );
    let mut models = run_capture(&bun, &args, MODELS_TIMEOUT)
        .and_then(|out| parse_omp_models(&out))
        .map_err(|error| format!("{shown}: {error:#}"))?;
    models.retain(|model| {
        model
            .split_once('/')
            .is_some_and(|(provider, id)| providers.contains(&provider) && !id.is_empty())
    });
    if models.is_empty() {
        return Err(format!("{shown}: no models listed"));
    }
    info.models = models;
    info.model_source = ModelSource::Listed;
    Ok(())
}

#[cfg(all(test, unix))]
#[path = "pi_native_tests.rs"]
mod tests;
