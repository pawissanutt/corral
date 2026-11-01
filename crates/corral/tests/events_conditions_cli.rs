//! T-328 slim-events through the CLI: `log --json` repeats, `export events --with-conditions`
//! windows, seq-ordered timelines and wait cursors, and slim verdict notes in every report.

use std::process::{Command, Output, Stdio};

use corral_core::{Status, Store};
use serde_json::{Value, json};

struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        Self { dir, store }
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_corral"));
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy();
            if name.starts_with("CORRAL_") || name.starts_with("HERDR_") {
                cmd.env_remove(key);
            }
        }
        cmd.env("CORRAL_ROOT", self.dir.path())
            .env("HERDR_BIN_PATH", self.dir.path().join("no-herdr"))
            .current_dir(self.dir.path())
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn lines(&self, args: &[&str]) -> Vec<Value> {
        self.ok(args)
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn observe(&self, task: &str, detail: Value) {
        self.store.log_event(Some(task), "queued", detail).unwrap();
    }

    fn clear(&self, task: &str) {
        self.store.clear_condition(Some(task), "queued").unwrap();
    }
}

fn a() -> Value {
    json!({"reason": "A"})
}

fn b() -> Value {
    json!({"reason": "B"})
}

/// Observe A 100 times, B once, clear, A 3 times, A once, B twice, clear, clear.
fn four_runs(fx: &Fixture, task: &str) {
    for _ in 0..100 {
        fx.observe(task, a());
    }
    fx.observe(task, b());
    fx.clear(task);
    for _ in 0..4 {
        fx.observe(task, a());
    }
    fx.observe(task, b());
    fx.observe(task, b());
    fx.clear(task);
    fx.clear(task);
}

fn run_lines(lines: &[Value]) -> Vec<&Value> {
    lines
        .iter()
        .filter_map(|line| line.get("condition_run"))
        .collect()
}

#[test]
fn log_json_shows_each_run_as_repeats_on_its_opening_entry() {
    // Given four condition runs on one task.
    let fx = Fixture::new();
    let task = fx.store.create("Queued", None, Status::Ready, "").unwrap();
    four_runs(&fx, &task.id);
    // When the full timeline is read as JSON.
    let entries = fx.lines(&["log", &task.id, "--all", "--json"]);
    let condition: Vec<_> = entries
        .iter()
        .filter(|entry| entry["kind"].as_str().unwrap().starts_with("queued"))
        .collect();
    // Then six entries appear, the four openers carrying their run's repeats.
    let kinds: Vec<_> = condition
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "queued",
            "queued",
            "queued_cleared",
            "queued",
            "queued",
            "queued_cleared"
        ]
    );
    let repeats: Vec<_> = condition
        .iter()
        .map(|e| {
            e.get("repeats")
                .map(|r| (r["count"].clone(), r["close_reason"].clone()))
        })
        .collect();
    assert_eq!(
        repeats,
        [
            Some((json!(100), json!("changed"))),
            Some((json!(1), json!("cleared"))),
            None,
            Some((json!(4), json!("changed"))),
            Some((json!(2), json!("cleared"))),
            None,
        ]
    );
    assert_eq!(
        condition[0]["repeats"]["closed_seq"],
        condition[1]["event"]["seq"]
    );
    assert_eq!(
        condition[4]["repeats"]["closed_seq"],
        condition[5]["event"]["seq"]
    );
}

#[test]
fn export_with_conditions_emits_every_run_that_could_have_changed_since_the_cursor() {
    // Given four closed runs on one task.
    let fx = Fixture::new();
    let task = fx.store.create("Queued", None, Status::Ready, "").unwrap();
    four_runs(&fx, &task.id);
    // When everything is exported with conditions.
    let all = fx.lines(&[
        "export",
        "events",
        "--since",
        "0",
        "--with-conditions",
        "--json",
    ]);
    // Then the four run lines follow the events in opened order.
    let runs = run_lines(&all);
    let counts: Vec<_> = runs.iter().map(|run| run["count"].clone()).collect();
    assert_eq!(counts, [json!(100), json!(1), json!(4), json!(2)]);
    assert!(
        all[..all.len() - 4]
            .iter()
            .all(|line| line.get("seq").is_some())
    );
    let third = runs[2]["opened_seq"].as_u64().unwrap();

    // When exporting after the third run's opening event.
    let since = third.to_string();
    let args = [
        "export",
        "events",
        "--since",
        &since,
        "--with-conditions",
        "--json",
    ];
    let later = fx.lines(&args);
    // Then only later events appear, and exactly runs 3 and 4 (closed after the cursor).
    let seqs: Vec<_> = later
        .iter()
        .filter_map(|line| line["seq"].as_u64())
        .collect();
    assert!(
        !seqs.is_empty() && seqs.iter().all(|&seq| seq > third),
        "{seqs:?}"
    );
    let opened: Vec<_> = run_lines(&later)
        .iter()
        .map(|run| run["opened_seq"].clone())
        .collect();
    assert_eq!(
        opened,
        [runs[2]["opened_seq"].clone(), runs[3]["opened_seq"].clone()]
    );

    // When a fifth run is open and the cursor is past every event.
    fx.observe(&task.id, a());
    fx.observe(&task.id, a());
    let tail = fx.store.event_tail().unwrap().to_string();
    let with_open = fx.lines(&args);
    let at_tail = fx.lines(&[
        "export",
        "events",
        "--since",
        &tail,
        "--with-conditions",
        "--json",
    ]);
    // Then the open run is included in both windows, with its count as of the export.
    let opened: Vec<_> = run_lines(&with_open)
        .iter()
        .map(|run| run["opened_seq"].as_u64().unwrap())
        .collect();
    assert_eq!(opened.len(), 3, "{opened:?}");
    assert_eq!(
        opened[..2],
        [third, runs[3]["opened_seq"].as_u64().unwrap()]
    );
    assert_eq!(at_tail.len(), 1, "{at_tail:?}");
    assert_eq!(at_tail[0]["condition_run"]["count"], 2);
    assert!(at_tail[0]["condition_run"]["closed_seq"].is_null());
    // And without the flag no run line is written.
    let plain = fx.lines(&["export", "events", "--since", "0", "--json"]);
    assert!(run_lines(&plain).is_empty());
}

