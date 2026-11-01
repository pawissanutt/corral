//! Shared memory authorization and validation for operator and authenticated callers.
use anyhow::{Result, ensure};
use corral_core::{
    Store,
    memory::{MemoryRole, MemoryScope},
};

/// Identity is supplied by admission, never by memory payloads.
pub enum MemoryActor<'a> {
    Operator,
    Pm(&'a str),
    Assistant,
    Worker(&'a str),
}

pub fn authorize(
    store: &Store,
    actor: &MemoryActor<'_>,
    scope: &MemoryScope,
    write: bool,
) -> Result<()> {
    if let MemoryScope::Project(project) = scope {
        store.project_path(project)?;
        if !write {
            ensure!(
                store.config.agent.capabilities.trusts(project),
                "project memory text is not trusted"
            );
        }
    }
    let allowed = match actor {
        MemoryActor::Operator => true,
        MemoryActor::Pm(binding) => match scope {
            MemoryScope::Workspace | MemoryScope::Role(MemoryRole::Pm) => true,
            MemoryScope::Project(project) => {
                crate::agent::hooks::pm::contains(store, binding, Some(project))?
            }
            _ => false,
        },
        MemoryActor::Assistant => matches!(
            scope,
            MemoryScope::Workspace
                | MemoryScope::Role(MemoryRole::Assistant)
                | MemoryScope::Project(_)
        ),
        MemoryActor::Worker(bound) => match scope {
            MemoryScope::Workspace => !write,
            MemoryScope::Role(MemoryRole::Worker) => true,
            MemoryScope::Project(project) => project == bound,
            _ => false,
        },
    };
    ensure!(
        allowed,
        "memory scope is outside this caller's authority; propose workspace knowledge through the PM inbox"
    );
    Ok(())
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
