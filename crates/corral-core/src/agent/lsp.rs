//! Language-server capability settings (T-569): configured servers, their file extensions and
//! root markers, validated once and recorded in the generation snapshot.
use super::{AgentRole, CapabilitySettings};

/// Validates configured servers and preserves their generation-bound settings.
pub(super) fn lsp_settings(table: &CapabilitySettings) -> Result<CapabilitySettings, String> {
    let mut extensions = std::collections::BTreeSet::new();
    for (key, value) in table {
        if key != "languages" {
            return Err(format!("has no setting {key} (known: languages)"));
        }
        let servers = value.as_object().ok_or("languages must be a table")?;
        for (id, value) in servers {
            let spec = value.as_object().ok_or("language server must be a table")?;
            if id.is_empty()
                || spec.keys().any(|k| {
                    !["command", "args", "extensions", "root_markers"].contains(&k.as_str())
                })
            {
                return Err(format!("invalid language server {id}"));
            }
            if !spec
                .get("command")
                .and_then(|v| v.as_str())
                .is_some_and(|v| !v.trim().is_empty() && !v.contains('\0'))
            {
                return Err(format!("{id}: command must be a nonempty string"));
            }
            for key in ["args", "root_markers"] {
                if let Some(value) = spec.get(key) {
                    let items = value
                        .as_array()
                        .ok_or_else(|| format!("{id}: {key} must be a list"))?;
                    if items.iter().any(|v| {
                        !v.as_str().is_some_and(|s| {
                            !s.contains('\0')
                                && (key != "root_markers"
                                    || (!s.is_empty() && s != "." && s != ".." && !s.contains('/')))
                        })
                    }) {
                        return Err(format!("{id}: invalid {key}"));
                    }
                }
            }
            let mappings = spec
                .get("extensions")
                .and_then(|v| v.as_object())
                .ok_or_else(|| format!("{id}: extensions must be a table"))?;
            if mappings.is_empty() {
                return Err(format!("{id}: extensions must not be empty"));
            }
            for (ext, language) in mappings {
                if !ext.starts_with('.')
                    || ext.len() < 2
                    || ext.contains('/')
                    || !extensions.insert(ext.clone())
                    || language.as_str().is_none_or(|s| s.trim().is_empty())
                {
                    return Err(format!("{id}: invalid or duplicate extension {ext}"));
                }
            }
        }
    }
    Ok(table.clone())
}

pub(super) fn lsp_defaults(role: AgentRole) -> Option<[bool; 3]> {
    match role {
        AgentRole::Worker => Some([true, true, true]),
        AgentRole::Reviewer => Some([false, false, true]),
        AgentRole::Planner | AgentRole::PlanReviewer => Some([false, true, true]),
        AgentRole::Pm => None,
    }
}
