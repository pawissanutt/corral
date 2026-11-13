//! Preconditions and expected follow-up work of the hot-reads budget (plan T-253 section 4,
//! "Budget fixture rules" and "Expected work per call"). Preconditions are checked on the
//! measured pass's own samples and output, before its counters are compared.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use corral_core::work::{Counter, WorkSnapshot};
use corral_core::{ResourceSnapshot, Store};

use super::hot_budget::Cells;
use super::hot_budget_fixture::Fixture;

#[cfg(test)]
#[path = "hot_budget_checks_tests.rs"]
mod tests;

/// Resource samples each measured path takes on its normal branch, the same at every size:
/// Main d0782aa5 replaced the workset's duplicate scoped-admission fallback with canonical
/// attention. Its free-slot/dispatch checks still sample; automation status additionally
/// samples in `automatic_dispatch_status`, and a frame once in `sample_board`.
/// Only producer counts changed; every per-call FU-3 work/bucket assertion remains intact.
fn expected_samples(path: &str) -> u64 {
    match path {
        "pm_next" => 3,
        "automation_status" => 4,
        // A sync tick: resources::observe, two dispatch_policy admission reads, and
        // main's owning attention reconciliation (each is a real sampled evaluation).
        super::sync_budget::PATH => 4,
        _ => 1,
    }
}

/// Exact resource probe work, from the variant's stored run locations and layout.
#[derive(Debug, Clone)]
pub(super) struct Expected {
    pub(super) tasks: u64,
    /// One existence probe per coding run of every task, plus each walk-only path's
    /// `existing_parent` probes and free-space query.
    pub(super) sample_stats: u64,
    pub(super) volumes: BTreeSet<PathBuf>,
}

impl Expected {
    pub(super) fn encode(&self) -> serde_json::Value {
        serde_json::json!([self.tasks, self.sample_stats, self.volumes])
    }

    pub(super) fn decode(value: &serde_json::Value) -> Expected {
        let n = |i: usize| value[i].as_u64().unwrap();
        Expected {
            tasks: n(0),
            sample_stats: n(1),
            volumes: serde_json::from_value(value[2].clone()).unwrap(),
        }
    }
}

/// The store, each project's `.worktrees` and `target`, and `CARGO_TARGET_DIR`.
fn fixed_volumes(store: &Store) -> BTreeSet<PathBuf> {
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

/// `existing_parent`'s probes up to the first existing ancestor, plus one free-space query.
fn path_stats(path: &Path) -> u64 {
    let mut probes = 0;
    for parent in path.ancestors() {
        probes += 1;
        if parent.exists() {
            return probes + 1;
        }
    }
    panic!("fixture path has no existing ancestor: {}", path.display());
}

pub(super) fn expected(fx: &Fixture) -> Expected {
    let store = &fx.ctx.store;
    let tasks = store.list().unwrap();
    let fixed = fixed_volumes(store);
    let mut volumes = fixed.clone();
    for run in tasks.iter().flat_map(|t| &t.runs) {
        if run.worktree.exists() {
            volumes.insert(run.worktree.clone());
            volumes.insert(run.worktree.join("target"));
        }
    }
    assert_eq!(
        volumes.len() - fixed.len(),
        2 * fx.checkouts.len(),
        "only the fixed current-run checkouts exist"
    );
    let runs: u64 = tasks.iter().map(|t| t.runs.len() as u64).sum();
    let walk_only: u64 = volumes.difference(&fixed).map(|p| path_stats(p)).sum();
    Expected {
        tasks: tasks.len() as u64,
        sample_stats: runs + walk_only,
        volumes,
    }
}

/// The measured pass sampled exactly as expected, every sample is the pinned admitting reading
/// over the expected volumes, and the output shows no machine hold.
pub(super) fn preconditions(
    fx: &Fixture,
    exp: &Expected,
    key: &str,
    path: &str,
    delta: &WorkSnapshot,
    samples: &[ResourceSnapshot],
    output: &str,
) -> Vec<String> {
    let mut failures = Vec::new();
    let mut fail = |what: String| failures.push(format!("{key}: precondition: {what}"));
    let sampled = samples.len() as u64;
    let calls = delta.followup_calls("FU-3", "sample");
    if sampled != expected_samples(path) || sampled == 0 || calls != sampled {
        fail(format!(
            "{sampled} samples ({calls} FU-3 sample calls), expected {}",
            expected_samples(path)
        ));
    }
    for sample in samples {
        let volumes: BTreeSet<PathBuf> = sample.disks.iter().map(|d| d.path.clone()).collect();
        if volumes != exp.volumes {
            fail(format!(
                "volume set {volumes:?}, expected {:?}",
                exp.volumes
            ));
        }
        let pinned = sample.load_one == 0.0
            && sample.load_per_core == 0.0
            && sample.mem_available == u64::MAX
            && sample.disks.iter().all(|d| d.free_bytes == u64::MAX);
        if !pinned {
            fail("a sample does not carry the pinned admitting readings".into());
        }
    }
    if output.contains("machine busy") || output.contains("waiting: machine") {
        fail("the pass reports a machine hold".into());
    }
    if let Err(error) = fx.ctx.store.config.validate(fx.ctx.store.root()) {
        fail(format!("the store config is not valid: {error}"));
    }
    failures
}

/// FU-3 samples read scalar worktree locations without hydrating Tasks or decoding documents.
/// Probe counts and all non-read bounds stay strict.
pub(super) fn followup_failures(
    variant: &str,
    key: &str,
    cells: &Cells,
    exp: &Expected,
) -> Vec<String> {
    let mut failures = Vec::new();
    let get = |cell: &str| cells.get(cell).copied().unwrap_or(0);
    let body_io = get("selected_artifact_bytes_read.total");
    if body_io != 0 {
        failures.push(format!(
            "{variant}/{key}: cold selected-artifact body I/O entered hot path: {body_io}"
        ));
    }
    let site = "sample";
    let calls = get("calls.FU-3.sample");
    let cell = |counter: &str| get(&format!("{counter}.followup.FU-3.sample"));
    let rows: u64 = Counter::DECODES.iter().map(|c| cell(c.name())).sum();
    let mut bound = |what: &str, got: u64, max: u64| {
        if got > max {
            failures.push(format!(
                "{variant}/{key}: FU-3 {site} {what} = {got}, maximum {max} ({calls} calls)"
            ));
        }
    };
    bound("hydrations", cell("full_task_hydrations"), 0);
    bound("rows", rows, 0);
    bound("doc bytes", cell("decoded_doc_bytes"), 0);
    bound("processes", cell("processes"), 0);
    let stats = calls * exp.sample_stats;
    if cell("file_stats") != stats {
        failures.push(format!(
            "{variant}/{key}: FU-3 {site} stats = {}, expected {stats} ({calls} calls)",
            cell("file_stats")
        ));
    }
    let views = cell("views_opened");
    if views != 0 && views != calls {
        failures.push(format!(
            "{variant}/{key}: FU-3 {site} views = {views}, expected 0 or {calls}"
        ));
    }
    failures
}
