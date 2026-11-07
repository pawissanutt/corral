//! Shared launch/preflight connection checks. No financial authority is supplied by a host.
use anyhow::{Context, Result, ensure};
use corral_core::connection::ConnectionBinding;
use std::path::Path;

pub fn compatible(assets: &Path, binding: &ConnectionBinding) -> Result<()> {
    binding.validate()?;
    // Existing AWS/local profile bindings keep their pre-connection host contract.
    if !binding.requires_ack() {
        return Ok(());
    }
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(assets.join("compat.json"))
            .context("reading host connection compatibility")?,
    )
    .context("decoding host connection compatibility")?;
    ensure!(
        manifest["connections"].as_array().is_some_and(|ids| ids
            .iter()
            .any(|id| id.as_str() == Some(binding.connection.id()))),
        "connection_unsupported_by_host: installed host does not advertise {}",
        binding.connection.id()
    );
    Ok(())
}

pub fn validate(
    binding: Option<&ConnectionBinding>,
    provider: Option<&str>,
    model: &str,
    effort: Option<&str>,
) -> Result<()> {
    if let Some(binding) = binding {
        binding.validate()?;
        ensure!(
            binding.model == model
                && binding.effort.as_deref() == effort
                && Some(binding.connection.provider()) == provider,
            "connection_invalid: host spec disagrees with binding"
        );
        binding.connection.require_admission()?;
    } else {
        ensure!(
            provider != Some("openai"),
            "connection_invalid: paid OpenAI requires an explicit binding"
        );
    }
    Ok(())
}

pub fn provider(resolved: &corral_core::ResolvedProfile) -> Result<Option<String>> {
    ensure!(
        corral_core::is_corral_pi(&resolved.harness),
        "connection_invalid: native launch has another harness binding"
    );
    Ok(resolved
        .connection_binding()?
        .map(|b| b.connection.provider().to_owned()))
}

pub fn snapshot(
    execution: Option<&corral_core::ExecutionSnapshot>,
) -> Result<Option<ConnectionBinding>> {
    execution
        .map(|snapshot| explicit(&snapshot.resolved))
        .transpose()
        .map(Option::flatten)
}

/// Legacy snapshots remain usable without requiring new host compatibility metadata.
pub fn explicit(resolved: &corral_core::ResolvedProfile) -> Result<Option<ConnectionBinding>> {
    if resolved.connection.is_none() && resolved.candidate.is_none() {
        return Ok(None);
    }
    resolved.connection_binding()
}
