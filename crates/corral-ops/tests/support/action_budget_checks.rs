//! Preconditions and expected work of the action budget (plan T-253 section 4, "Budget fixture
//! rules", "Expected work per call" and "Comparing variants"), checked on the measured pass's own
//! samples, effects and cells before variants are compared.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path as FsPath, PathBuf};

use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use corral_core::{FindingSource, ResourceSnapshot, SubmissionDisposition};

use super::action_budget_fixture::{Bench, Kind};

pub(super) type Cells = BTreeMap<String, u64>;

/// One full walk of the store, as the FU-3 sites read it: every task hydrated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct Walk {
    pub(super) tasks: u64,
    pub(super) rows: u64,
    pub(super) bytes: u64,
    /// One existence probe per coding run of every task, plus the walk-only paths'
    /// `existing_parent` probes and free-space queries.
    pub(super) stats: u64,
}

/// The store, each project's `.worktrees` and `target`, and `CARGO_TARGET_DIR`.
fn fixed_volumes(store: &corral_core::Store) -> BTreeSet<PathBuf> {
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
fn path_stats(path: &FsPath) -> u64 {
    let mut probes = 0;
    for parent in path.ancestors() {
        probes += 1;
        if parent.exists() {
            return probes + 1;
        }
    }
    panic!("fixture path has no existing ancestor: {}", path.display());
}

/// The walk on the store as it stands, and the volumes a resource sample covers.
pub(super) fn walk(bench: &Bench) -> (Walk, BTreeSet<PathBuf>) {
    store_walk(&bench.ctx.store)
}

pub(super) fn store_walk(store: &corral_core::Store) -> (Walk, BTreeSet<PathBuf>) {
    let tasks = store.list().unwrap();
    let (mut rows, mut bytes) = (0, 0);
    for task in &tasks {
        let _hot = HotScope::enter();
        let before = work::snapshot();
        store.get(&task.id).unwrap();
        let delta = work::snapshot().since(&before);
        rows += Counter::DECODES
            .iter()
            .map(|c| delta.total(*c))
            .sum::<u64>();
        bytes += delta.decoded_doc_bytes();
    }
    let fixed = fixed_volumes(store);
    let mut volumes = fixed.clone();
    for run in tasks.iter().flat_map(|t| &t.runs) {
        if run.worktree.exists() {
            volumes.insert(run.worktree.clone());
            volumes.insert(run.worktree.join("target"));
        }
    }
    let runs: u64 = tasks.iter().map(|t| t.runs.len() as u64).sum();
    let walk_only: u64 = volumes.difference(&fixed).map(|p| path_stats(p)).sum();
    let walk = Walk {
        tasks: tasks.len() as u64,
        rows,
        bytes,
        stats: runs + walk_only,
    };
    (walk, volumes)
}

/// What an action's durable effect leaves on its task and repository, read outside the counters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Effects {
    status: corral_core::Status,
    runs: usize,
    panes: usize,
    plan_runs: usize,
    plan_reviews: usize,
    approved: bool,
    children: usize,
    messages: u64,
    pending_reply: Option<String>,
    /// `changes_requested` events whose text was submitted to the run's pane.
    submitted_changes: usize,
    accepted: usize,
    main: String,
}

pub(super) fn effects(bench: &Bench) -> Effects {
    let task = bench.ctx.store.get(&bench.id).unwrap();
    let repo = bench.ctx.store.project_path("app").unwrap();
    let current = task.current_run();
    Effects {
        status: task.status,
        runs: task.runs.len(),
        panes: task.runs.iter().filter(|run| run.herdr.is_some()).count(),
        plan_runs: task.plan_runs.len(),
        plan_reviews: task.plans.last().map_or(0, |plan| plan.reviews.len()),
        approved: task
            .plans
            .last()
            .is_some_and(|plan| plan.approval.is_some()),
        children: task.children.len(),
        messages: current.map_or(0, |run| run.inbox.next_seq),
        pending_reply: current.and_then(|run| run.pending_reply.clone()),
        submitted_changes: bench
            .ctx
            .store
            .events()
            .unwrap()
            .iter()
            .filter(|event| {
                event.task.as_deref() == Some(bench.id.as_str())
                    && event.kind == "changes_requested"
                    && event.detail["submitted"] == true
            })
            .count(),
        accepted: task
            .aggregate_attempts
            .iter()
            .filter(|attempt| attempt.acceptance.is_some())
            .count(),
        main: corral_git::rev_sha(&repo, "main").unwrap_or_default(),
    }
}

