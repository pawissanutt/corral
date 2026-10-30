//! Doctor seam: pi-native readback, consulted only when the configuration names the pi-native
//! harness. Owner: T-252 child host-install (adds the Bun version, installed digest,
//! credential presence and wake route).
//!
//! OAuth credential presence comes from file metadata only. Bedrock reads its non-secret binding
//! and profile metadata offline; no doctor line exposes credential values.
use std::path::{Path, PathBuf};

use anyhow::Result;
use corral_core::agent::{AgentRole, QuestionWakeRoute};
use serde_json::{Value, json};

use super::consulted;
use crate::Ctx;
use crate::agent::bridge::assets::{
    asset_digest, bun_version, compiled_host, find_bun, has_module, read_compat, version_satisfies,
};
use crate::agent::{credentials, install};

/// One doctor line, mapped onto the CLI's own check type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorCheck {
    pub id: &'static str,
    pub ok: bool,
    pub hard: bool,
    pub message: String,
    pub fix: Option<String>,
}

const ROLES: [AgentRole; 3] = [AgentRole::Worker, AgentRole::Reviewer, AgentRole::Pm];

fn line(id: &'static str, ok: bool, message: String, fix: &str) -> DoctorCheck {
    DoctorCheck {
        id,
        ok,
        hard: false,
        message,
        fix: (!ok).then(|| fix.to_owned()),
    }
}

fn active(ctx: &Ctx, root: Option<&Path>) -> Result<Option<(PathBuf, &'static str)>, String> {
    if let Some(dir) = ctx.store.config.agent.assets_dir.clone() {
        return Ok(Some((dir, "[agent] assets_dir")));
    }
    let root = root.ok_or("no data directory: neither XDG_DATA_HOME nor HOME is absolute")?;
    match install::installed_in(root) {
        Ok(found) => Ok(found.map(|dir| (dir, "installed"))),
        Err(error) => Err(format!("{error:#}")),
    }
}

/// The installation readback shared by `corral doctor` and `corral agent state`.
pub fn installation(ctx: &Ctx, root: Option<PathBuf>) -> Value {
    let (dir, source) = match active(ctx, root.as_deref()) {
        Err(error) => return json!({"summary": error}),
        Ok(None) => {
            return json!({"summary": "no asset installation (assets_unavailable); run `corral agent install`"});
        }
        Ok(Some(found)) => found,
    };
    let digest = asset_digest(&dir).unwrap_or_else(|error| format!("unreadable ({error:#})"));
    let range = read_compat(&dir).ok().map(|pins| pins.range);
    let compiled = compiled_host(&dir);
    let auth = if !has_module(&dir, "src/auth/index.ts") {
        "missing"
    } else if dir.join("fixture.json").exists() {
        "fixture"
    } else {
        "corral-pi"
    };
    let roles: Vec<&str> = ROLES
        .iter()
        .filter(|role| has_module(&dir, &format!("src/roles/{}/index.ts", role.module_dir())))
        .map(|role| role.as_str())
        .collect();
    let summary = format!(
        "{} ({source}): sha256 {digest}, compat {}, auth {auth}, roles: {}",
        dir.display(),
        if range.is_some() { "GO" } else { "missing" },
        if roles.is_empty() {
            "none".to_owned()
        } else {
            roles.join(", ")
        },
    );
    json!({
        "dir": dir, "source": source, "digest": digest, "compat": range.as_ref().map(|_| "GO"),
        "bun_range": range, "auth": auth, "roles": roles, "summary": summary,
        "compiled": compiled,
    })
}

pub fn checks(ctx: &Ctx) -> Result<Vec<DoctorCheck>> {
    consulted("doctor");
    checks_with(ctx, install::data_root().ok(), find_bun())
}

/// Whether every doctor line (Bun in range, assets with GO and all roles, credentials) is ok.
/// Setup discovery reports the host installed exactly when `corral agent doctor` would.
pub fn ready(ctx: &Ctx) -> bool {
    ready_with(ctx, install::data_root().ok(), find_bun())
}

pub(crate) fn ready_with(ctx: &Ctx, root: Option<PathBuf>, bun: Option<PathBuf>) -> bool {
    checks_with(ctx, root, bun).is_ok_and(|checks| checks.iter().all(|check| check.ok))
}

