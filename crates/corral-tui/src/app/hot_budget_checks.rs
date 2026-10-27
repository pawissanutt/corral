//! Preconditions and expected follow-up work of the hot-reads budget (plan T-253 section 4,
//! "Budget fixture rules" and "Expected work per call"). Preconditions are checked on the
//! measured pass's own samples and output, before its counters are compared.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use corral_core::{ResourceSnapshot, Status, Store};

use super::hot_budget::Cells;
use super::hot_budget_fixture::Fixture;

/// Resource samples each measured path takes on its normal branch, the same at every size:
/// pm next samples in the workset's scoped admission read and its free-slot and dispatch
/// checks, automation status additionally in `automatic_dispatch_status`, and a frame once in
/// `sample_board`.
fn expected_samples(path: &str) -> u64 {
    match path {
        "pm_next" => 4,
        "automation_status" => 5,
        _ => 1,
    }
}

/// Promotion shortlists each path computes: the PM workset's one Idea suggestion.
fn expected_shortlists(path: &str) -> u64 {
    match path {
        "pm_next" | "automation_status" => 1,
        _ => 0,
    }
}

/// What one variant's walks are expected to cost, computed from its own stored rows and layout.
#[derive(Debug, Clone)]
pub(super) struct Expected {
    pub(super) tasks: u64,
    pub(super) walk_rows: u64,
    pub(super) walk_bytes: u64,
    /// One existence probe per coding run of every task, plus each walk-only path's
    /// `existing_parent` probes and free-space query.
    pub(super) sample_stats: u64,
    /// `likely_files` calls per shortlist: the Idea and every compared active task.
    pub(super) likely_per_shortlist: u64,
    pub(super) volumes: BTreeSet<PathBuf>,
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
    let (mut walk_rows, mut walk_bytes) = (0, 0);
    for task in &tasks {
        let _hot = HotScope::enter();
        let before = work::snapshot();
        store.get(&task.id).unwrap();
        let delta = work::snapshot().since(&before);
        walk_rows += Counter::DECODES
            .iter()
            .map(|c| delta.total(*c))
            .sum::<u64>();
        walk_bytes += delta.decoded_doc_bytes();
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

/// Every FU-3 cell of one measured pass against its expected work.
pub(super) fn followup_failures(
    variant: &str,
    key: &str,
    cells: &Cells,
    exp: &Expected,
) -> Vec<String> {
    let mut failures = Vec::new();
    let get = |cell: &str| cells.get(cell).copied().unwrap_or(0);
    for site in ["likely_files", "sample"] {
        let calls = get(&format!("calls.FU-3.{site}"));
        let cell = |counter: &str| get(&format!("{counter}.followup.FU-3.{site}"));
        let rows: u64 = Counter::DECODES.iter().map(|c| cell(c.name())).sum();
        let mut expect = |what: &str, got: u64, want: u64| {
            if got != want {
                failures.push(format!(
                    "{variant}/{key}: FU-3 {site} {what} = {got}, expected {want} ({calls} calls)"
                ));
            }
        };
        expect(
            "hydrations",
            cell("full_task_hydrations"),
            calls * exp.tasks,
        );
        expect("rows", rows, calls * exp.walk_rows);
        expect(
            "doc bytes",
            cell("decoded_doc_bytes"),
            calls * exp.walk_bytes,
        );
        expect("processes", cell("processes"), 0);
        let stats = if site == "sample" {
            calls * exp.sample_stats
        } else {
            0
        };
        expect("stats", cell("file_stats"), stats);
        let views = cell("views_opened");
        if views != 0 && views != calls {
            failures.push(format!(
                "{variant}/{key}: FU-3 {site} views = {views}, expected 0 or {calls}"
            ));
        }
    }
    failures
}
