use super::*;
use corral_core::{Artifact, Run, Store};
use serde_json::json;

fn timestamp() -> Timestamp {
    "2026-10-01T00:00:00Z".parse().unwrap()
}

fn fixture_run(summary: &str) -> Run {
    serde_json::from_value(json!({
        "id":"R1", "agent":"builder", "harness":"senpi",
        "worktree":"/unused", "branch":"task", "base":"main",
        "base_sha":"abc", "started":"2026-10-01T00:00:00Z",
        "ended":"2026-10-01T00:01:00Z", "outcome":"done",
        "summary":summary, "failures":2,
        "usage":{"input":10,"output":20,"cache_read":0,"cache_write":0,
            "reported_usd":0.25,"estimated_usd":0.5}
    }))
    .unwrap()
}

fn artifact(ctx: &Ctx, task: &mut Task, name: &str, kind: ArtifactKind, text: &str) {
    let path = format!("artifacts/{}/R1/{name}", task.id);
    let absolute = ctx.store.dir().join(&path);
    fs::create_dir_all(absolute.parent().unwrap()).unwrap();
    fs::write(absolute, text).unwrap();
    task.artifacts.push(Artifact {
        run: "R1".into(),
        kind,
        title: name.into(),
        path,
        source: "/not/read".into(),
        sha256: String::new(),
        bytes: u64::try_from(text.len()).unwrap(),
        added: timestamp(),
        tier_override: None,
        compressed_bytes: None,
        purged_at: None,
    });
}

fn event(task: &Task, kind: &str, detail: serde_json::Value) -> Event {
    Event {
        task: Some(task.id.clone()),
        ts: timestamp(),
        kind: kind.into(),
        detail,
    }
}

fn write_events(ctx: &Ctx, events: &[Event]) {
    let text: String = events
        .iter()
        .map(|event| format!("{}\n", serde_json::to_string(event).unwrap()))
        .collect();
    fs::write(ctx.store.dir().join("events.jsonl"), text).unwrap();
}

#[test]
fn observations_cluster_across_tasks_and_remain_project_isolated() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(dir.path()).unwrap());
    let mut events = Vec::new();
    for (project, suffix) in [("app", ""), ("app", "  "), ("other", "secret")] {
        let mut task = ctx
            .store
            .create("Fix", Some(project), Status::Done, "")
            .unwrap();
        task.kind = Some("bug".into());
        task.rounds = 2;
        task.runs.push(fixture_run(
            "Not verified: network access\nNot verified: none",
        ));
        artifact(
            &ctx,
            &mut task,
            "gate.log",
            ArtifactKind::Log,
            &format!(
                "test suite::regression ... FAILED{suffix}\n\
             src/large.rs: {} SLOC exceeds limit 800\n\
             CONFLICT (content): Merge conflict in src/shared.rs\n",
                801 + events.len(),
            ),
        );
        artifact(
            &ctx,
            &mut task,
            "final.diff",
            ArtifactKind::Diff,
            "--- a/src/old.rs\n+++ b/src/new.rs\n",
        );
        task.runs[0].review = Some(
            serde_json::from_value(json!({
                "reviewer":"reviewer", "verdict":"changes", "notes":"[P1] Check the input boundary",
                "sha":"abc", "at":"2026-10-01T00:01:00Z"
            }))
            .unwrap(),
        );
        events.extend([
            event(
                &task,
                "changes_requested",
                json!({"text":"check   the INPUT boundary"}),
            ),
            event(&task, "gate", json!({"result":"fail","run":"R1"})),
            event(
                &task,
                "autopilot",
                json!({"to":"escalated","round":2,"reason":"review budget exhausted"}),
            ),
            event(&task, "approved", json!({"run":"R1","sha":"abc"})),
        ]);
        ctx.store.save(&task).unwrap();
    }
    write_events(&ctx, &events);
    let signals = lesson_signals(&ctx, "app").unwrap();
    for key in [
        "failed_test:suite::regression",
        "size_ceiling:src/large.rs",
        "conflict:src/shared.rs",
        "reviewer_note:check the input boundary",
    ] {
        let cluster = signals.iter().find(|signal| signal.key == key).unwrap();
        assert_eq!(
            cluster
                .evidence
                .iter()
                .map(|e| &e.task)
                .collect::<BTreeSet<_>>()
                .len(),
            2
        );
    }
    assert!(
        signals
            .iter()
            .all(|signal| signal.evidence.iter().all(|e| e.task != "T-003"))
    );
    assert_eq!(
        signals
            .iter()
            .filter(|s| s.category == "not_verified")
            .count(),
        1
    );
    let touched = signals
        .iter()
        .find(|s| s.category == "touched_files")
        .unwrap();
    assert_eq!(touched.files, ["src/new.rs", "src/old.rs"]);
    let stats = signals.iter().find(|s| s.category == "run_stats").unwrap();
    assert_eq!(stats.profiles, ["builder"]);
    assert_eq!(stats.harnesses, ["senpi"]);
    assert_eq!(stats.kinds, ["bug"]);
    assert!(signals.iter().any(|s| s.category == "success"));
    assert!(signals.iter().any(|s| s.category == "round"));
    assert_eq!(
        serde_json::to_value(&signals).unwrap(),
        serde_json::to_value(lesson_signals(&ctx, "app").unwrap()).unwrap()
    );
    assert!(lesson_signals(&ctx, "absent").unwrap().is_empty());
}

