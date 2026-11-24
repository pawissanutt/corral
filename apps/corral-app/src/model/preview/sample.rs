//! The sample task records: twenty tasks covering every group and closed state, with their
//! runs, checks and artifacts. Every instant is relative to the caller's clock.

use corral_protocol::detail::TaskDetail;
use corral_types::{
    Actor, Artifact, ArtifactKind, Blocked, Check, CheckKind, CheckResult, Next, Op, Outcome,
    Priority, Role, Run, SessionRef, Status, Task, TaskDifficulty,
};
use std::path::PathBuf;

use super::ago;
use crate::model::daemon::Feed;

pub(super) const MINUTE: u64 = 60;
pub(super) const HOUR: u64 = 3600;
pub(super) const DAY: u64 = 86_400;

/// Every sample task record, in id order.
pub(super) fn records(feed: &Feed) -> Vec<TaskDetail> {
    SEEDS.iter().map(|seed| record(feed, seed)).collect()
}

struct Seed {
    id: &'static str,
    title: &'static str,
    status: Status,
    project: Option<&'static str>,
    parent: Option<&'static str>,
    priority: Priority,
    /// Seconds before the clock: created, last updated.
    created: u64,
    updated: u64,
    goal: &'static str,
}

const fn seed(
    id: &'static str,
    title: &'static str,
    status: Status,
    project: Option<&'static str>,
    parent: Option<&'static str>,
    (created, updated): (u64, u64),
    goal: &'static str,
) -> Seed {
    Seed {
        id,
        title,
        status,
        project,
        parent,
        priority: Priority::P2,
        created,
        updated,
        goal,
    }
}

const CORRAL: Option<&str> = Some("corral");
const PLAN: Option<&str> = Some("T-1");

const SEEDS: &[Seed] = &[
    Seed {
        priority: Priority::P1,
        ..seed(
            "T-1",
            "Simple engine app",
            Status::Backlog,
            CORRAL,
            None,
            (8 * DAY, 2 * HOUR),
            "",
        )
    },
    seed(
        "T-2",
        "Shared Actions menu",
        Status::Done,
        CORRAL,
        PLAN,
        (8 * DAY - HOUR, 2 * HOUR),
        "One Actions menu lists every engine operation for a task, always enabled.",
    ),
    seed(
        "T-3",
        "Builds destination",
        Status::Dropped,
        CORRAL,
        PLAN,
        (8 * DAY - 2 * HOUR, DAY),
        "A Builds page for published build sessions.",
    ),
    seed(
        "T-4",
        "Refusal sheet with typed next steps",
        Status::Done,
        CORRAL,
        None,
        (7 * DAY, 26 * HOUR),
        "A refused operation shows its reason and one button per next step.",
    ),
    seed(
        "T-5",
        "One op.submit on the wire",
        Status::Done,
        CORRAL,
        None,
        (7 * DAY - HOUR, 2 * DAY),
        "Every engine operation travels as one op.submit request.",
    ),
    seed(
        "T-6",
        "Landing page copy",
        Status::Done,
        Some("site"),
        None,
        (6 * DAY, 3 * DAY),
        "The landing page says what Corral does in two sentences.",
    ),
    seed(
        "T-7",
        "Device authority editor",
        Status::Dropped,
        CORRAL,
        None,
        (6 * DAY - HOUR, 4 * DAY),
        "Per-device authority switches in Settings.",
    ),
    Seed {
        priority: Priority::P1,
        ..seed(
            "T-8",
            "Restore paused sessions after a daemon restart",
            Status::Blocked,
            CORRAL,
            PLAN,
            (4 * DAY, 25 * MINUTE),
            "A paused session is still paused after corrald restarts.",
        )
    },
    Seed {
        priority: Priority::P0,
        ..seed(
            "T-9",
            "Pairing page survives a refresh",
            Status::Blocked,
            CORRAL,
            None,
            (3 * DAY + 12 * HOUR, 50 * MINUTE),
            "A paired browser stays paired across a page refresh.",
        )
    },
    seed(
        "T-10",
        "Usage page explains missing records",
        Status::Review,
        CORRAL,
        None,
        (2 * DAY + 12 * HOUR, 30 * MINUTE),
        "The Usage page says what its totals cover and why some runs have no records.",
    ),
    Seed {
        priority: Priority::P1,
        ..seed(
            "T-11",
            "Collapse the sidebar on narrow screens",
            Status::Review,
            CORRAL,
            PLAN,
            (2 * DAY, 40 * MINUTE),
            "The sidebar folds to its rail below the narrow breakpoint and stays available.",
        )
    },
    seed(
        "T-12",
        "Timeline zoom presets",
        Status::Ready,
        CORRAL,
        PLAN,
        (30 * HOUR, 5 * HOUR),
        "Hour, day and week presets for the Timeline.",
    ),
    seed(
        "T-13",
        "Changes list marks forced events",
        Status::Review,
        CORRAL,
        None,
        (28 * HOUR, 8 * MINUTE),
        "A forced event in Changes lists what it skipped.",
    ),
    Seed {
        priority: Priority::P1,
        ..seed(
            "T-14",
            "Real dates on the timeline",
            Status::Running,
            CORRAL,
            PLAN,
            (26 * HOUR, 2 * HOUR),
            "Timeline axes show the real calendar dates of the runs they draw.",
        )
    },
    seed(
        "T-15",
        "Terminal keys bar for foreign harnesses",
        Status::Running,
        Some("site"),
        None,
        (22 * HOUR, 4 * HOUR),
        "A keys bar under a foreign harness's pane sends Enter, Escape and arrows.",
    ),
    seed(
        "T-16",
        "Inline artifact previews",
        Status::Ready,
        CORRAL,
        None,
        (20 * HOUR, 9 * HOUR),
        "Text artifacts open inline on the task page.",
    ),
    seed(
        "T-17",
        "Edit corral.toml from Settings",
        Status::Backlog,
        CORRAL,
        None,
        (18 * HOUR, 7 * HOUR),
        "Settings edits the raw corral.toml and shows the daemon's validation error.",
    ),
    seed(
        "T-18",
        "Phone lock-screen notifications",
        Status::Backlog,
        None,
        None,
        (15 * HOUR, 15 * HOUR),
        "A blocked task notifies the paired phone.",
    ),
    seed(
        "T-19",
        "Group changes by plan",
        Status::Idea,
        None,
        None,
        (10 * HOUR, 10 * HOUR),
        "Changes can be grouped under their parent task.",
    ),
    seed(
        "T-20",
        "Keep gate results across rebases",
        Status::Idea,
        CORRAL,
        None,
        (6 * HOUR, 6 * HOUR),
        "A rebase that leaves the tree unchanged keeps its green gate.",
    ),
];

