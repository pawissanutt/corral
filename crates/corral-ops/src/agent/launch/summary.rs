//! Quick-thread summarization is generation-bound configuration, never a client model override.
use anyhow::{Context, Result, ensure};
use corral_core::connection::ConnectionBinding;
use serde::{Deserialize, Serialize};

use crate::Ctx;
use crate::agent::admission::BindTarget;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryProfile {
    pub profile: String,
    pub model: String,
    pub provider: String,
    pub effort: Option<String>,
    pub binding: Option<ConnectionBinding>,
    pub aws: Option<crate::agent::aws_profile::AwsBinding>,
}

pub(super) fn prepare(
    ctx: &Ctx,
    target: &BindTarget,
    assets: &std::path::Path,
) -> Result<(bool, Option<SummaryProfile>)> {
    let BindTarget::Assistant { session } = target else {
        return Ok((false, None));
    };
    if ctx.store.app_thread_scope("quick")?.as_deref() != Some(session) {
        return Ok((false, None));
    }
    let config = ctx.store.config_layers()?.config()?;
    let Some(name) = config.assistant.summary_profile.as_deref() else {
        return Ok((true, None));
    };
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(assets.join("compat.json")).context("reading summary host compatibility")?,
    )
    .context("decoding summary host compatibility")?;
    ensure!(
        manifest["summary_profile"].as_u64() == Some(1),
        "summary_profile_unsupported: install a corral-pi host supporting summary profiles"
    );
    let resolved = corral_core::assistant::resolve_summary_profile(&config, name)
        .with_context(|| format!("quick-thread summary profile {name:?} is unavailable"))?;
    crate::agent::hooks::preflight::check(ctx, corral_core::agent::AgentRole::Assistant, &resolved)
        .with_context(|| format!("quick-thread summary profile {name:?} preflight failed"))?;
    let model = resolved
        .model
        .as_deref()
        .context("summary profile has no model")?;
    let provider = crate::agent::connections::provider(&resolved)?
        .or_else(|| super::host_provider(resolved.vendor.as_deref(), model))
        .context("summary profile has no provider")?;
    let binding = crate::agent::connections::explicit(&resolved)?;
    if let Some(binding) = &binding {
        crate::availability::continuation::require_launch(
            &ctx.store,
            &target.container(),
            binding,
        )?;
        crate::agent::connections::compatible(assets, binding)?;
    }
    let aws = if provider == "amazon-bedrock" {
        crate::agent::aws_profile::read_binding(&crate::agent::install::data_root()?)?
    } else {
        None
    };
    Ok((
        true,
        Some(SummaryProfile {
            profile: name.to_owned(),
            model: resolved.model.context("summary profile has no model")?,
            provider,
            effort: resolved.effort,
            binding,
            aws,
        }),
    ))
}
