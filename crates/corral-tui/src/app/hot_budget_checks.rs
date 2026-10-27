//! Preconditions of the hot-reads budget (plan T-253 section 4, "Budget fixture rules"),
//! checked before counters mean anything: the pinned probe admits, the sample's volume set is
//! exactly the one the fixture layout implies, and no resource state holds work.

use std::collections::BTreeSet;
use std::path::PathBuf;

use corral_core::Store;

use super::hot_budget_fixture::Fixture;

/// The store, each project's `.worktrees` and `target`, and `CARGO_TARGET_DIR`; the fixture
/// keeps no run worktree, so no walk-only path joins them.
fn expected_volumes(store: &Store) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::from([store.dir().to_path_buf()]);
    for name in store.config.projects.keys() {
        let project = store.project_path(name).unwrap();
        paths.insert(project.join(".worktrees"));
        paths.insert(project.join("target"));
    }
    if let Some(target) = std::env::var_os("CARGO_TARGET_DIR").filter(|v| !v.is_empty()) {
        let target = PathBuf::from(target);
        paths.insert(if target.is_absolute() {
            target
        } else {
            std::env::current_dir().unwrap().join(target)
        });
    }
    paths
}

pub(super) fn preconditions(fx: &Fixture) -> Vec<String> {
    let mut failures = Vec::new();
    let state = match corral_ops::resources::read_status(&fx.ctx) {
        Ok(state) => state,
        Err(error) => return vec![format!("precondition: resource status failed: {error:#}")],
    };
    if state.held || state.severe || state.reason.is_some() {
        failures.push(format!(
            "precondition: resource state holds work: held={} severe={} reason={:?}",
            state.held, state.severe, state.reason
        ));
    }
    let volumes: BTreeSet<PathBuf> = state
        .snapshot
        .disks
        .iter()
        .map(|d| d.path.clone())
        .collect();
    let expected = expected_volumes(&fx.ctx.store);
    if volumes != expected {
        failures.push(format!(
            "precondition: volume set {volumes:?}, expected {expected:?}"
        ));
    }
    if let Err(error) = fx.ctx.store.config.validate(fx.ctx.store.root()) {
        failures.push(format!(
            "precondition: the store config is not valid: {error}"
        ));
    }
    failures
}