/// The sample repository's local branches besides the task branches: where a landing can go.
pub(super) const BRANCHES: &[&str] = &["main", "release/0.9"];

const PLAN_BODY: &str = concat!(
    "## Goal\n",
    "The app runs on the simple engine: one task list, one Actions menu, typed refusals.\n",
    "\n",
    "## Steps\n",
    "1. Shared Actions menu (T-2)\n",
    "2. Restore paused sessions (T-8)\n",
    "3. Narrow sidebar (T-11)\n",
    "4. Real timeline dates (T-14), then zoom presets (T-12)\n",
    "\n",
    "## Acceptance\n",
    "- Every engine operation is on every task row, enabled; the engine decides.\n",
    "- A refusal shows its reason and one button per next step, the forced one last.\n",
    "- A destructive or forced operation asks once, on this device.\n",
    "- Review lands onto the run's base unless another existing branch is named.\n",
    "\n",
    "## Order\n",
    "T-12 waits on T-14: zoom presets need the real dates first. The rest can run side by\n",
    "side; each lands on its own once its gate and review pass.\n",
    "\n",
    "## Notes\n",
    "Builds merge into Agents; the Builds destination was dropped (T-3).\n",
    "The plan is done when every member is done and the owner marks T-1 done.",
);

/// The size, difficulty and tags of the sample tasks that have them; T-8 keeps a size from
/// before the scale, and T-11 a custom tag with a blank.
fn planning(
    id: &str,
) -> (
    Option<&'static str>,
    Option<TaskDifficulty>,
    &'static [&'static str],
) {
    use TaskDifficulty::{Hard, Routine, Standard};
    match id {
        "T-8" => (Some("medium"), None, &[]),
        "T-9" => (Some("s"), Some(Hard), &["bug"]),
        "T-10" => (Some("m"), Some(Standard), &["documentation", "testing"]),
        "T-11" => (Some("xs"), Some(Routine), &["accessibility", "ui polish"]),
        "T-12" => (Some("s"), Some(Routine), &["performance"]),
        "T-13" => (Some("m"), Some(Hard), &["testing"]),
        _ => (None, None, &[]),
    }
}