/// The selected path's own durable effect happened exactly once: its transport, publication,
/// message or landing, not only a Submitted authorization.
pub(super) fn effect_failures(kind: Kind, b: &Effects, a: &Effects) -> Vec<String> {
    use corral_core::Status;
    let launched = a.runs == b.runs + 1 && a.panes == b.panes + 1 && a.status == Status::Running;
    let (ok, what) = match kind {
        Kind::Worker => (launched, "one launched run with its pane"),
        Kind::Planner => (a.plan_runs == b.plan_runs + 1, "one planning run"),
        Kind::PlanReviewPane | Kind::PlanReviewPm => {
            (a.plan_reviews == b.plan_reviews + 1, "one plan review")
        }
        Kind::AcceptPlan => (
            !b.approved && a.approved && launched,
            "one acceptance and launch",
        ),
        Kind::AcceptPlanScopes => (
            !b.approved && a.approved && b.children == 0 && a.children == 2,
            "one acceptance publishing two children",
        ),
        Kind::RepairInbox => (
            b.status == Status::Review
                && a.status == Status::Running
                && a.messages == b.messages + 1,
            "one changes request in the run's inbox",
        ),
        Kind::RepairLegacy => (
            b.status == Status::Review
                && a.status == Status::Running
                && a.submitted_changes == b.submitted_changes + 1
                && a.pending_reply.is_none(),
            "one changes request submitted to the run's pane",
        ),
        Kind::AggregateCode => (
            a.accepted == b.accepted + 1 && a.main != b.main,
            "one acceptance landing on main",
        ),
        Kind::AggregateNoCode => (
            a.accepted == b.accepted + 1 && a.main == b.main,
            "one acceptance without a landing",
        ),
    };
    if ok {
        Vec::new()
    } else {
        vec![format!(
            "effect: expected {what}; after {a:?}; before {b:?}"
        )]
    }
}

pub(super) fn submitted(bench: &Bench, kind: Kind) -> usize {
    super::authorizations(&bench.ctx, &bench.id, kind.action())
        .into_iter()
        .filter(|record| matches!(record.disposition, SubmissionDisposition::Submitted))
        .count()
}

