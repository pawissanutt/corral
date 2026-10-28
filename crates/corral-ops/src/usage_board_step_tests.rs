//! The board step's usage refresh against `db316644`'s (plan T-253 section 4, "Usage
//! refresh"): with the real collector and real transcript files, every tick stores exactly what
//! `db316644`'s `refresh_active_usage` (the board listing, the task's events and `refresh_runs`
//! over each active current run) stores on a copy of the same store and transcripts. Board,
//! other-task, earlier-run and event growth leave every counter identical; growth inside the
//! refreshed runs raises only the own-record and transcript cells, by exactly the rows, lines
//! and bytes the algorithm reads.
use std::path::{Path, PathBuf};

use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use corral_core::{Price, Run, Status, Store};
use serde_json::json;

use super::*;

const T: &str = "2026-10-01T00:00:00Z";
const ACTIVE: [&str; 4] = ["worker", "review", "legacy", "verdict"];

fn at(seconds: i64) -> Timestamp {
    T.parse::<Timestamp>()
        .unwrap()
        .checked_add(jiff::SignedDuration::from_secs(seconds))
        .unwrap()
}

/// One response; `input` 0 makes it a zero-token entry, so grown transcripts keep their totals.
fn line(id: &str, seconds: i64, input: u64) -> String {
    let output = if input == 0 { 0 } else { 5 };
    json!({"type": "message", "id": id, "timestamp": at(seconds),
           "message": {"role": "assistant", "model": "m",
                       "usage": {"input": input, "output": output, "cacheRead": 0, "cacheWrite": 0}}})
    .to_string()
        + "\n"
}

/// A session file under the agent directory for `cwd`, with `n` responses.
fn transcript(omo: &Path, cwd: &str, n: usize) -> PathBuf {
    let dir = omo
        .join("sessions")
        .join(crate::omo::encode_cwd(Path::new(cwd)));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session.jsonl");
    let mut text = json!({"type": "session", "cwd": cwd, "timestamp": at(0)}).to_string() + "\n";
    for i in 0..n {
        let input = if i < 2 { 10 + i as u64 } else { 0 };
        text.push_str(&line(&format!("{cwd}-{i}"), 10 + i as i64, input));
    }
    fs::write(&path, text).unwrap();
    path
}

fn run(id: &str, worktree: &str, started: i64, ended: Option<i64>) -> serde_json::Value {
    json!({"id": id, "agent": "omo", "harness": "omo", "worktree": worktree,
           "branch": "main", "base": "main", "base_sha": "fixture", "started": at(started),
           "ended": ended.map(at), "outcome": ended.map(|_| "done")})
}

/// The same board in a fresh store: the four active tasks of the scenario, `earlier` ended runs
/// before each current run, `terminal` Done tasks and `events` unrelated events.
fn board(root: &Path, omo: &Path, earlier: usize, terminal: usize, events: usize) -> Store {
    let mut store = Store::init(root).unwrap();
    store.config.sync.omo_agent_dir = Some(omo.to_path_buf());
    store.save_config().unwrap();
    let mut legacy = None;
    for name in ACTIVE {
        let status = match name {
            "worker" => Status::Running,
            "legacy" => Status::NeedsMe,
            _ => Status::Review,
        };
        let mut task = store.create(name, None, status, "").unwrap();
        task.created = at(0);
        for e in 0..earlier {
            let old = run(
                &format!("R{e:03}"),
                &format!("/fixture/{name}-old-{e}"),
                0,
                Some(1),
            );
            task.runs.push(serde_json::from_value(old).unwrap());
        }
        let current = format!("R{earlier:03}");
        let mut value = match name {
            // An unended worker whose transcript grows.
            "worker" => run(&current, "/fixture/worker", 0, None),
            _ => run(&current, &format!("/fixture/{name}"), 0, Some(5000)),
        };
        if name == "review" {
            let child = |n: u32, agent: &str, ended: Option<i64>| {
                let mut child = run(
                    &format!("{current}-review{n}"),
                    &format!("/fixture/rev{n}"),
                    1,
                    ended,
                );
                child["agent"] = json!(agent);
                if agent != "omo" {
                    child["harness"] = serde_json::Value::Null;
                }
                child
            };
            // A completed reviewer without entries, one with an unsupported-harness note, and
            // an unended reviewer whose transcript grows.
            value["review_runs"] = json!([
                child(1, "omo", Some(4000)),
                child(2, "unsupported", Some(4000)),
                child(3, "omo", None),
            ]);
        }
        if name == "verdict" {
            let mut child = run(
                &format!("{current}-review1"),
                "/fixture/verdict-rev",
                1,
                None,
            );
            child["agent"] = json!("omo");
            value["review_runs"] = json!([child]);
        }
        if name == "legacy" {
            // Stored entries without timestamps, from the transcript, to be backfilled.
            let mut parsed = None;
            let path = omo
                .join("sessions")
                .join(crate::omo::encode_cwd(Path::new("/fixture/legacy")))
                .join("session.jsonl");
            let probe: Run = serde_json::from_value(value.clone()).unwrap();
            if path.exists() {
                parsed = parse_session_priced(
                    BufReader::new(File::open(&path).unwrap()),
                    &probe,
                    false,
                    &store.config,
                )
                .unwrap();
            }
            if let Some(mut usage) = parsed {
                for entry in &mut usage.entries {
                    entry.at = None;
                    entry.latency_ms = None;
                }
                value["usage"] = serde_json::to_value(&usage).unwrap();
            }
            legacy = Some(task.id.clone());
        }
        task.runs.push(serde_json::from_value(value).unwrap());
        store.fixture_put_task(&task).unwrap();
    }
    for t in 0..terminal {
        let mut task = store
            .create(&format!("Done {t}"), None, Status::Done, "")
            .unwrap();
        let old = run("R000", &format!("/fixture/done-{t}"), 0, Some(1));
        task.runs.push(serde_json::from_value(old).unwrap());
        store.fixture_put_task(&task).unwrap();
    }
    let mut log = String::new();
    for e in 0..events {
        log.push_str(
            &json!({"seq": 0, "ts": at(1), "task": legacy, "kind": "commented",
                    "detail": {"run": "R999", "n": e}})
            .to_string(),
        );
        log.push('\n');
    }
    // The owner action that ends the unended verdict reviewer.
    let verdict = format!("T-{:03}", 4);
    log.push_str(
        &json!({"seq": 0, "ts": at(60), "task": verdict, "kind": "approved",
                "detail": {"run": format!("R{earlier:03}")}})
        .to_string(),
    );
    log.push('\n');
    store.fixture_append_events(&log).unwrap();
    store
}

