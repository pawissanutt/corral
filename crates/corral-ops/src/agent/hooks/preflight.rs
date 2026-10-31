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
        AgentRole::Planner | AgentRole::PlanReviewer => "plan-roles",
        AgentRole::Pm => "pm-role",
        AgentRole::Assistant => "assistant-role",
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
    corral_core::check_pi_native_metered_opt_in(
        &resolved.profile,
        ctx.store
            .config
            .profiles
            .get(&resolved.profile)
            .and_then(|profile| profile.vendor.as_deref()),
        resolved.vendor.as_deref(),
    )?;
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
    assets(&dir, role, explicit.is_some())?;
    host_provider(&dir, resolved.vendor.as_deref())?;
    // A development or fixture `[agent] assets_dir` brings its own auth module; only the managed
    // installation reads the corral-owned login, so only it is checked here.
    if explicit.is_none() {
        credential(
            &super::super::install::data_root()?,
            resolved.vendor.as_deref(),
        )?;
    }
    if resolved.vendor.as_deref() == Some("amazon-bedrock") {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| refuse("credentials_unavailable", "HOME is not set".into()))?;
        bedrock(&super::super::install::data_root()?, Path::new(&home))?;
    }
    Ok(())
}

/// Read-only AWS readiness, shared with doctor and fixture checks. A development host still
/// needs the owner's explicit, non-secret Bedrock binding.
pub(crate) fn bedrock(
    root: &Path,
    home: &Path,
) -> Result<super::super::aws_profile::AwsBinding, Refusal> {
    let checked = || -> Result<super::super::aws_profile::AwsBinding> {
        let binding = super::super::aws_profile::read_binding(root)?.ok_or_else(|| {
            anyhow::anyhow!(
                "amazon-bedrock not logged in; run `corral agent login --provider amazon-bedrock`"
            )
        })?;
        super::super::aws_profile::inspect_profile(home, &binding.profile)?
            .check_ready(jiff::Timestamp::now())?;
        Ok(binding)
    };
    checked().map_err(|error| {
        let detail = format!("{error:#}");
        let code = if detail.starts_with("network_refused:") {
            "network_refused"
        } else {
            "credentials_unavailable"
        };
        refuse(code, detail)
    })
}

/// Builds the notice only. Readiness checks never present metered messages or events.
pub fn metered_notice(
    role: &str,
    resolved: &ResolvedProfile,
    binding: &super::super::aws_profile::AwsBinding,
) -> Option<String> {
    (is_pi_native(&resolved.harness) && resolved.vendor.as_deref() == Some("amazon-bedrock")).then(
        || {
            format!(
                "metered: {role} profile {} runs on Amazon Bedrock (per-token AWS billing; \
             AWS profile {}, region {}, model {})",
                resolved.profile,
                binding.profile,
                binding.region,
                resolved.model.as_deref().unwrap_or("(unset)")
            )
        },
    )
}

/// Refuses a vendor whose provider has no usable corral-owned login, naming the provider and the
/// command that provisions it. Reads file metadata only.
pub fn credential(data_root: &Path, vendor: Option<&str>) -> Result<(), Refusal> {
    use super::super::credentials::{provider_for_vendor, provider_status};
    let Some(provider) = vendor.and_then(provider_for_vendor) else {
        return Ok(());
    };
    let status = provider_status(data_root, provider.id);
    let login = format!("run `corral agent login --provider {}`", provider.id);
    match (status.present, status.problem) {
        (true, None) => Ok(()),
        (_, Some(problem)) => Err(refuse(
            "credentials_unavailable",
            format!(
                "provider {} ({}) credential is unusable: {problem}; {login}",
                provider.id, provider.label
            ),
        )),
        (false, None) => Err(refuse(
            "credentials_unavailable",
            format!(
                "provider {} ({}) has no login in {}; {login}",
                provider.id,
                provider.label,
                status.dir.display()
            ),
        )),
    }
}

/// Providers a host declares when `compat.json` has no `providers` list: the original host
/// reaches OpenAI only.
const DEFAULT_HOST_PROVIDERS: [&str; 1] = ["openai"];

