//! Doctor seam: pi-native readback, consulted only when the configuration names the pi-native
//! harness. Owner: T-252 child host-install (adds the Bun version, installed digest,
//! credential presence and wake route).
use anyhow::Result;
use corral_core::agent::AgentRole;

use super::consulted;
use crate::Ctx;

/// One doctor line, mapped onto the CLI's own check type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorCheck {
    pub id: &'static str,
    pub ok: bool,
    pub hard: bool,
    pub message: String,
    pub fix: Option<String>,
}

pub fn checks(ctx: &Ctx) -> Result<Vec<DoctorCheck>> {
    consulted("doctor");
    let explicit = ctx.store.config.agent.assets_dir.clone();
    let dir = match explicit.clone() {
        Some(dir) => Some(dir),
        None => super::super::install::installed_assets(ctx)?,
    };
    let (ok, message) = match dir {
        None => (
            false,
            "pi-native: no asset installation (assets_unavailable)".to_owned(),
        ),
        Some(dir) => {
            let roles: Vec<String> = [AgentRole::Worker, AgentRole::Reviewer, AgentRole::Pm]
                .into_iter()
                .map(
                    |role| match super::preflight::assets(&dir, role, explicit.is_some()) {
                        Ok(()) => format!("{} ready", role.as_str()),
                        Err(refusal) => {
                            format!("{} {}: {}", role.as_str(), refusal.code, refusal.detail)
                        }
                    },
                )
                .collect();
            let ok = roles.iter().all(|line| line.ends_with(" ready"));
            (
                ok,
                format!("pi-native assets {}: {}", dir.display(), roles.join("; ")),
            )
        }
    };
    Ok(vec![DoctorCheck {
        id: "pi_native",
        ok,
        hard: false,
        message,
        fix: (!ok).then(|| {
            "Pi-native profiles are opt-in; remove them, or install the missing modules \
             named above."
                .to_owned()
        }),
    }])
}
