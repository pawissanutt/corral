//! Path helpers for the agent areas inside `.corral/`. Later modules use these accessors, never
//! `state_dir()` joins, so T-253 can move the areas without editing them. Runtime files,
//! credentials and installed assets live outside `.corral/` and are not described here.
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::ContainerRef;
use crate::Store;

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `st-` plus 16 hex digits of the canonical store directory's SHA-256.
pub fn store_id(store: &Store) -> String {
    store_id_of(store.dir())
}

pub fn store_id_of(dir: &Path) -> String {
    let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let digest = sha256_hex(canonical.as_os_str().as_encoded_bytes());
    format!("st-{}", &digest[..16])
}

pub fn pm_state_path(store: &Store) -> PathBuf {
    store.state_dir().join("agent-pm.json")
}

/// `.corral/agent/sessions/`: recovery-critical while a container is open, a terminal
/// transcript afterwards.
pub fn sessions_dir(store: &Store) -> PathBuf {
    store.dir().join("agent").join("sessions")
}

/// A launcher-selected auxiliary transcript. Hashing the opaque request key avoids path traversal.
pub fn usage_session_file(store: &Store, container: &ContainerRef, request: &str) -> PathBuf {
    container_sessions_dir(store, container)
        .join("auxiliary")
        .join(format!("{}.jsonl", sha256_hex(request.as_bytes())))
}

/// `sessions/<task>/<run>[/<review>]`, `<task>/plan-<D>`, `<task>/plan-<P>/<PR>`, or `pm`.
pub fn container_sessions_dir(store: &Store, container: &ContainerRef) -> PathBuf {
    let base = sessions_dir(store);
    match container {
        ContainerRef::Worker { task, run } => base.join(task).join(run),
        ContainerRef::Reviewer { task, run, review } => base.join(task).join(run).join(review),
        ContainerRef::Planner { task, run } => base.join(task).join(format!("plan-{run}")),
        ContainerRef::PlanReviewer { task, plan, review } => {
            base.join(task).join(format!("plan-{plan}")).join(review)
        }
        ContainerRef::Pm => base.join("pm"),
    }
}