/// `db316644`'s `refresh_active_usage`: the listing, each task's events and `refresh_runs`.
fn reference_tick(store: &Store) {
    for task in store.list().unwrap() {
        if !matches!(
            task.status,
            Status::Running | Status::NeedsMe | Status::Review
        ) {
            continue;
        }
        let Some(run) = task.current_run() else {
            continue;
        };
        let events = usage_events(store, &task.id).unwrap();
        refresh_runs(store, &task.id, std::slice::from_ref(run), None, &events).unwrap();
    }
}

fn current_runs(store: &Store) -> Vec<Run> {
    ACTIVE
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let id = format!("T-{:03}", i + 1);
            store.get(&id).unwrap().current_run().unwrap().clone()
        })
        .collect()
}

struct World {
    _tmp: tempfile::TempDir,
    omo: PathBuf,
    ours: Store,
    reference: Store,
}

fn world(n: usize, earlier: usize, terminal: usize, events: usize) -> World {
    let tmp = tempfile::tempdir().unwrap();
    let omo = tmp.path().join("omo");
    for cwd in [
        "/fixture/worker",
        "/fixture/rev1",
        "/fixture/rev3",
        "/fixture/legacy",
        "/fixture/verdict-rev",
    ] {
        transcript(&omo, cwd, n);
    }
    let ours = board(&tmp.path().join("ours"), &omo, earlier, terminal, events);
    let reference = board(
        &tmp.path().join("reference"),
        &omo,
        earlier,
        terminal,
        events,
    );
    World {
        _tmp: tmp,
        omo,
        ours,
        reference,
    }
}

fn tick(w: &World) {
    refresh_active_usage(&w.ours);
    transcript_cache::refresh(|| reference_tick(&w.reference)).unwrap();
    assert_eq!(current_runs(&w.ours), current_runs(&w.reference));
}

fn append_response(w: &World, cwd: &str, id: &str) {
    let path = w
        .omo
        .join("sessions")
        .join(crate::omo::encode_cwd(Path::new(cwd)))
        .join("session.jsonl");
    let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
    std::io::Write::write_all(&mut file, line(id, 3000, 7).as_bytes()).unwrap();
}

fn reprice(w: &mut World) {
    for store in [&mut w.ours, &mut w.reference] {
        store.config.pricing.insert(
            "m".into(),
            Price {
                input: 3.0,
                output: 9.0,
                cache_read: None,
                cache_write: None,
                aliases: vec![],
            },
        );
    }
}

