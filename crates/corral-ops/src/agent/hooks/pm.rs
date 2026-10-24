//! PM launch seam for a pi-native PM profile. Owner: T-252 child pm-role.
use anyhow::Result;
use corral_core::agent::AgentRole;
use corral_core::{ProfileChoice, ResolvedProfile};

use super::{consulted, is_pi_native};
use crate::Ctx;

/// pm open's preflight, before any policy delivery, workspace or pane: the bootstrap
/// connection's profile, else the configured `pm.agent` profile when its harness is pi-native.
pub fn preflight(ctx: &Ctx, bootstrap: Option<&ResolvedProfile>) -> Result<()> {
    let resolved = match bootstrap {
        Some(resolved) => resolved.clone(),
        None => {
            let config = &ctx.store.config;
            let name = &config.pm.agent;
            let harness = config
                .profiles
                .get(name)
                .map_or(name.as_str(), |p| p.harness.as_str());
            if !is_pi_native(harness) {
                return Ok(());
            }
            let choice = ProfileChoice {
                profile: Some(name.clone()),
                ..ProfileChoice::default()
            };
            config.resolve_profile(&choice, None, None)?
        }
    };
    super::preflight::check(ctx, AgentRole::Pm, &resolved)
}

pub fn command(_ctx: &Ctx) -> Result<String> {
    consulted("pm");
    anyhow::bail!(
        "op_unavailable: the pi-native PM launch lands with T-252 child {}",
        super::super::pm_wake::OWNER
    )
}