pub(crate) fn checks_with(
    ctx: &Ctx,
    root: Option<PathBuf>,
    bun: Option<PathBuf>,
) -> Result<Vec<DoctorCheck>> {
    let explicit = ctx.store.config.agent.assets_dir.is_some();
    let found = installation(ctx, root.clone());
    let summary = found["summary"].as_str().unwrap_or_default();
    let assets = match found["dir"].as_str() {
        None => line(
            "pi_native",
            false,
            format!("pi-native: {summary}"),
            "Pi-native profiles are opt-in; remove them, or run `corral agent install`.",
        ),
        Some(dir) => {
            let readiness: Vec<String> = ROLES
                .into_iter()
                .map(
                    |role| match super::preflight::assets(Path::new(dir), role, explicit) {
                        Ok(()) => format!("{} ready", role.as_str()),
                        Err(refusal) => {
                            format!("{} {}: {}", role.as_str(), refusal.code, refusal.detail)
                        }
                    },
                )
                .collect();
            line(
                "pi_native",
                readiness.iter().all(|r| r.ends_with(" ready")),
                format!("pi-native assets {summary}; {}", readiness.join("; ")),
                "Pi-native profiles are opt-in; remove them, or install the missing modules \
                 named above.",
            )
        }
    };
    let range = found["bun_range"].as_str();
    let runtime = match (
        found["compiled"].as_str(),
        bun.as_deref().map(|bun| (bun, bun_version(bun))),
    ) {
        (Some(host), _) => (
            true,
            format!(
                "pi-native bun embedded in {host} (GO range {})",
                range.unwrap_or("unknown")
            ),
        ),
        (None, runtime) => match runtime {
            None => (
                false,
                "pi-native bun: no executable `bun` on PATH".to_owned(),
            ),
            Some((_, Err(refusal))) => (false, format!("pi-native bun: {refusal}")),
            Some((path, Ok(version))) => match range {
                Some(range) if !version_satisfies(&version, range) => (
                    false,
                    format!(
                        "pi-native bun {version} at {} is outside the GO range {range}",
                        path.display()
                    ),
                ),
                Some(range) => (
                    true,
                    format!(
                        "pi-native bun {version} at {} (GO range {range})",
                        path.display()
                    ),
                ),
                None => (
                    true,
                    format!(
                        "pi-native bun {version} at {} (no GO range to compare)",
                        path.display()
                    ),
                ),
            },
        },
    };
    let stored = root.as_deref().map(|root| {
        let each = credentials::PROVIDERS
            .iter()
            .map(|p| {
                if p.id == "amazon-bedrock" {
                    return match crate::agent::aws_profile::read_binding(root) {
                        Ok(Some(binding)) => {
                            format!(
                                "amazon-bedrock bound: profile {}, region {}, metered",
                                binding.profile, binding.region
                            )
                        }
                        Ok(None) => "amazon-bedrock not logged in".into(),
                        Err(error) => format!("amazon-bedrock unusable: {error:#}"),
                    };
                }
                let state = if credentials::provider_status(root, p.id).present {
                    "logged in"
                } else {
                    "not logged in"
                };
                format!("{} {state}", p.id)
            })
            .collect::<Vec<_>>()
            .join(", ");
        (credentials::status_in(root), each)
    });
    let credential = match &stored {
        None => (false, "pi-native credentials: no data directory".to_owned()),
        Some((status, each)) => match (&status.problem, status.present) {
            (Some(problem), _) => (
                false,
                format!("pi-native credentials unsafe: {problem} ({each})"),
            ),
            (None, true) => (
                true,
                format!(
                    "pi-native credentials present in {} ({each})",
                    status.dir.display()
                ),
            ),
            (None, false) => (
                false,
                format!(
                    "pi-native credentials absent from {} ({each})",
                    status.dir.display()
                ),
            ),
        },
    };
    let wake = match crate::questions::wake_route(ctx)? {
        QuestionWakeRoute::PmInbox => {
            "pi-native question wakes go to the bound Pi-native PM's inbox".to_owned()
        }
        QuestionWakeRoute::OmoPush => {
            "pi-native question wakes go to the OmO PM push (pm.push_kinds includes question_opened)"
                .to_owned()
        }
        QuestionWakeRoute::None => format!(
            "pi-native question wakes have no push route (no Pi-native PM bound, no OmO PM push of \
             question_opened); OmO PM watchers wake through `corral wait` on question_opened, and \
             a blocking question reaches Blocked after {}s (agent.question_unattended_secs, 600 \
             by default)",
            ctx.store.config.agent.question_unattended_secs
        ),
    };
    let mut checks = vec![
        line(
            "pi_native_bun",
            runtime.0,
            runtime.1,
            "Install a Bun inside the GO manifest's tested range on the pane's PATH.",
        ),
        assets,
        line(
            "pi_native_credentials",
            credential.0,
            credential.1,
            "Run `corral agent login` in a pane to log in each provider your pi-native profiles use.",
        ),
        line("pi_native_question_wake", true, wake, ""),
        capabilities_check(ctx, found["dir"].as_str().map(Path::new)),
    ];
    let bedrock_configured = ctx.store.config.profiles.values().any(|profile| {
        corral_core::is_pi_native(&profile.harness)
            && profile
                .vendor
                .clone()
                .or_else(|| profile.model.as_deref().and_then(corral_core::infer_vendor))
                .as_deref()
                == Some("amazon-bedrock")
    });
    if bedrock_configured
        || root
            .as_deref()
            .is_some_and(|root| credentials::provider_status(root, "amazon-bedrock").present)
    {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        checks.push(bedrock_check(root.as_deref(), home.as_deref()));
    }
    Ok(checks)
}