fn record(feed: &Feed, seed: &Seed) -> TaskDetail {
    let (size, difficulty, tags) = planning(seed.id);
    let body = if seed.id == "T-1" {
        PLAN_BODY.to_owned()
    } else {
        format!(
            "## Goal\n{}\n\n## Acceptance\n- The behaviour is covered by one test at the lowest layer.",
            seed.goal
        )
    };
    let mut record = TaskDetail {
        task: Task {
            id: seed.id.into(),
            title: seed.title.into(),
            body,
            status: seed.status,
            project: seed.project.map(str::to_owned),
            priority: seed.priority,
            kind: (seed.id == "T-1").then(|| "plan".into()),
            size: size.map(str::to_owned),
            difficulty,
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
            profile: None,
            base: None,
            parent: seed.parent.map(str::to_owned),
            depends_on: Vec::new(),
            blocked: None,
            round: 0,
            created: ago(feed, seed.created),
            updated: ago(feed, seed.updated),
        },
        runs: Vec::new(),
        checks: Vec::new(),
        artifacts: Vec::new(),
        events: Vec::new(),
    };
    work(feed, &mut record);
    papers(feed, &mut record);
    record
}

/// Summaries, reports, a plan, evidence and screenshots beside the diffs and gate logs, so the
/// artifact finder has every kind to show and pages to walk.
fn papers(feed: &Feed, record: &mut TaskDetail) {
    let papers: &[(ArtifactKind, &str, u64)] = match record.task.id.as_str() {
        "T-1" => &[(ArtifactKind::Reference, "plan.md", 8 * DAY - 30 * MINUTE)],
        "T-2" => &[
            (
                ArtifactKind::Summary,
                "worker-summary.md",
                2 * HOUR + 20 * MINUTE,
            ),
            (
                ArtifactKind::Report,
                "actions-menu-report.md",
                2 * HOUR + 10 * MINUTE,
            ),
        ],
        "T-4" => &[(
            ArtifactKind::Evidence,
            "refusal-sheet-trace.txt",
            26 * HOUR + 30 * MINUTE,
        )],
        "T-6" => &[
            (
                ArtifactKind::Evidence,
                "landing-hero-mobile.png",
                3 * DAY - HOUR,
            ),
            (
                ArtifactKind::Report,
                "landing-copy-report.md",
                3 * DAY - 2 * HOUR,
            ),
        ],
        "T-9" => &[
            (ArtifactKind::Evidence, "reload-trace.txt", 19 * HOUR),
            (
                ArtifactKind::Evidence,
                "paired-after-reload.png",
                18 * HOUR + 30 * MINUTE,
            ),
        ],
        "T-10" => &[(
            ArtifactKind::Report,
            "usage-coverage-report.md",
            35 * MINUTE,
        )],
        "T-11" => &[(
            ArtifactKind::Evidence,
            "sidebar-80x24.png",
            2 * HOUR + 40 * MINUTE,
        )],
        "T-13" => &[(ArtifactKind::Summary, "worker-summary.md", 55 * MINUTE)],
        "T-15" => &[(ArtifactKind::Evidence, "lighthouse-scores.txt", HOUR)],
        _ => &[],
    };
    for (kind, title, at) in papers {
        let paper = artifact(feed, record, *kind, title, *at);
        record.artifacts.push(paper);
    }
}

