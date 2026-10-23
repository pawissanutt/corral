//! The gate context file (schema v1) a tier command reads through `CORRAL_GATE_CONTEXT`.
//! Like `CORRAL_GATE_TIER`, it derives from the identity and selection, so neither enters the
//! captured environment.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::{CandidateIdentity, ExpectedFailure};
use serde::Serialize;

use super::{GateContract, GateDestination};

const SCHEMA_VERSION: u32 = 1;

#[derive(Serialize)]
struct ContextFile<'a> {
    schema_version: u32,
    task: &'a str,
    run: &'a str,
    attempt: &'a str,
    tier: &'a str,
    commands: &'a [String],
    scope: &'a [String],
    tests: &'a [String],
    expected_failures: &'a [ExpectedFailure],
    destination: &'a GateDestination,
    integration_base: &'a str,
    /// `merge-base(head, integration_base)..head`.
    changed_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GateContext {
    pub(crate) tier: String,
    pub(crate) path: PathBuf,
    pub(crate) sha256: String,
}

/// `<task>-<run>-<attempt>.context.json` beside the attempt's log.
pub(crate) fn path_for(log: &Path) -> PathBuf {
    log.with_extension("context.json")
}

pub(crate) fn write(
    path: &Path,
    contract: &GateContract,
    identity: &CandidateIdentity,
    checkout: &Path,
) -> Result<GateContext> {
    let base = corral_git::merge_base(checkout, &identity.integration_base, &identity.head)?;
    let changed_paths = corral_git::changed_paths_between(checkout, &base, &identity.head)?;
    let bytes = serde_json::to_vec_pretty(&ContextFile {
        schema_version: SCHEMA_VERSION,
        task: &identity.task,
        run: &identity.run,
        attempt: &identity.attempt,
        tier: &contract.tier,
        commands: &contract.commands,
        scope: &contract.scope,
        tests: &contract.tests,
        expected_failures: &contract.expected_failures,
        destination: &contract.destination,
        integration_base: &identity.integration_base,
        changed_paths,
    })?;
    std::fs::write(path, &bytes)
        .with_context(|| format!("writing gate context {}", path.display()))?;
    Ok(GateContext {
        tier: contract.tier.clone(),
        path: path.to_owned(),
        sha256: crate::verification::digest(&bytes),
    })
}