/// Every role's effective settings against the installed host: an unavailable default is
/// informational, an explicit request the host lacks fails because launch refuses it.
fn capabilities_check(ctx: &Ctx, assets: Option<&Path>) -> DoctorCheck {
    let supported = assets.map(|dir| {
        read_compat(dir)
            .map(|pins| pins.capabilities)
            .unwrap_or_default()
    });
    let snapshots: Vec<_> = [
        AgentRole::Worker,
        AgentRole::Reviewer,
        AgentRole::Pm,
        AgentRole::Planner,
        AgentRole::PlanReviewer,
        AgentRole::Assistant,
    ]
    .into_iter()
    .map(|role| {
        ctx.store
            .config
            .agent
            .capabilities
            .resolve(role, supported.as_ref())
    })
    .collect();
    let refused = snapshots.iter().find_map(|snapshot| {
        snapshot
            .explicit_unavailable()
            .map(|(key, _)| (snapshot.role, key))
    });
    let summaries: Vec<String> = snapshots
        .iter()
        .map(|snapshot| snapshot.summary())
        .collect();
    line(
        "pi_native_capabilities",
        refused.is_none(),
        format!("pi-native capabilities: {}", summaries.join("; ")),
        &match refused {
            Some((role, key)) => format!(
                "Run `corral agent install` to update the host, or remove \
                 [agent.capabilities.{}] {key}.",
                role.as_str()
            ),
            None => String::new(),
        },
    )
}

