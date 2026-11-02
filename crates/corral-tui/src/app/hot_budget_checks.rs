//! Preconditions and expected follow-up work of the hot-reads budget (plan T-253 section 4,
//! "Budget fixture rules" and "Expected work per call"). Preconditions are checked on the
//! measured pass's own samples and output, before its counters are compared.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use corral_core::backend::{PageReq, TaskOrder, TaskQuery};
use corral_core::work::{Counter, WorkSnapshot};
use corral_core::{ResourceSnapshot, Status, Store};

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

/// Promotion shortlists each path computes: the PM workset's one Idea suggestion.
fn expected_shortlists(path: &str) -> u64 {
    match path {
        "pm_next" | "automation_status" | super::sync_budget::PATH => 1,
        _ => 0,
    }
}

/// V2 per-call header-read bounds and exact probe work, from the variant's stored rows/layout.
#[derive(Debug, Clone)]
pub(super) struct Expected {
    pub(super) tasks: u64,
    /// One header per task, never its full history. Samples need no task decodes at all.
    pub(super) walk_rows: u64,
    pub(super) walk_bytes: u64,
    /// One existence probe per coding run of every task, plus each walk-only path's
    /// `existing_parent` probes and free-space query.
    pub(super) sample_stats: u64,
    /// `likely_files` calls per shortlist: the Idea and every compared active task.
    pub(super) likely_per_shortlist: u64,
    pub(super) volumes: BTreeSet<PathBuf>,
    /// Header bytes after the measured pass's own write; either version bounds a later read.
    pub(super) walk_bytes_after: Option<u64>,
}

impl Expected {
    pub(super) fn encode(&self) -> serde_json::Value {
        serde_json::json!([
            self.tasks,
            self.walk_rows,
            self.walk_bytes,
            self.sample_stats,
            self.likely_per_shortlist,
            self.volumes,
            self.walk_bytes_after,
        ])
    }

    pub(super) fn decode(value: &serde_json::Value) -> Expected {
        let n = |i: usize| value[i].as_u64().unwrap();
        Expected {
            tasks: n(0),
            walk_rows: n(1),
            walk_bytes: n(2),
            sample_stats: n(3),
            likely_per_shortlist: n(4),
            volumes: serde_json::from_value(value[5].clone()).unwrap(),
            walk_bytes_after: value[6].as_u64(),
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
    // Bound title scans by stored headers, not the v1 full-Task hydration cost. Compute
    // payload sizes outside the measured scope, independently of the pass's work counters.
    let view = store.view().unwrap();
    let query = TaskQuery {
        statuses: Vec::new(),
        project: None,
        order: TaskOrder::IdAsc,
    };
    let mut page = PageReq::first(500);
    let (mut walk_rows, mut walk_bytes) = (0, 0);
    loop {
        let found = view.tasks(&query, page.clone()).unwrap();
        for header in &found.items {
            walk_rows += 1;
            walk_bytes += serde_json::to_vec(header).unwrap().len() as u64;
        }
        match found.next {
            Some(next) => page.after = Some(next),
            None => break,
        }
    }
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
    let compared = tasks
        .iter()
        .filter(|t| t.project.as_deref() == Some("app") && !t.tracker)
        .filter(|t| matches!(t.status, Status::Running | Status::NeedsMe) || t.launch.is_some())
        .count() as u64;
    Expected {
        tasks: tasks.len() as u64,
        walk_rows,
        walk_bytes,
        sample_stats: runs + walk_only,
        likely_per_shortlist: 1 + compared,
        volumes,
        walk_bytes_after: None,
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
    let likely = delta.followup_calls("FU-3", "likely_files");
    let want = expected_shortlists(path) * exp.likely_per_shortlist;
    if likely != want {
        fail(format!("{likely} likely_files calls, expected {want}"));
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

/// FU-3 may read at most one header walk per likely-files call (a pass can reuse it).
/// V2 samples read scalar worktree locations, so neither site may hydrate full Tasks and
/// samples may not decode task documents. Probe counts and all non-read bounds stay strict.
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
    for site in ["likely_files", "sample"] {
        let calls = get(&format!("calls.FU-3.{site}"));
        let cell = |counter: &str| get(&format!("{counter}.followup.FU-3.{site}"));
        let rows: u64 = Counter::DECODES.iter().map(|c| cell(c.name())).sum();
        let mut bound = |what: &str, got: u64, max: u64| {
            if got > max {
                failures.push(format!(
                    "{variant}/{key}: FU-3 {site} {what} = {got}, maximum {max} ({calls} calls)"
                ));
            }
        };
        bound("hydrations", cell("full_task_hydrations"), 0);
        let (max_rows, max_bytes) = if site == "likely_files" {
            let bytes = exp.walk_bytes.max(exp.walk_bytes_after.unwrap_or(0));
            (calls * exp.walk_rows, calls * bytes)
        } else {
            (0, 0)
        };
        bound("rows", rows, max_rows);
        bound("doc bytes", cell("decoded_doc_bytes"), max_bytes);
        bound("processes", cell("processes"), 0);
        let stats = if site == "sample" {
            calls * exp.sample_stats
        } else {
            0
        };
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
    }
    failures
}