#[test]
fn normalization_recognizes_test_runner_forms_without_clustering_different_tests() {
    assert_eq!(
        failed_test("test suite::works ... FAILED"),
        Some("suite::works".into())
    );
    assert_eq!(
        failed_test("--- FAIL: TestWorks (0.10s)"),
        Some("testworks".into())
    );
    assert_eq!(
        failed_test("FAIL [ 0.01s] package suite::works"),
        Some("suite::works".into())
    );
    assert_eq!(failed_test("test suite::works ... ok"), None);
    assert_ne!(
        failed_test("test suite::one ... FAILED"),
        failed_test("test suite::two ... FAILED")
    );
    assert_eq!(
        mentioned_files("src/large.rs:901:2 exceeds SLOC ceiling"),
        BTreeSet::from(["src/large.rs".into()])
    );
}

#[test]
fn bounded_reader_rejects_traversal_directories_and_symlinks_and_caps_bytes() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("log"), b"abcdef").unwrap();
    assert_eq!(
        bounded_read(dir.path(), Path::new("log"), 3).unwrap(),
        Some(b"abc".to_vec())
    );
    assert!(
        bounded_read(dir.path(), Path::new("../outside"), 3)
            .unwrap()
            .is_none()
    );
    assert!(
        bounded_read(dir.path(), Path::new("/dev/zero"), 3)
            .unwrap()
            .is_none()
    );
    assert!(
        bounded_read(dir.path(), Path::new("missing"), 3)
            .unwrap()
            .is_none()
    );
    fs::create_dir(dir.path().join("directory")).unwrap();
    assert!(
        bounded_read(dir.path(), Path::new("directory"), 3)
            .unwrap()
            .is_none()
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("log", dir.path().join("link")).unwrap();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("parent")).unwrap();
        std::os::unix::fs::symlink("/dev/zero", dir.path().join("device")).unwrap();
        for name in ["link", "parent/log", "device"] {
            assert!(
                bounded_read(dir.path(), Path::new(name), 3)
                    .unwrap()
                    .is_none()
            );
        }
    }
}

#[test]
fn compressed_artifacts_and_context_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(dir.path()).unwrap());
    let mut task = ctx
        .store
        .create("Work", Some("app"), Status::Dropped, "")
        .unwrap();
    artifact(&ctx, &mut task, "gate.log", ArtifactKind::Log, "");
    let bytes = crate::artifact_storage::gzip(&vec![b'x'; ARTIFACT_BYTES * 4]).unwrap();
    fs::write(ctx.store.dir().join(&task.artifacts[0].path), &bytes).unwrap();
    task.artifacts[0].compressed_bytes = Some(u64::try_from(bytes.len()).unwrap());
    assert_eq!(
        artifact_text(&ctx, &task.artifacts[0], ARTIFACT_BYTES)
            .unwrap()
            .unwrap()
            .len(),
        ARTIFACT_BYTES
    );
    let mut collector = Collector::default();
    let input = "x".repeat(CONTEXT_BYTES * 2);
    assert_eq!(collector.context(&input).len(), CONTEXT_BYTES);
    assert!(collector.context("more").is_empty());
    let source = evidence(&task, timestamp(), "event:discarded");
    for number in 0..SIGNAL_LIMIT * 2 {
        collector.add(
            &task,
            &source,
            "fixture",
            &number.to_string(),
            "observation",
            &[],
        );
    }
    assert_eq!(collector.signals.len(), SIGNAL_LIMIT);
}