/// Runs, checks, artifacts and blocks for the tasks that have been worked on.
fn work(feed: &Feed, record: &mut TaskDetail) {
    let id = record.task.id.clone();
    let landed = |record: &mut TaskDetail, started: u64, landed: u64, profile: &str| {
        let mut run = run(feed, record, Role::Worker, profile, started);
        finish(feed, &mut run, landed, Outcome::Landed);
        record.runs.push(run);
        record
            .checks
            .push(gate(feed, record, true, None, landed + 40 * MINUTE));
        record.artifacts.push(artifact(
            feed,
            record,
            ArtifactKind::Diff,
            "diff",
            landed + HOUR,
        ));
        record.artifacts.push(artifact(
            feed,
            record,
            ArtifactKind::Log,
            "gate log",
            landed + 40 * MINUTE,
        ));
    };
    match id.as_str() {
        "T-2" => {
            landed(record, 30 * HOUR, 2 * HOUR, "omp-sol");
            record
                .checks
                .push(review(feed, record, true, None, 2 * HOUR + 30 * MINUTE));
        }
        "T-4" => {
            landed(record, 3 * DAY, 26 * HOUR, "omp-sol");
            let red = gate(
                feed,
                record,
                false,
                Some("tui::snapshot::narrow timed out after 60s"),
                27 * HOUR,
            );
            record.checks = vec![red];
        }
        "T-5" => landed(record, 4 * DAY, 2 * DAY, "omp-sol"),
        // Closed headless runs: the first keeps its log, the stopped second had it pruned.
        "T-6" => {
            landed(record, 4 * DAY, 3 * DAY, "codex-terra");
            if let Some(run) = record.runs.last_mut() {
                headless(run, 31207);
            }
        }
        "T-7" => {
            let mut run = run(feed, record, Role::Worker, "codex-terra", 5 * DAY);
            finish(feed, &mut run, 4 * DAY + HOUR, Outcome::Stopped);
            headless(&mut run, 29114);
            record.runs.push(run);
        }
        "T-8" => {
            record
                .runs
                .push(run(feed, record, Role::Worker, "omp-sol", 3 * HOUR));
            record.task.blocked = Some(Blocked {
                reason: "worker needs a reply".into(),
                detail: Some("Should a paused session stay paused after corrald restarts, or resume its queue? Resuming may deliver queued messages twice.".into()),
                next: vec![
                    Next::run(Op::Reply { id: id.clone(), text: String::new() }),
                    Next::run(Op::Retry { id: id.clone(), profile: None, fresh: false, note: None }),
                    Next::run(Op::Stop { id: id.clone() }),
                ],
            });
        }
        "T-9" => {
            let notes = "The paired marker is cleared on reload before the IndexedDB read resolves; keep it until the read finishes.";
            let mut first = run(feed, record, Role::Worker, "omp-sol", 30 * HOUR);
            finish(feed, &mut first, 20 * HOUR, Outcome::Done);
            record.runs.push(first);
            let mut second = run(feed, record, Role::Worker, "omp-sol", 18 * HOUR);
            second.quiet_since = Some(ago(feed, 45 * MINUTE));
            record.runs.push(second);
            record.task.round = 2;
            record
                .checks
                .push(gate(feed, record, true, None, 19 * HOUR + 30 * MINUTE));
            record
                .checks
                .push(review(feed, record, false, Some(notes), 50 * MINUTE));
            record.artifacts.push(artifact(
                feed,
                record,
                ArtifactKind::Diff,
                "diff",
                20 * HOUR,
            ));
            record.task.blocked = Some(Blocked {
                reason: "automatic change rounds exhausted".into(),
                detail: Some(notes.into()),
                next: vec![
                    Next::run(Op::Changes {
                        id: id.clone(),
                        text: String::new(),
                    }),
                    Next::run(Op::Retry {
                        id: id.clone(),
                        profile: None,
                        fresh: false,
                        note: None,
                    }),
                    Next::force(Op::Approve {
                        id: id.clone(),
                        onto: None,
                    }),
                ],
            });
        }
        "T-10" => {
            let mut run = run(feed, record, Role::Worker, "omp-sol", 7 * HOUR);
            finish(feed, &mut run, 40 * MINUTE, Outcome::Done);
            record.runs.push(run);
            record.artifacts.push(artifact(
                feed,
                record,
                ArtifactKind::Diff,
                "diff",
                40 * MINUTE,
            ));
            let mut red = gate(
                feed,
                record,
                false,
                Some("usage::coverage::uncovered_runs_are_named: left 3, right 2"),
                30 * MINUTE,
            );
            let log = artifact(feed, record, ArtifactKind::Log, "gate log", 30 * MINUTE);
            red.log = Some(log.reference());
            record.artifacts.push(log);
            record.checks.push(red);
        }
        "T-11" => {
            let mut worker = run(feed, record, Role::Worker, "omp-sol", 9 * HOUR);
            finish(feed, &mut worker, 3 * HOUR, Outcome::Done);
            record.runs.push(worker);
            let mut reviewer = run(
                feed,
                record,
                Role::Reviewer,
                "omp-sol-review",
                2 * HOUR + 30 * MINUTE,
            );
            finish(feed, &mut reviewer, 45 * MINUTE, Outcome::Done);
            record.runs.push(reviewer);
            record
                .checks
                .push(gate(feed, record, true, None, 2 * HOUR + 50 * MINUTE));
            record.checks.push(review(
                feed,
                record,
                true,
                Some("Rail keeps every destination; focus order unchanged."),
                45 * MINUTE,
            ));
            record
                .artifacts
                .push(artifact(feed, record, ArtifactKind::Diff, "diff", 3 * HOUR));
            record.artifacts.push(artifact(
                feed,
                record,
                ArtifactKind::Log,
                "gate log",
                2 * HOUR + 50 * MINUTE,
            ));
            record.artifacts.push(artifact(
                feed,
                record,
                ArtifactKind::Report,
                "narrow-sidebar-report.html",
                3 * HOUR,
            ));
        }
        "T-12" => record.task.depends_on = vec!["T-14".into()],
        // A done, a dropped and a mistyped dependency, each shown with what corrald knows.
        "T-16" => {
            record.task.depends_on = vec!["T-2".into(), "T-3".into(), "T-41".into()];
        }
        // Sent back once: the reviewer asked for changes on the first submission, the second
        // passed its gate, and the same reviewer session now judges round 1.
        "T-13" => {
            let mut first = run(feed, record, Role::Worker, "omp-sol", 5 * HOUR);
            finish(feed, &mut first, 3 * HOUR, Outcome::Done);
            record.runs.push(first);
            record
                .checks
                .push(gate(feed, record, true, None, 2 * HOUR + 55 * MINUTE));
            record.runs.push(run(
                feed,
                record,
                Role::Reviewer,
                "omp-sol-review",
                2 * HOUR + 50 * MINUTE,
            ));
            record.checks.push(review(
                feed,
                record,
                false,
                Some("A forced line names the operation but not what it skipped; list the skipped checks after it."),
                2 * HOUR + 20 * MINUTE,
            ));
            let mut second = run(
                feed,
                record,
                Role::Worker,
                "omp-sol",
                2 * HOUR + 15 * MINUTE,
            );
            finish(feed, &mut second, HOUR, Outcome::Done);
            record.runs.push(second);
            record.task.round = 1;
            record
                .checks
                .push(gate(feed, record, true, None, 30 * MINUTE));
            record
                .artifacts
                .push(artifact(feed, record, ArtifactKind::Diff, "diff", HOUR));
        }
        "T-14" => record
            .runs
            .push(run(feed, record, Role::Worker, "omp-sol", 2 * HOUR)),
        // A headless run: a detached process writing `session.log`, which takes no keys.
        "T-15" => {
            let mut run = run(feed, record, Role::Worker, "codex-terra", 4 * HOUR);
            run.quiet_since = Some(ago(feed, 25 * MINUTE));
            headless(&mut run, 48213);
            record.runs.push(run);
        }
        "T-17" => record.task.body.push_str(
            "\n\nKeep comments and formatting: the daemon writes the TOML document as typed.",
        ),
        _ => {}
    }
}

