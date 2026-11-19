//! Pure named Build profile resolution; spending permission is independent of board authority.
use super::{BuildArgs, Overrides, refuse, require_provider};
use anyhow::{Context, Result, ensure};
use corral_core::{
    Config, ProfileChoice, connection::ConnectionBinding, paid_episode::PaidApproval,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Route {
    pub name: String,
    pub label: String,
    pub provider: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding: Option<ConnectionBinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paid: Option<PaidApproval>,
}

pub(super) fn resolve(config: &Config, name: &str, args: &BuildArgs) -> Result<Route> {
    let mut resolved = config.resolve_profile(
        &ProfileChoice {
            profile: Some(name.into()),
            ..ProfileChoice::default()
        },
        None,
        None,
    )?;
    ensure!(
        super::super::hooks::is_corral_pi(&resolved.harness),
        "profile '{name}' is not corral-pi"
    );
    super::super::hooks::pm::apply(
        &mut resolved,
        &Overrides {
            profile: Some(name.into()),
            model: args.model.clone(),
            provider: args.provider.clone(),
            effort: args.effort.clone(),
            ..Overrides::default()
        },
    )?;
    let binding = resolved
        .connection_binding()?
        .context("profile has no native connection")?;
    let paid = if binding.connection.api_key() {
        Some(
            approval(config, name, &binding)
                .map_err(|error| refuse("paid_admission_required", format!("{error:#}")))?,
        )
    } else {
        None
    };
    let model = resolved.model.context("profile names no model")?;
    let provider = binding.connection.provider().to_owned();
    require_provider(&provider)?;
    let label = config
        .profiles
        .get(name)
        .and_then(|p| p.metadata.as_ref())
        .map(|m| m.display_name.clone())
        .unwrap_or_else(|| name.into());
    Ok(Route {
        name: name.into(),
        label,
        provider,
        model,
        effort: resolved.effort,
        binding: Some(binding),
        paid,
    })
}

fn approval(config: &Config, name: &str, binding: &ConnectionBinding) -> Result<PaidApproval> {
    let approval = crate::availability::continuation::api_start_approval(
        config,
        name,
        corral_core::agent::AgentRole::Assistant,
    )?;
    ensure!(
        approval.candidates.as_slice() == std::slice::from_ref(binding),
        "API start must use the exact approved preferred route"
    );
    Ok(approval)
}

pub(super) fn recorded(data_root: &std::path::Path, id: &str) -> Result<(Option<Route>, bool)> {
    use std::io::Read;
    let path = data_root.join("build").join(id).join("session.json");
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .with_context(|| format!("opening {}", path.display()))?
        .take(super::SESSION_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("reading build route")?;
    ensure!(
        bytes.len() as u64 <= super::SESSION_MAX_BYTES,
        "build record exceeds bound"
    );
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).context("reading build identity")?;
    let route = value
        .get("route")
        .map(|route| serde_json::from_value(route.clone()).context("reading recorded build route"))
        .transpose()?;
    let mixed = value
        .get("mixedPaid")
        .map(|flag| {
            flag.as_bool()
                .filter(|flag| *flag)
                .context("invalid mixed Build lineage")
        })
        .transpose()?
        .unwrap_or(false);
    Ok((route, mixed))
}

pub(super) fn validate_recorded(config: &Config, route: &Route) -> Result<()> {
    require_provider(&route.provider)?;
    if let Some(binding) = &route.binding {
        binding.validate()?;
        ensure!(
            binding.model == route.model
                && binding.effort == route.effort
                && binding.connection.provider() == route.provider,
            "recorded build route differs from binding"
        );
        if binding.connection.api_key() {
            let current = resolve(config, &route.name, &BuildArgs::default())?;
            // Spending caps are disabled: only the exact API route must remain approved.
            ensure!(
                current.binding == route.binding,
                "paid_admission_required: recorded API route changed; select its profile again"
            );
        } else {
            ensure!(
                route.paid.is_none(),
                "subscription route carries paid approval"
            );
        }
    } else {
        ensure!(route.paid.is_none(), "paid route has no binding");
    }
    Ok(())
}

pub(super) fn override_recorded(mut route: Route, args: &BuildArgs) -> Result<Route> {
    if args.model.is_none() && args.provider.is_none() && args.effort.is_none() {
        return Ok(route);
    }
    ensure!(
        route.paid.is_none(),
        "paid_admission_required: recorded API model/effort/connection is immutable"
    );
    if let Some(provider) = &args.provider {
        route.provider.clone_from(provider);
    }
    let provider = require_provider(&route.provider)?;
    ensure!(
        route.provider != "openai",
        "paid_admission_required: choose an explicitly approved API profile"
    );
    if let Some(model) = &args.model {
        route.model.clone_from(model);
    }
    if let Some(effort) = &args.effort {
        route.effort = Some(effort.trim().to_ascii_lowercase());
    }
    if route.binding.is_some() {
        let connection = corral_core::connection::ConnectionId::legacy(provider.vendor)
            .context("unknown subscription connection")?;
        route.binding = Some(ConnectionBinding::new(
            connection,
            None,
            route.model.clone(),
            route.effort.clone(),
        )?);
    }
    Ok(route)
}

pub(super) fn require_host(
    assets: &std::path::Path,
    named: bool,
    paid: bool,
    mixed: bool,
) -> Result<()> {
    if !named && !paid && !mixed {
        return Ok(());
    }
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(assets.join("compat.json"))
            .context("reading Build profile host compatibility")?,
    )
    .context("decoding Build profile host compatibility")?;
    for (required, key) in [
        (paid, "build_paid"),
        (named, "build_profiles"),
        (mixed, "build_mixed_paid"),
    ] {
        if required && manifest[key].as_u64() != Some(1) {
            return Err(refuse(
                "build_protocol_unsupported",
                format!(
                    "installed host does not advertise {key}=1; run `corral agent install` before using this Build profile"
                ),
            ));
        }
    }
    Ok(())
}
