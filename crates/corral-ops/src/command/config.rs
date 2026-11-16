//! Workspace configuration edits use the canonical validated, comment-preserving writer.
use anyhow::{Context, Result, bail};
use corral_core::config_doc::toml_edit::Key;
use corral_core::{ConfigDoc, ConfigLayer, ConfigSource, backend::ApplyError};
use corral_protocol::cmd::config::ConfigCommand;
use sha2::{Digest, Sha256};

use super::{Exec, Verb};

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;

/// Identity of the exact editable workspace document, shared with the settings projection.
pub fn config_hash(doc: &ConfigDoc) -> String {
    format!("{:x}", Sha256::digest(doc.to_string().as_bytes()))
}

/// App settings never install host commands, paths, environment, or operating grants.
pub(super) fn app_key(key: &str) -> bool {
    let Ok(keys) = Key::parse(key) else {
        return false;
    };
    let parts: Vec<_> = keys.iter().map(Key::get).collect();
    matches!(
        parts.as_slice(),
        [
            "assistant",
            "agent" | "summary_profile" | "max_live" | "scratch_max" | "scratch_idle_days"
        ] | ["defaults", "profile"]
            | ["pm", "agent"]
            | ["profiles", _, "model" | "effort"]
            | [
                "limits",
                "running"
                    | "wip"
                    | "running_target"
                    | "review"
                    | "count_reviews"
                    | "count_gates"
                    | "capacity"
            ]
            | ["limits", "per_project", _]
            | [
                "limits",
                "capacity",
                "unfinished" | "agents" | "gates" | "adhoc_reserve"
            ]
            | [
                "ui",
                "mouse"
                    | "done_window"
                    | "done_max"
                    | "agents_ended_window"
                    | "columns"
                    | "show_wip"
                    | "accent"
                    | "compact_cards"
                    | "route_warnings"
                    | "notify_needs_me"
                    | "notify_review"
            ]
            | ["ui", "keys", _]
    )
}

impl Verb for ConfigCommand {
    fn execute(self, cx: &mut Exec<'_>) -> Result<()> {
        match self {
            Self::Set {
                key,
                value,
                expected_hash,
            } => {
                let keys = Key::parse(&key).with_context(|| format!("invalid key {key:?}"))?;
                let parts: Vec<_> = keys.iter().map(Key::get).collect();
                if parts.is_empty() {
                    bail!("empty key");
                }
                if matches!(
                    parts.as_slice(),
                    ["automation", ..] | ["review", "autopilot"]
                ) {
                    bail!(
                        "use corral mode for operating policy changes; activation requires an exact finite preview and explicit confirmation"
                    );
                }
                if crate::owner_only_config_key(&parts) {
                    crate::refuse_pm(cx.invoker, &key)?;
                }
                let whole_review = parts.as_slice() == ["review"];
                let whole_capacity = parts.as_slice() == ["limits", "capacity"];
                cx.ctx.store.edit_config_layer_from(ConfigLayer::Workspace, ConfigSource::SettingsUi, |doc| {
                    // The callback runs under the canonical dispatch/config locks, never before them.
                    if config_hash(doc) != expected_hash {
                        return Err(ApplyError::Precondition("workspace configuration changed; reload settings".into()).into());
                    }
                    let legacy_review = whole_review && doc.get("review.autopilot")?.is_some();
                    if whole_capacity { doc.unset(&key)?; }
                    if parts.as_slice() == ["assistant", "summary_profile"] && value.is_empty() {
                        doc.unset(&key)?;
                    } else {
                        doc.set_str(&key, &value)?;
                    }
                    if legacy_review || whole_review && doc.get("review.autopilot")?.is_some() {
                        bail!("review.autopilot is legacy evidence, not an independent control; use corral mode");
                    }
                    Ok(())
                })?;
                cx.out.line(format_args!("{key} updated"));
                Ok(())
            }
            Self::ProfileAdd {
                name,
                from,
                model,
                effort,
                expected_hash,
            } => {
                super::config_profiles::add(
                    &cx.ctx.store,
                    &name,
                    &from,
                    model,
                    effort,
                    &expected_hash,
                )?;
                cx.out
                    .line(format_args!("profile {} added from {from}", name.trim()));
                Ok(())
            }
            Self::ProfileRemove {
                name,
                expected_hash,
            } => {
                super::config_profiles::remove(&cx.ctx.store, &name, &expected_hash)?;
                cx.out.line(format_args!("profile {name} removed"));
                Ok(())
            }
            Self::Other => Err(super::unknown_command("other")),
        }
    }
}