/// A new run of `record`, numbered after its existing runs and started `started` seconds ago.
pub(super) fn run(
    feed: &Feed,
    record: &TaskDetail,
    role: Role,
    profile: &str,
    started: u64,
) -> Run {
    let task = &record.task.id;
    let id = format!("R{}", record.runs.len() + 1);
    let root = match record.task.project.as_deref() {
        Some("site") => "/preview/corral/site",
        _ => "/preview/corral",
    };
    let base = record.task.base.clone().unwrap_or_else(|| "main".into());
    Run {
        task: task.clone(),
        role,
        profile: profile.into(),
        harness: harness(profile).into(),
        model: Some(
            if harness(profile) == "codex" {
                "terra"
            } else {
                "sol"
            }
            .into(),
        ),
        effort: Some(
            if role == Role::Reviewer {
                "xhigh"
            } else {
                "high"
            }
            .into(),
        ),
        worktree: PathBuf::from(format!("{root}/.worktrees/{task}")),
        branch: format!("corral/{task}"),
        base_sha: sha(&format!("{base}@{task}"), 40),
        base,
        session: Some(SessionRef::Herdr {
            workspace: "corral".into(),
            pane: format!("{task}-{id}"),
        }),
        started: ago(feed, started),
        ended: None,
        outcome: None,
        summary: None,
        head_sha: None,
        quiet_since: None,
        id,
    }
}