/// Offline readiness with explicit roots so a fixture never inspects the owner's AWS files.
pub(crate) fn bedrock_check(root: Option<&Path>, home: Option<&Path>) -> DoctorCheck {
    let readiness = || -> Result<String> {
        let root = root.ok_or_else(|| anyhow::anyhow!("no data directory"))?;
        let home = home.ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
        let binding = super::preflight::bedrock(root, home)?;
        let profile = crate::agent::aws_profile::inspect_profile(home, &binding.profile)?;
        Ok(format!(
            "amazon-bedrock bound: profile {}, region {}, {}, metered",
            binding.profile,
            binding.region,
            profile.description()
        ))
    };
    match readiness() {
        Ok(message) => line("pi_native_bedrock", true, message, ""),
        Err(error) => line(
            "pi_native_bedrock",
            false,
            format!("amazon-bedrock: {error:#}"),
            &format!("{error:#}"),
        ),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::agent::install::tests::{fake_bun, package};
    use crate::agent::test_support::fx;

    #[test]
    fn doctor_reports_runtime_digest_roles_credentials_and_wake_route_without_token_bytes() {
        // Given an installation from the real package with all three production roles:
        let fx = fx();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("data");
        let bun = fake_bun(tmp.path(), "1.4.2", 0);
        let record = install::install_into(&package(), &root, Some(bun.clone())).unwrap();
        let text = |checks: &[DoctorCheck]| {
            checks
                .iter()
                .map(|c| format!("{} {} {}", c.id, c.ok, c.message))
                .collect::<Vec<_>>()
                .join("\n")
        };

        // When doctor runs without credentials:
        let before = checks_with(&fx.ctx, Some(root.clone()), Some(bun.clone())).unwrap();
        let report = text(&before);

        // Then it names the Bun, digest, GO verdict, real auth module, the worker and reviewer roles and no login:
        assert!(
            report.contains("pi_native_bun true pi-native bun 1.4.2"),
            "{report}"
        );
        assert!(
            report.contains(&format!("sha256 {}", record.digest)),
            "{report}"
        );
        assert!(
            report.contains("compat GO, auth corral-pi, roles: worker, reviewer"),
            "{report}"
        );
        assert_eq!(
            installation(&fx.ctx, Some(root.clone()))["roles"],
            json!(["worker", "reviewer", "pm"])
        );
        assert!(
            before.iter().find(|c| c.id == "pi_native").unwrap().ok,
            "{report}"
        );
        assert!(report.contains("pi_native_credentials false pi-native credentials absent"));
        // And with no Pi PM bound and pm.push_events false, the wake line names both fallbacks:
        assert!(!fx.ctx.store.config.pm.push_events);
        let wake = before
            .iter()
            .find(|c| c.id == "pi_native_question_wake")
            .unwrap();
        for part in [
            "have no push route",
            "OmO PM watchers wake through `corral wait` on question_opened",
            "a blocking question reaches Blocked after 600s",
            "agent.question_unattended_secs, 600 by default",
        ] {
            assert!(wake.message.contains(part), "{}", wake.message);
        }

        // And after a login wrote a credential, presence is reported but never its bytes:
        let dir = credentials::auth_dir(&root);
        credentials::ensure_auth_dir(&dir).unwrap();
        let file = dir.join(credentials::AUTH_FILE);
        std::fs::write(
            &file,
            "{\"openai-codex\":{\"access\":\"SYNTHETIC-SECRET-BYTES\"}}",
        )
        .unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let after = text(&checks_with(&fx.ctx, Some(root.clone()), Some(bun)).unwrap());
        assert!(after.contains("pi_native_credentials true pi-native credentials present"));
        assert!(!after.contains("SYNTHETIC-SECRET-BYTES"), "{after}");
        let state = installation(&fx.ctx, Some(root));
        assert!(!state.to_string().contains("SYNTHETIC-SECRET-BYTES"));
        assert_eq!(state["roles"], json!(["worker", "reviewer", "pm"]));
        assert_eq!(state["auth"], "corral-pi");
    }

    #[test]
    fn ready_needs_every_doctor_line_including_credentials() {
        // Given a full installation and a working Bun.
        let fx = fx();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("data");
        let bun = fake_bun(tmp.path(), "1.4.2", 0);
        install::install_into(&package(), &root, Some(bun.clone())).unwrap();
        // Then it is not ready without credentials.
        assert!(!ready_with(&fx.ctx, Some(root.clone()), Some(bun.clone())));
        // And it is ready once a login wrote a private credential file.
        let dir = credentials::auth_dir(&root);
        credentials::ensure_auth_dir(&dir).unwrap();
        let file = dir.join(credentials::AUTH_FILE);
        std::fs::write(&file, "{}").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(ready_with(&fx.ctx, Some(root), Some(bun)));
    }

    #[test]
    fn doctor_flags_a_bun_outside_the_go_range_and_a_missing_installation() {
        let fx = fx();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("data");
        let missing = checks_with(&fx.ctx, Some(root.clone()), None).unwrap();
        assert!(!missing[0].ok && missing[0].message.contains("no executable `bun`"));
        assert!(!missing[1].ok && missing[1].message.contains("assets_unavailable"));
        install::install_into(&package(), &root, Some(fake_bun(tmp.path(), "1.4.2", 0))).unwrap();
        let old = fake_bun(&tmp.path().join("old"), "1.3.0", 0);
        let outside = checks_with(&fx.ctx, Some(root), Some(old)).unwrap();
        assert!(!outside[0].ok, "{:?}", outside[0]);
        assert!(outside[0].message.contains("outside the GO range 1.4.2"));
    }
}
