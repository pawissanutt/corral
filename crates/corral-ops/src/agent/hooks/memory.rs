//! Generation-local memory projection. No OmO reads, Git writes or model-selected paths.
use anyhow::{Context, Result, ensure};
use corral_core::{Store, agent::AgentRole, memory::MemoryRole};
use std::path::Path;

pub const FILE: &str = "memory-context.md";

pub fn role(role: AgentRole) -> Option<MemoryRole> {
    match role {
        AgentRole::Pm => Some(MemoryRole::Pm),
        AgentRole::Assistant => Some(MemoryRole::Assistant),
        AgentRole::Worker => Some(MemoryRole::Worker),
        _ => None,
    }
}

/// Old installed hosts must not silently discard the new projection contract.
pub fn compatible(assets: &Path, agent_role: AgentRole) -> Result<()> {
    if role(agent_role).is_none() {
        return Ok(());
    }
    let bytes =
        std::fs::read(assets.join("compat.json")).context("reading memory host contract")?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&bytes).context("decoding memory host contract")?;
    ensure!(
        manifest.get("memory_projection").and_then(|v| v.as_u64()) == Some(1),
        "installed host lacks memory projection v1; reinstall Pi host assets"
    );
    Ok(())
}

pub fn projection(store: &Store, agent_role: AgentRole, project: &str) -> Result<Option<String>> {
    let Some(role) = role(agent_role) else {
        return Ok(None);
    };
    let trusted_project = (role == MemoryRole::Worker
        && store.config.agent.capabilities.trusts(project))
    .then_some(project);
    if let Some(project) = trusted_project {
        store.project_path(project)?;
    }
    Ok(Some(store.memory_projection(role, trusted_project)?.text))
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