#[test]
fn every_tick_stores_what_the_reference_refresh_stores() {
    let _serial = super::CACHE_TESTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for n in [2, 20] {
        let mut w = world(n, 2, 2, 10);
        tick(&w);
        // One new response for the unended worker and the unended reviewer.
        append_response(&w, "/fixture/worker", "worker-new");
        append_response(&w, "/fixture/rev3", "rev3-new");
        tick(&w);
        // A price-table change reprices every owner of the refreshed runs.
        reprice(&mut w);
        tick(&w);
        tick(&w);
        let runs = current_runs(&w.ours);
        assert_eq!(
            runs[3].review_runs[0].ended,
            Some(at(60)),
            "the verdict ends it"
        );
        assert!(
            runs[0]
                .usage
                .as_ref()
                .is_some_and(|u| u.entries.len() == n + 1)
        );
        assert!(
            runs[2].usage.as_ref().is_some_and(|u| u
                .entries
                .iter()
                .filter(|e| e.at.is_some())
                .count()
                == 2)
        );
    }
}

/// One settled tick of the refresh, warm or with an empty transcript cache, as cells.
fn measure(w: &World, cold: bool) -> WorkSnapshot {
    refresh_active_usage(&w.ours);
    if cold {
        transcript_cache::clear();
    }
    let _hot = HotScope::enter();
    let before = work::snapshot();
    refresh_active_usage(&w.ours);
    work::snapshot().since(&before)
}

/// What the algorithm reads on a settled tick: the refreshed runs' rows, and every line of the
/// transcripts of the owners it re-collects, whose bytes it reads only when the cache is cold.
fn expected(w: &World, cold: bool) -> (u64, u64, u64, u64) {
    let (mut rows, mut bytes) = (0, 0);
    for head in w.ours.heads(&HeadQuery::Open { project: None }).unwrap() {
        let run = head.current_run.as_ref().unwrap();
        let _hot = HotScope::enter();
        let before = work::snapshot();
        {
            let _own = work::own_records("expected", &head.id, &run.id);
            head.current_run_record().unwrap();
        }
        let delta = work::snapshot().since(&before);
        rows += delta.own_record_rows();
        bytes += delta.own_record_bytes();
    }
    // refresh_run re-collects an owner that is unended, has no entries, carries an
    // unsupported-harness note or holds untimed entries; only omo owners read a transcript.
    let (mut lines, mut read) = (0, 0);
    for run in current_runs(&w.ours) {
        for owner in std::iter::once(&run).chain(&run.review_runs) {
            let entries = owner
                .usage
                .as_ref()
                .map(|u| u.entries.as_slice())
                .unwrap_or(&[]);
            let unsupported = owner
                .usage_note
                .as_deref()
                .is_some_and(|n| n.starts_with("unsupported harness"));
            let collects = owner.ended.is_none()
                || entries.is_empty()
                || unsupported
                || entries.iter().any(|e| e.at.is_none());
            if !collects || owner.harness.as_deref() != Some("omo") {
                continue;
            }
            let path = w
                .omo
                .join("sessions")
                .join(crate::omo::encode_cwd(&owner.worktree))
                .join("session.jsonl");
            let Ok(text) = fs::read_to_string(path) else {
                continue;
            };
            lines += text.lines().count() as u64;
            if cold {
                read += text.len() as u64;
            }
        }
    }
    (rows, bytes, lines, read)
}

fn attributed(delta: &WorkSnapshot) -> (u64, u64, u64, u64) {
    let cell = |c: Counter| delta.cells(c).own_record;
    (
        delta.own_record_rows(),
        delta.own_record_bytes(),
        cell(Counter::TranscriptLinesAggregated),
        cell(Counter::TranscriptBytesRead),
    )
}

fn unattributed(delta: &WorkSnapshot) -> Vec<(&'static str, u64)> {
    Counter::ALL
        .iter()
        .map(|c| (c.name(), delta.cells(*c).unattributed))
        .collect()
}

#[test]
fn growth_outside_the_refreshed_runs_changes_nothing_and_inside_them_only_own_records() {
    let _serial = super::CACHE_TESTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for cold in [false, true] {
        let base = world(2, 1, 1, 5);
        let board = world(2, 10, 10, 50);
        let grown = world(20, 1, 1, 5);
        let (b, o, g) = (
            measure(&base, cold),
            measure(&board, cold),
            measure(&grown, cold),
        );
        for (delta, w, name) in [
            (&b, &base, "base"),
            (&o, &board, "board"),
            (&g, &grown, "grown"),
        ] {
            assert!(delta.check_identity().is_ok(), "{name}: {delta:?}");
            assert_eq!(delta.unattributed_hydrations(), 0, "{name}");
            assert_eq!(attributed(delta), expected(w, cold), "{name} cold={cold}");
        }
        // Terminal tasks, earlier runs and events: every cell identical.
        assert_eq!(unattributed(&o), unattributed(&b), "cold={cold}");
        assert_eq!(attributed(&o), attributed(&b), "cold={cold}");
        // Entries and transcript lines inside the refreshed runs: only the attributed cells.
        assert_eq!(unattributed(&g), unattributed(&b), "cold={cold}");
        assert!(attributed(&g).0 > attributed(&b).0 && attributed(&g).2 > attributed(&b).2);
    }
}
