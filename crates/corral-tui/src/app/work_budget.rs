//! The board work budget, run by the gate through `scripts/work-budget.sh`.
//!
//! Drives every live board path ([`super::work_paths`]) over two synthetic stores that differ
//! only in history (finished tasks with runs and comments, and event-log lines) and asserts on
//! deterministic work counts, never on time: a fixed budget per path, and identical counts at
//! both history sizes, so per-frame and per-tick work cannot grow with history.

use std::path::Path;
use std::time::Duration;

use corral_core::{
    CommentAuthor, Event, HerdrRef, RunOutcome, Status, Store, Task, TaskComment, work::Work,
};
use corral_herdr::Herdr;
use corral_ops::Ctx;
use jiff::Timestamp;

use super::work_paths::{Paths, Row, report};

const START: Timestamp = Timestamp::constant(1_767_268_800, 0);
const SMALL: (usize, usize) = (60, 6_000);
const LARGE: (usize, usize) = (240, 24_000);

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
    _root: tempfile::TempDir,
    ctx: Ctx,
    detail: String,
    review: String,
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
            let task = task(&seed, &dir, n, status, herdr);
            std::fs::write(&task.path, corral_core::render_task(&task)).unwrap();
            ids.push((task.id, status));
        }
    }
    for i in 0..history {
        n += 1;
        let status = if i % 10 == 9 {
            Status::Dropped
        } else {
            Status::Done
        };
        let task = task(&seed, &dir, n, status, None);
        transcript(&agent, &task.runs[2].worktree);
        std::fs::write(&task.path, corral_core::render_task(&task)).unwrap();
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
    Fixture {
        detail: pick(Status::Running),
        review: pick(Status::Review),
        ctx: Ctx {
            store,
            herdr,
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
        },
        _root: root,
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

/// The measured rows and how many task files the store holds.
fn measure(size: (usize, usize)) -> (Vec<Row>, u64) {
    let fx = fixture(size);
    let tasks = fx.ctx.store.list().unwrap().len() as u64;
    let mut paths = Paths::new(fx.ctx);
    paths.run(&fx.detail, &fx.review);
    let errors = paths.errors();
    assert!(
        errors.is_empty(),
        "the fixture board reported:\n  {}",
        errors.join("\n  ")
    );
    println!(
        "history {} tasks / {} events ({tasks} task files):\n{}",
        size.0,
        size.1,
        report(&paths.rows)
    );
    (paths.rows, tasks)
}

#[test]
#[ignore = "run by scripts/work-budget.sh (the gate)"]
fn board_work_stays_within_budget_and_independent_of_history() {
    // Counters are per thread and caches per path, so the two stores can be measured at once.
    let small = std::thread::spawn(|| measure(SMALL));
    let (large, tasks) = measure(LARGE);
    let (small, _) = small.join().unwrap();
    let mut failures = Vec::new();
    for (s, l) in small.iter().zip(&large) {
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
    assert!(
        failures.is_empty(),
        "board work budget exceeded:\n  {}",
        failures.join("\n  ")
    );
}

/// A path's allowance for one counter.
#[derive(Clone, Copy)]
enum Limit {
    /// At most this much, and the same at both history sizes.
    Fixed(u64),
    /// At most this much per task file in the store: work that still visits every task once per
    /// listing or rebuild (see the T-235 report's remaining limits).
    PerTask(u64),
}

/// The most work each path may do; `None` leaves a counter to the history-independence check.
///
/// Frames, keys, mouse input and redraws do no store, herdr or git work and visit no task at
/// all; this is where the T-217 per-frame config re-read would show. Other fixed limits sit a
/// little above the fixture's current counts (in parentheses) and do not depend on its size, so
/// moderate growth is a deliberate budget change while O(history) reads fail loudly. Only the
/// metadata checks of task listings and the board's derived-state rebuild are per task.
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
            ("task_stats", PerTask(48)), // (42) one check per file per listing or lookup
            ("task_reads", Fixed(2)),    // (1) the task edited since the previous tick
            ("task_parses", Fixed(2)),   // (1)
            ("event_reads", Fixed(16)),  // (12)
            ("event_parses", Fixed(8)),  // (2) lines appended since the previous tick
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
