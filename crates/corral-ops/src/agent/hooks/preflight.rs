//! The dispatch, review-run and pm-open preflight for pi-native profiles (F6), owned by
//! integration-seams. It refuses before any worktree, pane or bootstrap exists, naming the
//! missing piece and the T-252 child that supplies it. Later children make it pass by adding
//! modules to the asset directory, never by editing this check.
use std::path::Path;

use anyhow::Result;
use corral_core::agent::AgentRole;
use corral_core::{ResolvedProfile, RouteDecision};

use super::{consulted, is_pi_native};
use crate::Ctx;

/// A preflight refusal: a stable code plus what is missing and who supplies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub detail: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "pi-native preflight refused: {}: {}",
            self.code, self.detail
        )
    }
}

impl std::error::Error for Refusal {}

fn refuse(code: &'static str, detail: String) -> Refusal {
    Refusal { code, detail }
}

/// The child that lands a role's module.
pub fn role_owner(role: AgentRole) -> &'static str {
    match role {
        AgentRole::Worker => "worker-role",
        AgentRole::Reviewer => "reviewer-role",
        AgentRole::Pm => "pm-role",
    }
}

/// A dispatch route: resolved only when its harness is pi-native.
pub fn route(ctx: &Ctx, role: AgentRole, route: &RouteDecision) -> Result<()> {
    if !is_pi_native(&route.harness) {
        return Ok(());
    }
    check(ctx, role, &ctx.store.config.resolve_route(route)?)
}

/// Refuses a pi-native profile whose vendor, assets, GO manifest or modules are missing.
pub fn check(ctx: &Ctx, role: AgentRole, resolved: &ResolvedProfile) -> Result<()> {
    if !is_pi_native(&resolved.harness) {
        return Ok(());
    }
    consulted("preflight");
    corral_core::check_pi_native_vendor(&resolved.profile, resolved.vendor.as_deref())?;
    let explicit = ctx.store.config.agent.assets_dir.clone();
    let dir = match explicit.clone() {
        Some(dir) => dir,
        None => super::super::install::installed_assets(ctx)?.ok_or_else(|| {
            refuse(
                "assets_unavailable",
                format!(
                    "no pi-native asset installation; set [agent] assets_dir or install one \
                     (owner {})",
                    super::super::install::OWNER
                ),
            )
        })?,
    };
    Ok(assets(&dir, role, explicit.is_some())?)
}

/// The asset-directory half of the check, in the order a launch needs them: the directory, the
/// GO compatibility manifest, then the role, delivery and auth modules at their fixed paths.
pub fn assets(dir: &Path, role: AgentRole, explicit: bool) -> Result<(), Refusal> {
    if !dir.is_dir() {
        return Err(refuse(
            "assets_unavailable",
            format!(
                "asset directory {} does not exist (owner {})",
                dir.display(),
                super::super::install::OWNER
            ),
        ));
    }
    if !explicit && dir.join("fixture.json").exists() {
        return Err(refuse(
            "fixture_assets_not_allowed",
            format!(
                "{} is a fixture installation; fixtures load only through an explicit \
                 [agent] assets_dir",
                dir.display()
            ),
        ));
    }
    let manifest = dir.join("compat.json");
    let verdict = std::fs::read(&manifest)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value["verdict"].as_str().map(str::to_owned));
    if verdict.as_deref() != Some("GO") {
        return Err(refuse(
            "compat_unavailable",
            format!(
                "no GO compatibility manifest at {} (owner {})",
                manifest.display(),
                super::super::launch::OWNER
            ),
        ));
    }
    let role_name = role.as_str();
    let modules = [
        (
            format!("src/roles/{role_name}/index.ts"),
            "role_unavailable",
            format!("{role_name}, "),
            role_owner(role),
        ),
        (
            "src/delivery/index.ts".to_owned(),
            "delivery_unavailable",
            String::new(),
            super::super::delivery::OWNER,
        ),
        (
            "src/auth/index.ts".to_owned(),
            "credentials_unavailable",
            String::new(),
            super::super::credentials::OWNER,
        ),
    ];
    for (module, code, subject, owner) in modules {
        if !dir.join(&module).is_file() {
            return Err(refuse(
                code,
                format!(
                    "{subject}owner {owner} (no {module} under {})",
                    dir.display()
                ),
            ));
        }
    }
    Ok(())
}