#[test]
fn explicit_other_project_events_are_not_attributed_to_current_project() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(dir.path()).unwrap());
    let task = ctx
        .store
        .create("Moved", Some("app"), Status::Running, "")
        .unwrap();
    write_events(
        &ctx,
        &[event(
            &task,
            "changes_requested",
            json!({
                "project":"other", "text":"test hidden::case ... FAILED"
            }),
        )],
    );
    assert!(lesson_signals(&ctx, "app").unwrap().is_empty());
}

#[test]
fn recent_event_suffix_drops_partial_first_line_and_retains_new_observations() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(dir.path()).unwrap());
    let task = ctx
        .store
        .create("Work", Some("app"), Status::Running, "")
        .unwrap();
    let old = event(
        &task,
        "changes_requested",
        json!({"text":"test old::case ... FAILED"}),
    );
    let recent = event(
        &task,
        "changes_requested",
        json!({"text":"test recent::case ... FAILED"}),
    );
    let mut bytes = format!("{}\n", serde_json::to_string(&old).unwrap()).into_bytes();
    bytes.extend(vec![b'x'; EVENT_BYTES + 10]);
    bytes.extend_from_slice(format!("\n{}\n", serde_json::to_string(&recent).unwrap()).as_bytes());
    fs::write(ctx.store.dir().join("events.jsonl"), bytes).unwrap();
    let signals = lesson_signals(&ctx, "app").unwrap();
    assert!(signals.iter().any(|s| s.key == "failed_test:recent::case"));
    assert!(!signals.iter().any(|s| s.key == "failed_test:old::case"));
    let encoded = serde_json::to_string(&signals).unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<LessonSignal>>(&encoded).unwrap(),
        signals
    );
    fs::write(ctx.store.dir().join("boundary"), b"one\ntwo\n").unwrap();
    assert_eq!(
        bounded_source(ctx.store.dir(), Path::new("boundary"), 4, true).unwrap(),
        Some(b"two\n".to_vec())
    );
}

#[test]
fn latest_tasks_are_selected_by_persisted_time() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(dir.path()).unwrap());
    for index in 0..=TASK_LIMIT {
        let mut task = ctx
            .store
            .create("Work", Some("app"), Status::Running, "")
            .unwrap();
        task.updated = timestamp()
            .checked_add(jiff::SignedDuration::from_secs(
                i64::try_from(index).unwrap(),
            ))
            .unwrap();
        if index == 0 {
            task.runs.push(fixture_run("test old::case ... FAILED"));
        } else if index == TASK_LIMIT {
            task.runs.push(fixture_run("test recent::case ... FAILED"));
        }
        fs::write(&task.path, corral_core::render_task(&task)).unwrap();
    }
    let signals = lesson_signals(&ctx, "app").unwrap();
    assert!(signals.iter().any(|s| s.key == "failed_test:recent::case"));
    assert!(!signals.iter().any(|s| s.key == "failed_test:old::case"));
}

#[test]
fn newest_tasks_get_signal_budget_before_older_busy_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(dir.path()).unwrap());
    let mut old = ctx
        .store
        .create("Old", Some("app"), Status::Running, "")
        .unwrap();
    let failures: String = (0..SIGNAL_LIMIT * 2)
        .map(|number| format!("test old::case_{number} ... FAILED\n"))
        .collect();
    artifact(&ctx, &mut old, "gate.log", ArtifactKind::Log, &failures);
    old.updated = timestamp();
    fs::write(&old.path, corral_core::render_task(&old)).unwrap();
    let mut recent = ctx
        .store
        .create("Recent", Some("app"), Status::Running, "")
        .unwrap();
    recent.updated = timestamp()
        .checked_add(jiff::SignedDuration::from_secs(1))
        .unwrap();
    recent
        .runs
        .push(fixture_run("test recent::case ... FAILED"));
    fs::write(&recent.path, corral_core::render_task(&recent)).unwrap();
    let signals = lesson_signals(&ctx, "app").unwrap();
    assert_eq!(signals.len(), SIGNAL_LIMIT);
    assert!(signals.iter().any(|s| s.key == "failed_test:recent::case"));
    assert!(signals.windows(2).all(|pair| pair[0].key < pair[1].key));
}