/// The measured pass sampled with the pinned admitting readings over the expected volumes,
/// showed no machine hold, and took the selected action's durable effect exactly once.
pub(super) fn preconditions(
    bench: &Bench,
    kind: Kind,
    (volumes, after): (&BTreeSet<PathBuf>, &BTreeSet<PathBuf>),
    delta: &WorkSnapshot,
    samples: &[(ResourceSnapshot, Walk)],
    output: &str,
    effects: usize,
) -> Vec<String> {
    let mut failures = Vec::new();
    let mut fail = |what: String| failures.push(format!("precondition: {what}"));
    let sampled = samples.len() as u64;
    let calls = delta.followup_calls("FU-3", "sample");
    if calls != sampled || (kind.ticked() && sampled == 0) {
        fail(format!("{sampled} samples, {calls} FU-3 sample calls"));
    }
    for (sample, _) in samples {
        let seen: BTreeSet<PathBuf> = sample.disks.iter().map(|d| d.path.clone()).collect();
        // A launch creates its worktree mid-pass; later samples cover it.
        if &seen != volumes && &seen != after {
            fail(format!(
                "volume set {seen:?}, expected {volumes:?} or {after:?}"
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
    if effects != 1 {
        fail(format!(
            "{effects} submitted {:?} actions, expected exactly one",
            kind.action()
        ));
    }
    if let Err(error) = bench.ctx.store.config.validate(bench.ctx.store.root()) {
        fail(format!("the store config is not valid: {error}"));
    }
    failures
}

/// Every FU-3 cell against its expected work: each call walks the store as it stood before or
/// after the pass's own write.
pub(super) fn followup_failures(
    cells: &Cells,
    before: &Walk,
    after: &Walk,
    samples: &[Walk],
    live_samples: u64,
) -> Vec<String> {
    let mut failures = Vec::new();
    let get = |cell: &str| cells.get(cell).copied().unwrap_or(0);
    let sampled = samples.iter().fold((0, 0, 0, 0), |a, w| {
        (a.0 + w.tasks, a.1 + w.rows, a.2 + w.bytes, a.3 + w.stats)
    });
    for site in ["likely_files", "sample"] {
        let calls = get(&format!("calls.FU-3.{site}"));
        let cell = |counter: &str| get(&format!("{counter}.followup.FU-3.{site}"));
        let rows: u64 = Counter::DECODES.iter().map(|c| cell(c.name())).sum();
        let observed = (
            cell("full_task_hydrations"),
            rows,
            cell("decoded_doc_bytes"),
            cell("file_stats"),
        );
        let stats = |walk: &Walk| if site == "sample" { walk.stats } else { 0 };
        let fits = (site == "sample" && observed == sampled)
            || (site != "sample")
                && (0..=calls).any(|k| {
                    let mix = |a: u64, b: u64| k * a + (calls - k) * b;
                    observed
                        == (
                            mix(before.tasks, after.tasks),
                            mix(before.rows, after.rows),
                            mix(before.bytes, after.bytes),
                            mix(stats(before), stats(after)),
                        )
                });
        if !fits {
            failures.push(format!(
                "FU-3 {site}: (hydrations, rows, bytes, stats) = {observed:?} over {calls} calls, \
                 expected each call's walk {before:?} or {after:?}"
            ));
        }
        if cell("processes") != 0 {
            failures.push(format!("FU-3 {site}: processes = {}", cell("processes")));
        }
        // A sample on a live store opens one view; one on a pass snapshot lists through the
        // snapshot's view. Likely files read on views the pass already holds.
        let views = cell("views_opened");
        let want = if site == "sample" { live_samples } else { 0 };
        if views != want {
            failures.push(format!(
                "FU-3 {site}: views = {views} over {calls} calls, expected {want}"
            ));
        }
    }
    failures
}

/// The emitted sections a kind's prompt shows, each with the times its pass builds it.
pub(super) fn sections(kind: Kind) -> &'static [(&'static str, u64)] {
    match kind {
        Kind::Worker => &[("comments", 1), ("resume", 2)],
        Kind::Planner => &[("comments", 1), ("plan_findings", 1)],
        Kind::PlanReviewPane | Kind::PlanReviewPm => &[("comments", 1), ("plan_findings", 1)],
        // The accepting tick admits and launches the implementation run; it has no source run.
        Kind::AcceptPlan => &[("comments", 1)],
        _ => &[],
    }
}

/// The emitted (rows, stored bytes) one build of `section` reads on the bench, measured by the
/// store's own emitted readers before the pass.
pub(super) fn section_cells(bench: &Bench, section: &str) -> (u64, u64) {
    let head = bench.ctx.store.head(&bench.id).unwrap();
    let _hot = HotScope::enter();
    let before = work::snapshot();
    match section {
        "comments" => {
            head.emitted_comments().unwrap();
        }
        "plan_findings" => {
            head.emitted_findings(FindingSource::Plan, None).unwrap();
        }
        "resume" => {
            head.emitted_obligations("R1").unwrap();
            let meta = head
                .checkpoint_metas_newest_first("R1")
                .unwrap()
                .next()
                .map(Result::unwrap);
            if let Some(meta) = meta {
                head.emitted_checkpoint(&meta).unwrap();
            }
        }
        other => panic!("unknown section {other}"),
    }
    let delta = work::snapshot().since(&before);
    (delta.emitted_rows(), delta.emitted_doc_bytes())
}