/// The providers an installation's `compat.json` declares, or the default when it lists none.
fn declared_providers(dir: &Path) -> Vec<String> {
    std::fs::read(dir.join("compat.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| {
            value["providers"].as_array().map(|list| {
                list.iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
        })
        .unwrap_or_else(|| {
            DEFAULT_HOST_PROVIDERS
                .iter()
                .map(|p| (*p).to_owned())
                .collect()
        })
}

/// The allowlisted vendors the host a dispatch would use can run: setup reports these, not the
/// bare allowlist, so it never offers a vendor that preflight would refuse.
pub fn supported_vendors(ctx: &Ctx) -> Vec<String> {
    let dir = ctx
        .store
        .config
        .agent
        .assets_dir
        .clone()
        .or_else(|| super::super::install::installed_assets(ctx).ok().flatten());
    let declared = dir.map_or_else(
        || {
            DEFAULT_HOST_PROVIDERS
                .iter()
                .map(|p| (*p).to_owned())
                .collect()
        },
        |dir| declared_providers(&dir),
    );
    allowed(&declared)
}

/// The allowlisted vendors the host installed at `dir` can run.
pub(crate) fn host_vendors(dir: &Path) -> Vec<String> {
    allowed(&declared_providers(dir))
}

fn allowed(declared: &[String]) -> Vec<String> {
    corral_core::PI_NATIVE_VENDORS
        .iter()
        .filter(|vendor| declared.iter().any(|d| d == *vendor))
        .map(|vendor| (*vendor).to_owned())
        .collect()
}

/// Refuses an allowlisted vendor the installed host cannot yet reach. The allowlist says which
/// vendors a profile may name; `compat.json` `providers` says which ones this installation's host
/// can run, so a profile never passes preflight and then fails at runtime.
fn host_provider(dir: &Path, vendor: Option<&str>) -> Result<(), Refusal> {
    let Some(vendor) = vendor else {
        return Ok(());
    };
    let declared = declared_providers(dir);
    if declared.iter().any(|provider| provider == vendor) {
        return Ok(());
    }
    Err(refuse(
        "provider_unsupported_by_host",
        format!(
            "vendor '{vendor}' is allowed for pi-native profiles but the installed host declares \
             providers [{}] in {}/compat.json; use a supported vendor or install a host that \
             declares '{vendor}'",
            declared.join(", "),
            dir.display()
        ),
    ))
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
            format!("src/roles/{}/index.ts", role.module_dir()),
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
        if !crate::agent::bridge::assets::has_module(dir, &module) {
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

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::agent::credentials::{auth_dir, ensure_auth_dir};

    fn login(root: &Path, file: &str) {
        ensure_auth_dir(&auth_dir(root)).unwrap();
        let path = auth_dir(root).join(file);
        std::fs::write(&path, "{\"type\":\"oauth\"}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn an_anthropic_profile_needs_an_anthropic_login_and_openai_keeps_its_own() {
        // Given only a Codex login (still in the pre-T-479 shared file):
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        login(root, "auth.json");
        // Then openai passes and anthropic is refused naming its provider and login command:
        assert_eq!(credential(root, Some("openai")), Ok(()));
        let refused = credential(root, Some("anthropic")).unwrap_err();
        assert_eq!(refused.code, "credentials_unavailable");
        assert!(
            refused
                .detail
                .starts_with("provider anthropic (Anthropic Claude (Pro/Max)) has no login")
                && refused
                    .detail
                    .ends_with("run `corral agent login --provider anthropic`"),
            "{refused}"
        );
        // And once Anthropic is logged in, a claude profile passes too:
        login(root, "anthropic.json");
        assert_eq!(credential(root, Some("anthropic")), Ok(()));
        // An Anthropic login alone does not make an openai profile runnable:
        std::fs::remove_file(auth_dir(root).join("auth.json")).unwrap();
        let openai = credential(root, Some("openai")).unwrap_err();
        assert!(
            openai
                .detail
                .ends_with("run `corral agent login --provider openai-codex`"),
            "{openai}"
        );
        // A loose file is unusable, not absent:
        std::fs::set_permissions(
            auth_dir(root).join("anthropic.json"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(
            credential(root, Some("anthropic"))
                .unwrap_err()
                .detail
                .contains("is unusable")
        );
    }
}
