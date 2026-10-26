//! The board work budget, run by the gate through `scripts/work-budget.sh`.
//!
//! Drives every live board path ([`super::work_paths`]) over two synthetic stores that differ
//! only in history (finished tasks with runs and comments, and event-log lines) and asserts on
//! deterministic work counts, never on time: a fixed budget per path, and identical counts at
//! both history sizes, so per-frame work and per-tick content work cannot grow with history.
//!
//! Two counters are budgeted per task file instead, intentionally: `task_stats` (each task
//! listing checks every task file's metadata once) and `board_visits` (applying a board snapshot
//! derives column order and slot usage from every task). T-235's amended deliverable 2 makes this
//! an explicit per-task tick budget; change-driven ticks are T-244 (inotify-driven task
//! observation), T-245 (change-driven sync passes) and T-246 (incremental search refresh), and
//! each should turn its per-task limit here into a fixed one.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use corral_core::{
    Artifact, ArtifactKind, CommentAuthor, Event, HerdrRef, ResourceSnapshot, RunOutcome, Status,
    Store, Task, TaskComment, work::Work,
};
use corral_herdr::Herdr;
use corral_ops::Ctx;
use corral_ops::resources::{FixedResourceProbe, ResourceProbe};
use jiff::Timestamp;

use super::work_paths::{Paths, Row, report};

const START: Timestamp = Timestamp::constant(1_767_268_800, 0);
const SMALL: (usize, usize) = (60, 6_000);
const LARGE: (usize, usize) = (240, 24_000);
/// Artifact snapshot files, recorded on the active tasks so they do not grow with history.
const ARTIFACTS: usize = 2_000;

/// Active work stays fixed while history grows: (status, count).
const ACTIVE: [(Status, usize); 6] = [
    (Status::Idea, 3),
    (Status::Backlog, 8),
    (Status::Ready, 3),
    (Status::Running, 3),
    (Status::NeedsMe, 2),
    (Status::Review, 4),
];

struct Fixture {
    _trap: ScratchTrap,
    _root: tempfile::TempDir,
    ctx: Ctx,
    detail: String,
    review: String,
    /// Raises the machine's memory pressure to severe once set.
    pressure: Arc<AtomicBool>,
    /// Runs still in flight whose worktree exists: all that escalation may size.
    in_flight: u64,
    /// Runs with an existing worktree that are not in flight.
    finished_worktrees: u64,
    scratch_files: u64,
}

/// Reviewer scratch (`state/reviews`, `state/review-watch`) made unreadable for the board's
/// process: any sync pass that touched it would fail loudly, on top of the strace proof.
struct ScratchTrap(Vec<PathBuf>);

impl Drop for ScratchTrap {
    fn drop(&mut self) {
        for dir in &self.0 {
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
        }
    }
}

/// A healthy machine until `severe` is set, then one with no memory available.
#[derive(Debug)]
struct PressureProbe(Arc<AtomicBool>);

impl ResourceProbe for PressureProbe {
    fn sample(&self, _store: &Store) -> anyhow::Result<ResourceSnapshot> {
        let mut snapshot = FixedResourceProbe::healthy().0;
        if self.0.load(Ordering::SeqCst) {
            snapshot.mem_available = 0;
        }
        Ok(snapshot)
    }
}

/// Every run gets a real worktree (with a target directory), and `artifacts` snapshot files
/// are recorded on the task, so a pass that walked or stat'ed them would have work to do.
fn settle(task: &mut Task, store_dir: &Path, worktrees: &Path, artifacts: usize) {
    // The review screen diffs a task under review in its worktree, which would need a git repo.
    let real = task.status != Status::Review;
    for run in task.runs.iter_mut().filter(|_| real) {
        run.worktree = worktrees.join(format!("{}-{}", task.id, run.id));
        std::fs::create_dir_all(run.worktree.join("target")).unwrap();
        std::fs::write(run.worktree.join("target/build.bin"), [0_u8; 1024]).unwrap();
    }
    let n: u128 = task.id.trim_start_matches("T-").parse().unwrap();
    let dir = store_dir.join("artifacts").join(&task.id).join("R1");
    std::fs::create_dir_all(&dir).unwrap();
    task.artifacts = (0..artifacts)
        .map(|k| {
            let path = format!("artifacts/{}/R1/{k:04}-evidence.txt", task.id);
            let body = text("Artifact", k, 6);
            std::fs::write(store_dir.join(&path), &body).unwrap();
            Artifact {
                tier_override: None,
                compressed_bytes: None,
                purged_at: None,
                run: "R1".into(),
                kind: ArtifactKind::Evidence,
                title: format!("Evidence {k}"),
                path,
                source: String::new(),
                sha256: format!("{n:032x}{k:032x}"),
                bytes: body.len() as u64,
                added: START,
            }
        })
        .collect();
}

