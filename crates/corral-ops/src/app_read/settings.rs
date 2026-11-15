//! Safe, descriptive settings projections from canonical configuration and operating records.
//!
//! This module never performs launch preflight, reads provider credentials, or derives authority
//! from the process environment. Grant flags describe configured workspace policy, not permission
//! to execute: the canonical executor still validates confirmation, expiry, scope and task state.
//! Workspace/store paths are server-selected display values, never client-supplied read targets.
use anyhow::{Context, Result};
use corral_core::{Config, ConfigLayer, OperatingApproval, OperatingPolicy};
use corral_protocol::rows::{
    GrantLine, ListenerInfo, ModelChoice, OperatingMode, SettingsSnapshot,
};
use jiff::SignedDuration;

use crate::Ctx;

const MODEL_CHOICES: usize = 16;
const ORIGINS: usize = 8;

/// Return the singleton settings topic row, with a stable content revision.
///
/// Model choices contain only corral-pi profiles with a configured model. Their names are exact
/// profile keys suitable for `assistant.agent`; launch eligibility is not asserted. The explicit
/// assistant profile is absent when it inherits `pm.agent`; the summary profile is absent when
/// rollover inherits the thread's current model. Host identity is not available in this contract.
/// `profiles` separately contains every exact configured profile (including non-assistant harnesses,
/// profiles without a model, and personal-layer profiles). It exposes configured model/effort,
/// workspace/personal presence, and removal restrictions from the same service used at commit.
/// It also reports structural summary-profile refusals; credentials and access are not asserted.
///
/// Choices are limited to sixteen (the configured effective assistant choice first), origins to
/// eight, and display-only paths/scope descriptions to 256 characters. Oversized identifiers are
/// omitted rather than truncated into a different actionable profile, model, effort or origin.
/// The listener describes configuration, not the currently bound socket; changing it needs restart.
///
/// `config_hash` identifies the complete editable workspace document, using the same canonical
/// helper as `config.set`, rather than hashing this bounded/redacted view or the personal layer.
/// Errors reading configuration or operating state propagate; there is no fixture fallback.
pub fn settings_rows(ctx: &Ctx) -> Result<Vec<SettingsSnapshot>> {
    Ok(vec![settings(ctx)?])
}

fn settings(ctx: &Ctx) -> Result<SettingsSnapshot> {
    let layers = ctx
        .store
        .config_layers()
        .context("reading settings configuration layers")?;
    let config = layers.config().context("decoding settings configuration")?;
    let document = layers
        .doc(ConfigLayer::Workspace)
        .context("reading editable settings document")?;
    let state = ctx
        .store
        .operating_state()
        .context("reading settings operating state")?;
    let policy = &config.automation;
    let approval = state.approval.as_ref().filter(|approval| {
        policy.authorization.as_deref() == Some(approval.authorization.as_str())
            && policy.revision == approval.revision
            && approval.workspace == ctx.store.root()
    });
    let listener = config.daemon.listen.as_ref().map(|address| ListenerInfo {
        port: address.addr().port(),
        origins: config
            .daemon
            .web_origins
            .iter()
            .filter_map(|origin| exact(origin.as_str(), 256))
            .take(ORIGINS)
            .collect(),
    });
    let mut row = SettingsSnapshot {
        id: "settings".into(),
        workspace: super::short(&ctx.store.root().display().to_string(), 256),
        store: super::short(&ctx.store.dir().display().to_string(), 256),
        host: String::new(),
        mode: match policy.mode {
            corral_core::OperatingMode::Assist => OperatingMode::Assist,
            corral_core::OperatingMode::Autopilot => OperatingMode::Autopilot,
        },
        grants: grants(&config, policy, approval),
        assistant_models: models(&config),
        profiles: crate::command::profile_choices(&ctx.store, &layers, &config)?,
        assistant_profile: config
            .assistant
            .agent
            .as_deref()
            .and_then(|value| exact(value, 96)),
        summary_profile: config
            .assistant
            .summary_profile
            .as_deref()
            .and_then(|value| exact(value, 96)),
        listener,
        config_hash: crate::command::config_hash(&document),
        rev: 0,
    };
    row.rev = super::revision(&row)?;
    Ok(row)
}

fn exact(value: &str, max_bytes: usize) -> Option<String> {
    (value.len() <= max_bytes).then(|| value.to_owned())
}

fn models(config: &Config) -> Vec<ModelChoice> {
    let selected = config.assistant.agent.as_ref().unwrap_or(&config.pm.agent);
    let choices = config
        .profiles
        .get_key_value(selected)
        .into_iter()
        .chain(config.profiles.iter().filter(|(name, _)| *name != selected));
    choices
        .filter_map(|(name, profile)| {
            if !corral_core::profile::is_corral_pi(&profile.harness) {
                return None;
            }
            // Preserve all actionable values exactly; never advertise a truncated choice.
            let effort = match profile.effort.as_deref() {
                Some(value) => Some(exact(value, 32)?),
                None => None,
            };
            Some(ModelChoice {
                name: exact(name, 96)?,
                model: exact(profile.model.as_deref()?, 192)?,
                effort,
                note: None,
            })
        })
        .take(MODEL_CHOICES)
        .collect()
}

fn grants(
    config: &Config,
    policy: &OperatingPolicy,
    approval: Option<&OperatingApproval>,
) -> Vec<GrantLine> {
    let scope = scope(policy);
    // This is receipt metadata only, not an independent grant-validity implementation.
    let expires_at = approval.and_then(|approval| {
        let days = policy.scope.expires_after_days?;
        approval
            .confirmed_at
            .checked_add(SignedDuration::from_hours(i64::from(days) * 24))
            .ok()
            .map(|at| at.to_string())
    });
    [
        (
            "automation.authority.dispatch",
            policy.authority.dispatch,
            true,
        ),
        ("automation.authority.accept", policy.authority.accept, true),
        (
            "automation.authority.plan_accept",
            policy.authority.plan_accept,
            true,
        ),
        (
            "automation.authority.aggregate_accept",
            policy.authority.aggregate_accept,
            true,
        ),
        ("automation.paused", policy.paused, false),
        ("automation.turbo", policy.turbo, false),
        ("dispatch.paused", config.dispatch.paused, false),
        ("pm.supervise", config.pm.supervise, false),
    ]
    .into_iter()
    .map(|(key, enabled, authority)| GrantLine {
        key: key.into(),
        enabled,
        scope: if authority { scope.clone() } else { None },
        expires_at: if authority { expires_at.clone() } else { None },
    })
    .collect()
}

fn scope(policy: &OperatingPolicy) -> Option<String> {
    let scope = &policy.scope;
    let labels = scope
        .tasks
        .keys()
        .take(12)
        .map(|value| ("task", value.as_str()))
        .chain(
            scope
                .projects
                .keys()
                .take(12)
                .map(|value| ("project", value.as_str())),
        )
        .chain(
            scope
                .plans
                .iter()
                .take(12)
                .map(|plan| ("plan", plan.task.as_str())),
        )
        .chain(
            scope
                .exclusions
                .iter()
                .take(12)
                .map(|value| ("excluded", value.as_str())),
        );
    let mut text = String::new();
    for (prefix, value) in labels {
        if !text.is_empty() {
            text.push_str(", ");
        }
        text.push_str(prefix);
        text.push(':');
        text.extend(value.chars().take(48));
        if text.chars().count() >= 256 {
            break;
        }
    }
    (!text.is_empty()).then(|| super::short(&text, 256))
}