#[test]
fn log_orders_equal_timestamps_by_seq_and_wait_never_wakes_on_a_suppressed_repeat() {
    // Given events with one timestamp whose kinds sort opposite to their commit order.
    let fx = Fixture::new();
    let task = fx.store.create("Ordered", None, Status::Ready, "").unwrap();
    let at = "2026-01-01T00:00:00Z";
    let lines: String = ["zeta", "alpha", "moved"]
        .iter()
        .map(|kind| {
            format!(
                "{}\n",
                json!({"ts": at, "task": task.id, "kind": kind, "detail": {}})
            )
        })
        .collect();
    fx.store.fixture_append_events(&lines).unwrap();
    // And a later event stamped before them.
    fx.store
        .fixture_append_events(&format!(
            "{}\n",
            json!({"ts": "2025-01-01T00:00:00Z", "task": task.id, "kind": "late", "detail": {}})
        ))
        .unwrap();
    // When the timeline is read.
    let entries = fx.lines(&["log", &task.id, "--all", "--json"]);
    let seqs: Vec<_> = entries
        .iter()
        .map(|e| e["event"]["seq"].as_u64().unwrap())
        .collect();
    let kinds: Vec<_> = entries
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    // Then it follows commit order.
    assert!(seqs.windows(2).all(|pair| pair[0] < pair[1]), "{seqs:?}");
    assert_eq!(kinds[kinds.len() - 4..], ["zeta", "alpha", "moved", "late"]);

    // Given a cursor at the tail, then repeats that the open run absorbs.
    fx.observe(&task.id, a());
    let cursor = fx.store.event_tail().unwrap();
    for _ in 0..5 {
        fx.observe(&task.id, a());
    }
    // When a wait resumes from that cursor.
    // Routine lifecycle kinds are opt-in under the integrated actionable-default policy.
    let timeout = fx.run(&[
        "wait",
        "--for",
        "done",
        "--after",
        &cursor.to_string(),
        "--timeout",
        "0ms",
    ]);
    // Then the repeats wrote no event, so nothing wakes it.
    assert_eq!(timeout.status.code(), Some(124), "{timeout:?}");
    assert_eq!(fx.store.event_tail().unwrap(), cursor);
    // And the next actionable event takes the very next sequence.
    fx.store
        .log_event(Some(&task.id), "done", json!({}))
        .unwrap();
    let woke = fx.ok(&[
        "wait",
        "--for",
        "done",
        "--after",
        &cursor.to_string(),
        "--json",
        "--timeout",
        "120s",
    ]);
    let woke: Value = serde_json::from_str(&woke).unwrap();
    assert_eq!(woke["kind"], "done");
    assert_eq!(woke["seq"], cursor + 1);
}

fn find_verdict(value: &Value) -> Option<&Value> {
    match value {
        Value::Object(map) if map.get("kind") == Some(&json!("verdict")) => Some(value),
        Value::Object(map) => map.values().find_map(find_verdict),
        Value::Array(items) => items.iter().find_map(find_verdict),
        _ => None,
    }
}

#[test]
fn twenty_kib_verdict_notes_are_a_blob_ref_that_log_and_review_sprint_read_back() {
    // Given a review whose verdict notes are 20 KiB.
    let fx = Fixture::new();
    let task = fx
        .store
        .create("Reviewed", None, Status::Review, "")
        .unwrap();
    let notes = "needs-work ".repeat(2 * 1024).trim_end().to_owned();
    assert!(notes.len() >= 20 * 1024 - 1);
    fx.store
        .update(&task.id, |t| {
            let value = fx.store.event_text(&notes)?;
            fx.store.log_event(
                Some(&t.id),
                "verdict",
                json!({"run": "R1", "verdict": "changes", "reviewer": "rev", "notes": value}),
            )
        })
        .unwrap();
    // Then the stored event holds only a BlobRef.
    let exported = fx.lines(&["export", "events", "--since", "0", "--json"]);
    let verdict = exported.iter().find(|e| e["kind"] == "verdict").unwrap();
    let reference = verdict["detail"]["notes"].as_str().unwrap();
    assert!(reference.starts_with("blob:sha256:"), "{reference}");
    let expected_summary = format!(
        "changes {}",
        notes.split_whitespace().collect::<Vec<_>>().join(" ")
    );

    // When the CLI log and review-sprint read the history (digest and stats are checked on a
    // real verdict in corral-ops' cleanup_ops tests).
    let log = fx.lines(&["log", &task.id, "--json"]);
    let sprint: Value = serde_json::from_str(&fx.ok(&["review-sprint", "--json"])).unwrap();

    // Then both show the text the blob holds.
    let entry = log.iter().find(|e| e["kind"] == "verdict").unwrap();
    assert_eq!(entry["summary"], expected_summary);
    assert_eq!(entry["event"]["detail"]["notes"], reference);
    let change = find_verdict(&sprint).expect("sprint lists the sent-back verdict");
    assert_eq!(change["reason"], notes);
    assert_eq!(change["verdict"], "changes");
}