/// Populated reviewer scratch for every task, then unreadable. Returns the trap and file count.
fn scratch(state: &Path, ids: &[String]) -> (ScratchTrap, u64) {
    let (reviews, watch) = (state.join("reviews"), state.join("review-watch"));
    let mut files = 0;
    for id in ids {
        let child = watch.join(id);
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir_all(&reviews).unwrap();
        std::fs::write(
            reviews.join(format!("{id}-R1-notes.md")),
            text("Notes", 0, 40),
        )
        .unwrap();
        std::fs::write(child.join("R1-review1.json"), r#"{"round":1}"#).unwrap();
        std::fs::write(child.join("R1-review1.screen.log"), text("Screen", 0, 60)).unwrap();
        files += 3;
    }
    for dir in [&reviews, &watch] {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    (ScratchTrap(vec![reviews, watch]), files)
}

/// Whether this process can still read a trapped directory (running as root).
fn trap_ineffective(state: &Path) -> bool {
    std::fs::read_dir(state.join("reviews")).is_ok()
}

fn text(label: &str, n: usize, words: usize) -> String {
    format!("{label} {n}: ") + &"bounded acceptance evidence and review notes ".repeat(words)
}

/// Finished OmO runs keep their session transcripts, which questions are checked against.
fn transcript(agent: &Path, worktree: &Path) {
    let encoded = worktree
        .to_string_lossy()
        .trim_start_matches('/')
        .replace(['/', ':'], "-");
    let dir = agent.join("sessions").join(format!("--{encoded}--"));
    std::fs::create_dir_all(&dir).unwrap();
    let line = serde_json::json!({"type": "message", "message": {"role": "assistant",
        "content": [{"type": "text", "text": text("Transcript", 0, 20)}]}});
    std::fs::write(dir.join("session.jsonl"), format!("{line}\n{line}\n")).unwrap();
}

fn task(seed: &Task, dir: &Path, n: usize, status: Status, pane: Option<HerdrRef>) -> Task {
    let mut task = seed.clone();
    task.id = format!("T-{n:03}");
    task.path = dir.join(format!("T-{n:03}-fixture.md"));
    task.title = format!("Fixture {n:03}");
    task.status = status;
    task.created = START;
    task.updated = START;
    task.body = format!("## Goal\n{}\n", text("Spec", n, 20));
    task.comments = (0..6)
        .map(|i| TaskComment {
            author: if i % 2 == 0 {
                CommentAuthor::Owner
            } else {
                CommentAuthor::Pm
            },
            at: START,
            text: text("Comment", i, 8),
        })
        .collect();
    let open = matches!(status, Status::Running | Status::NeedsMe);
    let finished = matches!(status, Status::Done | Status::Dropped);
    task.runs = (1..=3)
        .map(|r| {
            let agent = if finished { "omo" } else { "claude" };
            let mut run = crate::board::tests::run(&format!("R{r}"), agent);
            run.worktree = dir.join(format!("missing-worktree-{n}"));
            run.summary = Some(text("Summary", r, 20));
            if r < 3 || !open {
                run.ended = Some(START);
                run.outcome = Some(RunOutcome::Done);
                run.agent_state = Some("done".into());
            } else {
                run.herdr = pane.clone();
            }
            if status == Status::NeedsMe && r == 3 {
                run.agent_state = Some("blocked".into());
                run.blocked_excerpt = Some("Which option should I take?".into());
            }
            run
        })
        .collect();
    task
}

/// `history` finished tasks plus [`ACTIVE`], and `events` log lines.
fn fixture((history, events): (usize, usize)) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let mut store = Store::init(root.path()).unwrap();
    store.config.dispatch.paused = true;
    store.config.review.autopilot = Some(false);
    let agent = root.path().join("agent");
    store.config.sync.omo_agent_dir = Some(agent.clone());
    store.save_config().unwrap();
    let store = Store::open(root.path()).unwrap();
    // A checked-in executable cannot inherit a writable fd from a parallel fixture setup.
    let bin = root.path().join("herdr");
    std::os::unix::fs::symlink(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app/work_budget_herdr.sh"),
        &bin,
    )
    .unwrap();
    let herdr = Herdr::with_bin(bin, Some("corral-test".into()));
    let socket = herdr.socket_path().unwrap();
    let seed = store.create("Seed", None, Status::Backlog, "").unwrap();
    std::fs::remove_file(&seed.path).unwrap();
    let dir = store.dir().join("tasks");
    let mut n = 0;
    let mut pane = 0;
    let mut ids = Vec::new();
    let worktrees = root.path().join("worktrees");
    let active: usize = ACTIVE.iter().map(|(_, count)| count).sum();
    let share = ARTIFACTS.div_ceil(active);
    let mut unplaced = ARTIFACTS;
    for (status, count) in ACTIVE {
        for _ in 0..count {
            n += 1;
            let herdr = matches!(status, Status::Running | Status::NeedsMe).then(|| {
                pane += 1;
                HerdrRef {
                    workspace_id: "w1".into(),
                    pane_id: format!("w1:p{pane}"),
                    socket: Some(socket.clone()),
                }
            });
            let mut task = task(&seed, &dir, n, status, herdr);
            let artifacts = share.min(unplaced);
            unplaced -= artifacts;
            settle(&mut task, store.dir(), &worktrees, artifacts);
            std::fs::write(&task.path, corral_core::render_task(&task)).unwrap();
            ids.push((task.id, status));
        }
    }
    let mut history_ids = Vec::new();
    for i in 0..history {
        n += 1;
        let status = if i % 10 == 9 {
            Status::Dropped
        } else {
            Status::Done
        };
        let mut task = task(&seed, &dir, n, status, None);
        settle(&mut task, store.dir(), &worktrees, 0);
        transcript(&agent, &task.runs[2].worktree);
        std::fs::write(&task.path, corral_core::render_task(&task)).unwrap();
        history_ids.push(task.id);
    }
    let mut log = std::fs::read_to_string(store.dir().join("events.jsonl")).unwrap();
    for i in 0..events {
        let kind = ["commented", "moved", "done", "artifact", "approved"][i % 5];
        let event = Event {
            ts: START,
            task: Some(format!("T-{:03}", 1 + i % n)),
            kind: kind.into(),
            actor: None,
            via: None,
            detail: serde_json::json!({"run": "R1", "to": "done", "note": text("Event", i, 4)}),
        };
        log.push_str(&serde_json::to_string(&event).unwrap());
        log.push('\n');
    }
    std::fs::write(store.dir().join("events.jsonl"), log).unwrap();
    // Every fixture file was just written; a live board's history files settled long ago.
    store.set_task_settle_window(Duration::ZERO);
    corral_ops::set_transcript_settle_window(Duration::ZERO);
    let pick = |wanted: Status| ids.iter().find(|(_, s)| *s == wanted).unwrap().0.clone();
    let all: Vec<String> = ids
        .iter()
        .map(|(id, _)| id.clone())
        .chain(history_ids)
        .collect();
    let (trap, scratch_files) = scratch(&store.state_dir(), &all);
    let runs: Vec<_> = store
        .list()
        .unwrap()
        .into_iter()
        .flat_map(|task| task.runs)
        .filter(|run| run.worktree.is_dir())
        .collect();
    let in_flight = |run: &&corral_core::Run| run.ended.is_none() && run.outcome.is_none();
    let in_flight_runs = runs.iter().filter(in_flight).count() as u64;
    let pressure = Arc::new(AtomicBool::new(false));
    Fixture {
        detail: pick(Status::Running),
        review: pick(Status::Review),
        ctx: Ctx {
            store,
            herdr,
            resource_probe: Arc::new(PressureProbe(pressure.clone())),
        },
        _trap: trap,
        _root: root,
        pressure,
        in_flight: in_flight_runs,
        finished_worktrees: runs.len() as u64 - in_flight_runs,
        scratch_files,
    }
}

fn counters(row: &Row) -> Vec<(&'static str, u64)> {
    let mut out: Vec<_> = Work::ALL
        .iter()
        .map(|w| (w.name(), row.work.get(*w)))
        .collect();
    out.push(("herdr_calls", row.work.herdr));
    out.push(("git_calls", row.work.git));
    out
}

/// The measured rows, how many task files the store holds and how many runs were in flight.
fn measure(size: (usize, usize)) -> (Vec<Row>, u64, u64) {
    let fx = fixture(size);
    let tasks = fx.ctx.store.list().unwrap().len() as u64;
    let state = fx.ctx.store.state_dir();
    assert!(
        !trap_ineffective(&state),
        "reviewer scratch is readable here (running as root?); the trap proves nothing"
    );
    let mut paths = Paths::new(fx.ctx).with_pressure(fx.pressure.clone());
    paths.run(&fx.detail, &fx.review);
    let errors = paths.errors();
    assert!(
        errors.is_empty(),
        "the fixture board reported:\n  {}",
        errors.join("\n  ")
    );
    println!(
        "history {} tasks / {} events ({tasks} task files, {ARTIFACTS} artifact files, {} \
unreadable scratch files, {} in-flight and {} finished worktrees):\n{}",
        size.0,
        size.1,
        fx.scratch_files,
        fx.in_flight,
        fx.finished_worktrees,
        report(&paths.rows)
    );
    (paths.rows, tasks, fx.in_flight)
}

#[test]
#[ignore = "run by scripts/work-budget.sh (the gate)"]
fn board_work_stays_within_budget_and_independent_of_history() {
    // Counters are per thread and caches per path, so the two stores can be measured at once.
    let small = std::thread::spawn(|| measure(SMALL));
    let (large, tasks, in_flight) = measure(LARGE);
    let (small, _, small_in_flight) = small.join().unwrap();
    let mut failures = violations(&small, &large, tasks);
    failures.extend(pass_failures(&small, small_in_flight));
    failures.extend(pass_failures(&large, in_flight));
    assert!(
        failures.is_empty(),
        "board work budget exceeded:\n  {}",
        failures.join("\n  ")
    );
}

/// Every budget and history-independence failure between the two measured stores.
fn violations(small: &[Row], large: &[Row], tasks: u64) -> Vec<String> {
    let mut failures = Vec::new();
    for (s, l) in small.iter().zip(large) {
        assert_eq!(s.path, l.path);
        for ((name, small), (_, large)) in counters(s).into_iter().zip(counters(l)) {
            let limit = budget(&l.path, name);
            let allowed = match limit {
                Some(Limit::Fixed(n)) => Some(n),
                Some(Limit::PerTask(n)) => Some(n * tasks),
                None => None,
            };
            if let Some(allowed) = allowed
                && large > allowed
            {
                let unit = match limit {
                    Some(Limit::PerTask(n)) => format!(" ({n} per task file x {tasks})"),
                    _ => String::new(),
                };
                failures.push(format!(
                    "{}: {name} = {large}, budget {allowed}{unit}",
                    l.path
                ));
            }
            let per_task = matches!(limit, Some(Limit::PerTask(_)));
            if small != large && !name.ends_with("_bytes") && !per_task {
                failures.push(format!(
                    "{}: {name} grows with history: {small} at {} tasks/{} events, {large} at {} tasks/{} events",
                    l.path, SMALL.0, SMALL.1, LARGE.0, LARGE.1
                ));
            }
        }
    }
    failures
}

/// A path's allowance for one counter.
#[derive(Clone, Copy)]
enum Limit {
    /// At most this much, and the same at both history sizes.
    Fixed(u64),
    /// At most this much per task file in the store, exempt from the history check: work that
    /// intentionally visits every task once per listing or board rebuild (see the module docs).
    PerTask(u64),
}

/// The most work each path may do; `None` leaves a counter to the history-independence check.
///
/// Frames, keys, mouse input and redraws do no store, herdr or git work and visit no task at
/// all; this is where the T-217 per-frame config re-read would show. Other fixed limits sit a
/// little above the fixture's current counts (in parentheses) and do not depend on its size, so
/// moderate growth is a deliberate budget change while O(history) reads fail loudly. Only the
/// metadata checks of task listings and the board's derived-state rebuild are per task, by
/// design until T-244/T-245; a per-task limit still fails if the rate per task grows.
fn budget(path: &str, counter: &str) -> Option<Limit> {
    let input = path.ends_with("_frame")
        || path.ends_with("_redraw")
        || path.ends_with("_key_down_handle")
        || path.ends_with("_wheel_handle")
        || path.ends_with("_select_handle");
    if input {
        return Some(Limit::Fixed(0));
    }
    let apply_board = path.starts_with("tick_apply_board") || path == "tick_apply_settings";
    if apply_board {
        // Applying the worker's snapshot rebuilds column order and slot usage: two passes.
        return Some(match counter {
            "board_visits" => Limit::PerTask(2),
            _ => Limit::Fixed(0),
        });
    }
    use Limit::{Fixed, PerTask};
    let limits: &[(&str, Limit)] = if path.starts_with("sync_tick") {
        // One background tick: sync, usage refresh and the board snapshot.
        &[
            ("config_reads", Fixed(60)), // (50) strict policy re-reads per action check
            ("config_parses", Fixed(100)), // (87)
            ("task_lists", Fixed(44)),   // (38) 24 listings + 14 read-only snapshots
            // (46) one check per file per listing or lookup; the tick that raises the resource
            // alert lists tasks twice more (escalation reads the board, then updates one task).
            (
                "task_stats",
                PerTask(if path.ends_with("_severe") { 50 } else { 48 }),
            ),
            ("task_reads", Fixed(2)), // (1) the task edited since the previous tick
            ("task_parses", Fixed(2)), // (1)
            ("event_reads", Fixed(16)), // (12)
            ("event_parses", Fixed(8)), // (2) lines appended since the previous tick
            ("transcript_reads", Fixed(0)),
            ("board_visits", Fixed(0)), // the worker builds no board index
            ("herdr_calls", Fixed(6)),  // (4)
            ("git_calls", Fixed(4)),    // (0) fixture runs have no worktrees
        ]
    } else if path.starts_with("tick_apply") {
        // The visible detail or review screen re-reading its task after a tick.
        &[
            ("config_reads", Fixed(2)),
            ("config_parses", Fixed(4)),
            ("task_lists", Fixed(2)),
            ("task_stats", PerTask(3)), // (2)
            ("task_reads", Fixed(2)),
            ("task_parses", Fixed(2)),
            ("event_reads", Fixed(2)),
            ("event_parses", Fixed(0)),
            ("transcript_reads", Fixed(0)),
            ("board_visits", PerTask(2)), // the board below re-applies the snapshot
            ("herdr_calls", Fixed(2)),
            ("git_calls", Fixed(4)),
        ]
    } else {
        // Opening a screen, or Back, which reloads the board synchronously.
        &[
            ("config_reads", Fixed(16)),  // (12)
            ("config_parses", Fixed(24)), // (20)
            ("task_lists", Fixed(10)),    // (8) including read-only policy snapshots
            ("task_stats", PerTask(10)),  // (8)
            ("task_reads", Fixed(4)),     // (3) the task being opened
            ("task_parses", Fixed(4)),    // (3)
            ("event_reads", Fixed(6)),    // (4)
            ("event_parses", Fixed(0)),
            ("transcript_reads", Fixed(0)),
            ("board_visits", PerTask(2)), // (2) the reloaded board's index
            ("herdr_calls", Fixed(2)),    // (2) a running task's live pane
            ("git_calls", Fixed(4)),
        ]
    };
    limits
        .iter()
        .find(|(name, _)| *name == counter)
        .map(|(_, limit)| *limit)
}

/// T-343: the fingerprint and escalation passes must have run in every measured sync tick
/// before their zeros mean anything. Only then: no artifact file is stat'ed when nothing
/// changed, and a worktree is sized only for a run still in flight, on the tick that raises the
/// severe sample. Reviewer scratch is unreadable in the fixture, so any read fails the tick.
fn pass_failures(rows: &[Row], in_flight: u64) -> Vec<String> {
    let mut failures = Vec::new();
    for row in rows.iter().filter(|r| r.path.starts_with("sync_tick")) {
        let get = |work| row.work.get(work);
        let active = u64::try_from(ACTIVE.iter().map(|(_, count)| count).sum::<usize>()).unwrap();
        if get(Work::AttentionVisit) != active {
            failures.push(format!(
                "{}: attention_visits = {}, expected {active} (unfinished tasks)",
                row.path,
                get(Work::AttentionVisit)
            ));
        }
        let ran = [Work::FingerprintPass, Work::ResourcePass].map(|work| {
            let ran = get(work) >= 1;
            if !ran {
                failures.push(format!(
                    "{}: {} = 0, the pass did not run",
                    row.path,
                    work.name()
                ));
            }
            ran
        });
        if ran.contains(&false) {
            continue;
        }
        if row.path == "sync_tick_board" && get(Work::ArtifactStat) != 0 {
            failures.push(format!(
                "{}: artifact_stats = {} with nothing changed",
                row.path,
                get(Work::ArtifactStat)
            ));
        }
        let walks = if row.path == "sync_tick_board_severe" {
            in_flight
        } else {
            0
        };
        if get(Work::WorktreeWalk) != walks {
            failures.push(format!(
                "{}: worktree_walks = {}, expected {walks} (runs in flight)",
                row.path,
                get(Work::WorktreeWalk)
            ));
        }
    }
    if !rows.iter().any(|r| r.path == "sync_tick_board_severe") {
        failures.push("no sync tick was measured under the severe resource sample".into());
    }
    failures
}

fn pass_row(path: &str, passes: [(Work, u64); 5]) -> Row {
    let mut work = crate::app::work_probe::Sample::default();
    for (counter, n) in passes {
        work.work[counter as usize] = n;
    }
    Row {
        path: path.into(),
        work,
        elapsed: Duration::ZERO,
    }
}

fn passes(artifact_stats: u64, walks: u64) -> [(Work, u64); 5] {
    [
        (Work::FingerprintPass, 1),
        (Work::ResourcePass, 1),
        (Work::ArtifactStat, artifact_stats),
        (Work::WorktreeWalk, walks),
        (
            Work::AttentionVisit,
            u64::try_from(ACTIVE.iter().map(|(_, count)| count).sum::<usize>()).unwrap(),
        ),
    ]
}

#[test]
fn missing_attention_classification_cannot_pass_the_budget() {
    let mut absent = passes(0, 0);
    absent[4].1 = 0;
    let rows = [
        pass_row("sync_tick_board", absent),
        pass_row("sync_tick_board_severe", passes(0, 5)),
    ];
    let failures = pass_failures(&rows, 5);
    assert_eq!(failures.len(), 1);
    assert!(failures[0].contains("attention_visits = 0"));
}

#[test]
fn passes_that_did_not_run_fail_before_their_zeros_count() {
    let mut idle = passes(0, 0);
    idle[0].1 = 0;
    let rows = [
        pass_row("sync_tick_board", idle),
        pass_row("sync_tick_board_severe", passes(0, 5)),
    ];
    assert_eq!(
        pass_failures(&rows, 5),
        ["sync_tick_board: fingerprint_passes = 0, the pass did not run"]
    );
}

#[test]
fn stats_and_walks_beyond_the_allowance_fail() {
    let rows = [
        pass_row("sync_tick_board", passes(2_000, 0)),
        pass_row("sync_tick_board_after_event", passes(2_000, 3)),
        pass_row("sync_tick_board_severe", passes(0, 7)),
    ];
    assert_eq!(
        pass_failures(&rows, 5),
        [
            "sync_tick_board: artifact_stats = 2000 with nothing changed",
            "sync_tick_board_after_event: worktree_walks = 3, expected 0 (runs in flight)",
            "sync_tick_board_severe: worktree_walks = 7, expected 5 (runs in flight)",
        ]
    );
}

#[test]
fn rebuild_ticks_may_stat_artifacts_and_severe_ticks_walk_in_flight_runs() {
    let rows = [
        pass_row("sync_tick_board", passes(0, 0)),
        pass_row("sync_tick_board_after_event", passes(2_000, 0)),
        pass_row("sync_tick_board_severe", passes(0, 5)),
    ];
    assert!(pass_failures(&rows, 5).is_empty());
    assert_eq!(
        pass_failures(&rows[..2], 5),
        ["no sync tick was measured under the severe resource sample"]
    );
}

fn transcript_row(path: &str, reads: u64) -> Row {
    let mut work = crate::app::work_probe::Sample::default();
    work.work[Work::TranscriptRead as usize] = reads;
    Row {
        path: path.into(),
        work,
        elapsed: Duration::ZERO,
    }
}

#[test]
fn transcript_reads_must_not_grow_with_history() {
    let small = [transcript_row("board_open", 54)];
    let large = [transcript_row("board_open", 0)];
    let failures = violations(&small, &large, 263);
    assert_eq!(
        failures,
        [
            "board_open: transcript_reads grows with history: 54 at 60 tasks/6000 events, 0 at 240 tasks/24000 events"
        ]
    );
    let failures = violations(&large, &small, 263);
    assert!(
        failures
            .iter()
            .any(|f| f.contains("transcript_reads = 54, budget 0")),
        "{failures:?}"
    );
}

#[test]
fn equal_zero_transcript_reads_pass() {
    let rows = [transcript_row("board_open", 0)];
    assert!(violations(&rows, &rows, 263).is_empty());
}