/// Makes `run` a headless run: a detached process writing `session.log` in its run directory.
pub(super) fn headless(run: &mut Run, pid: u32) {
    run.session = Some(SessionRef::Process {
        pid,
        dir: PathBuf::from(format!(
            "/preview/corral/.corral-next/runs/{}/{}",
            run.task, run.id
        )),
    });
}

pub(super) fn harness(profile: &str) -> &'static str {
    if profile.starts_with("codex") {
        "codex"
    } else {
        "omp"
    }
}

/// End `run` `ended` seconds ago; a worker that submitted has a head commit and summary.
pub(super) fn finish(feed: &Feed, run: &mut Run, ended: u64, outcome: Outcome) {
    run.ended = Some(ago(feed, ended));
    run.outcome = Some(outcome);
    if run.role == Role::Worker && matches!(outcome, Outcome::Done | Outcome::Landed) {
        run.head_sha = Some(sha(&format!("{}/{}", run.task, run.id), 40));
        run.summary = Some(format!("Preview · {} work submitted", run.task));
    }
}

/// A gate check on the task's latest submitted head, recording the round and the worker run
/// it gated.
pub(super) fn gate(
    feed: &Feed,
    record: &TaskDetail,
    pass: bool,
    notes: Option<&str>,
    at: u64,
) -> Check {
    let submitted = record.runs.iter().rev().find(|run| run.head_sha.is_some());
    let head = submitted
        .and_then(|run| run.head_sha.clone())
        .unwrap_or_else(|| sha(&format!("{}/wip", record.task.id), 40));
    Check {
        task: record.task.id.clone(),
        kind: CheckKind::Gate,
        key: head,
        result: if pass {
            CheckResult::Pass
        } else {
            CheckResult::Fail
        },
        notes: notes.map(str::to_owned),
        log: None,
        by: Actor::Auto,
        at: ago(feed, at),
        round: Some(record.task.round),
        run: submitted.map(|run| run.id.clone()),
    }
}

/// A review check for the task's current round, recording the reviewer run that judged it.
pub(super) fn review(
    feed: &Feed,
    record: &TaskDetail,
    approve: bool,
    notes: Option<&str>,
    at: u64,
) -> Check {
    let reviewer = record
        .runs
        .iter()
        .rev()
        .find(|run| run.role == Role::Reviewer);
    Check {
        task: record.task.id.clone(),
        kind: CheckKind::Review,
        key: record.task.round.to_string(),
        result: if approve {
            CheckResult::Pass
        } else {
            CheckResult::Fail
        },
        notes: notes.map(str::to_owned),
        log: None,
        by: Actor::Reviewer,
        at: ago(feed, at),
        round: Some(record.task.round),
        run: reviewer.map(|run| run.id.clone()),
    }
}

pub(super) fn artifact(
    feed: &Feed,
    record: &TaskDetail,
    kind: ArtifactKind,
    title: &str,
    at: u64,
) -> Artifact {
    let id = record.artifacts.len() as u32 + 1;
    Artifact {
        task: record.task.id.clone(),
        id,
        sha256: sha(&format!("{}:{id}", record.task.id), 64),
        bytes: 1_200 + u64::from(id) * 830,
        kind,
        title: title.into(),
        at: ago(feed, at),
        by: if kind == ArtifactKind::Log {
            Actor::Auto
        } else {
            Actor::Agent
        },
        // corrald states the project when it lists the catalog; the run is the latest one, which
        // an agent's artifact came from.
        project: None,
        run: record
            .runs
            .last()
            .filter(|_| kind != ArtifactKind::Log)
            .map(|run| run.id.clone()),
    }
}

/// A stable hex digest of `seed`, `len` characters long; samples need identities, not security.
pub(super) fn sha(seed: &str, len: usize) -> String {
    let mut out = String::with_capacity(len + 16);
    let mut salt = 0u64;
    while out.len() < len {
        let mut hash = 0xcbf2_9ce4_8422_2325u64 ^ salt;
        for byte in seed.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        out.push_str(&format!("{hash:016x}"));
        salt += 1;
    }
    out.truncate(len);
    out
}
