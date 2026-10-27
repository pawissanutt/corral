use super::*;
use corral_core::work::{self, Counter, WorkSnapshot};
use corral_core::{Artifact, ArtifactKind, Project, Store};
use jiff::Timestamp;
use serde_json::json;

#[path = "lesson_rows_artifact_tests.rs"]
mod artifacts;
#[path = "lesson_rows_overlap_tests.rs"]
mod overlap;
#[path = "lesson_rows_preparation_tests.rs"]
mod preparation;

fn ts(second: i64) -> Timestamp {
    Timestamp::from_second(1_790_812_800 + second).unwrap()
}

fn run(id: usize, text: &str) -> Run {
    serde_json::from_value(json!({
        "id":format!("R{id}"),"agent":"builder","profile":"worker","harness":"senpi",
        "worktree":"/unused","branch":"work","base":"main","base_sha":"abc",
        "started":ts(0),"ended":ts(60),"outcome":"done","summary":text,
        "usage":{"input":10,"output":20,"cache_read":0,"cache_write":0,
            "reported_usd":0.25,"estimated_usd":0.5}
    }))
    .unwrap()
}

fn fixture() -> (tempfile::TempDir, Ctx) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::init(dir.path()).unwrap();
    store.config.projects.insert(
        "app".into(),
        Project {
            path: "app".into(),
            dispatch: Default::default(),
            default_profile: None,
            base: None,
            review: Default::default(),
            artifact_retention: None,
            post_land: vec![],
        },
    );
    store.config.lessons.after_landings = Some(1);
    store.config.lessons.after_failures = Some(1);
    (dir, Ctx::from_store(store))
}

/// The baseline acquisition algorithm: full task history, full recent-event read and
/// recursive run usage. The observation parser itself is deliberately shared unchanged.
fn legacy(ctx: &Ctx, project: &str) -> Vec<LessonSignal> {
    let mut tasks: Vec<_> = ctx
        .store
        .list()
        .unwrap()
        .into_iter()
        .filter(|task| task.project.as_deref() == Some(project))
        .collect();
    tasks.sort_by(|a, b| (b.updated, &b.id).cmp(&(a.updated, &a.id)));
    tasks.truncate(TASK_LIMIT);
    let events = ctx
        .store
        .recent_events(u64::try_from(EVENT_BYTES).unwrap())
        .unwrap();
    let mut landed = BTreeMap::new();
    for event in &events {
        landed.insert(event.seq, landed_files(ctx, project, event).unwrap());
    }
    let tasks: Vec<_> = tasks
        .into_iter()
        .map(|task| {
            let usage = task
                .runs
                .iter()
                .map(|run| {
                    (
                        run.id.clone(),
                        run.total_usage().map(|usage| {
                            serde_json::from_value(serde_json::to_value(usage).unwrap()).unwrap()
                        }),
                    )
                })
                .collect();
            Source { task, usage }
        })
        .collect();
    crate::lesson_signals::collect(ctx, project, &tasks, &events, &landed).unwrap()
}

fn measure<T>(f: impl FnOnce() -> T) -> (T, WorkSnapshot) {
    // Public lesson readers own their production preparation boundary and HotScope.
    let before = work::snapshot();
    let value = f();
    let snapshot = work::snapshot().since(&before);
    snapshot.check_identity().unwrap();
    assert_eq!(snapshot.unattributed_hydrations(), 0);
    (value, snapshot)
}

fn same_cells(left: &WorkSnapshot, right: &WorkSnapshot) {
    for counter in Counter::ALL {
        assert_eq!(left.get(counter), right.get(counter), "{counter:?}");
    }
    assert_eq!(
        left.unattributed_byte_remainder(),
        right.unattributed_byte_remainder()
    );
}

#[test]
fn derived_rows_equal_full_task_oracle_across_fixture_boards_and_updates() {
    let (_dir, ctx) = fixture();
    let mut seed = 729_u64;
    for i in 0..18 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let status =
            [Status::Running, Status::Done, Status::Dropped][usize::try_from(seed % 3).unwrap()];
        let mut task = ctx
            .store
            .create(&format!("Work {i}"), Some("app"), status, "")
            .unwrap();
        task.kind = Some(if seed & 1 == 0 { "bug" } else { "feature" }.into());
        for j in 1..=usize::try_from(seed % 4 + 1).unwrap() {
            let mut owner = run(j, &format!("test suite::case_{} ... FAILED", seed % 5));
            owner.review_runs.push(run(50, "not emitted child summary"));
            task.runs.push(owner);
        }
        let path = format!("artifacts/{}/R1/report", task.id);
        std::fs::create_dir_all(ctx.store.dir().join(&path).parent().unwrap()).unwrap();
        std::fs::write(ctx.store.dir().join(&path), "Not verified: network\n").unwrap();
        task.artifacts.push(Artifact {
            run: "R1".into(),
            kind: ArtifactKind::Report,
            title: "Report".into(),
            path,
            source: "/unused".into(),
            sha256: String::new(),
            bytes: 22,
            added: ts(i),
            tier_override: None,
            compressed_bytes: None,
            purged_at: None,
        });
        ctx.store.fixture_put_task(&task).unwrap();
        ctx.store
            .log_event(Some(&task.id), "gate", json!({"result":"fail","run":"R1"}))
            .unwrap();
        assert_eq!(
            crate::lesson_signals(&ctx, "app").unwrap(),
            legacy(&ctx, "app")
        );
    }
    let mut task = ctx.store.get("T-001").unwrap();
    task.runs[0]
        .review_runs
        .extend((0..10).map(|_| run(51, "child")));
    ctx.store.fixture_put_task(&task).unwrap();
    assert_eq!(
        crate::lesson_signals(&ctx, "app").unwrap(),
        legacy(&ctx, "app")
    );
    let before = crate::lesson_signals(&ctx, "app").unwrap();
    std::fs::remove_dir_all(ctx.store.cache_root().unwrap()).unwrap();
    prepare(&ctx).unwrap();
    let (after, _) = measure(|| crate::lesson_signals(&ctx, "app").unwrap());
    assert_eq!(before, after);
}

#[test]
fn derived_rows_hot_reads_ignore_terminal_history_events_and_unrelated_records() {
    let (_dir, ctx) = fixture();
    let mut task = ctx
        .store
        .create("Current", Some("app"), Status::Running, "")
        .unwrap();
    task.runs.push(run(1, "Not verified: network"));
    task.runs.splice(0..0, (0..31).map(|i| run(i + 10, "")));
    ctx.store.fixture_put_task(&task).unwrap();
    prepare(&ctx).unwrap();
    let (base, counts) = measure(|| crate::lesson_signals(&ctx, "app").unwrap());
    for i in 0..10 {
        ctx.store
            .create(&format!("Unrelated {i}"), Some("other"), Status::Done, "")
            .unwrap();
        ctx.store
            .log_event(None, "unrelated", json!({"n":i}))
            .unwrap();
    }
    task.runs.splice(0..0, (0..320).map(|i| run(i + 100, "")));
    ctx.store.fixture_put_task(&task).unwrap();
    prepare(&ctx).unwrap();
    // History grows tenfold below the bounded prefix; emitted signals stay equal.
    let (grown, grown_counts) = measure(|| crate::lesson_signals(&ctx, "app").unwrap());
    assert_eq!(grown, legacy(&ctx, "app"));
    assert_eq!(grown, base);
    same_cells(&counts, &grown_counts);
    assert_eq!(counts.get(Counter::EventDecodes), 0);
    assert_eq!(counts.get(Counter::Processes), 0);
    let recomputed = recomputations(&ctx).unwrap();
    corral_core::records::RecordArea::new("resources")
        .save(&ctx.store, "observation", &json!({"count":10}))
        .unwrap();
    let (_, record_counts) = measure(|| crate::lesson_signals(&ctx, "app").unwrap());
    assert_eq!(recomputed, recomputations(&ctx).unwrap());
    same_cells(&counts, &record_counts);
}

#[test]
fn derived_schedule_preserves_request_append_order_pending_and_announcement() {
    let (_dir, ctx) = fixture();
    let work = ctx
        .store
        .create("Done", Some("app"), Status::Done, "")
        .unwrap();
    let request = ctx
        .store
        .create("Distill", Some("app"), Status::Running, "")
        .unwrap();
    ctx.store
        .log_event(Some(&work.id), "approved", json!({"strategy":"rebase"}))
        .unwrap();
    ctx.store
        .log_event(
            Some(&request.id),
            "lessons_distill_requested",
            json!({"project":"app"}),
        )
        .unwrap();
    ctx.store
        .log_event(Some(&work.id), "gate", json!({"result":"fail"}))
        .unwrap();
    prepare(&ctx).unwrap();
    let (due, counts) = measure(|| crate::lessons_due(&ctx, ts(3600)).unwrap());
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].pending.as_deref(), Some(request.id.as_str()));
    assert_eq!(due[0].reasons, ["1 failure signals since last request"]);
    for i in 0..10 {
        ctx.store
            .log_event(None, "unrelated", json!({"i":i}))
            .unwrap();
        ctx.store
            .create("Unrelated", Some("other"), Status::Dropped, "")
            .unwrap();
    }
    prepare(&ctx).unwrap();
    let (grown, grown_counts) = measure(|| crate::lessons_due(&ctx, ts(3600)).unwrap());
    assert_eq!(
        serde_json::to_value(due).unwrap(),
        serde_json::to_value(grown).unwrap()
    );
    same_cells(&counts, &grown_counts);
    ctx.store
        .update(&request.id, |task| {
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();
    crate::lesson_distill::announce_due(&ctx, ts(3600)).unwrap();
    crate::lesson_distill::announce_due(&ctx, ts(3600)).unwrap();
    assert_eq!(
        ctx.store.events_of(None, &["lessons_due"]).unwrap().len(),
        1
    );
}

#[test]
fn derived_landed_paths_survive_git_memo_deletion_without_git_or_event_scans() {
    let (dir, ctx) = fixture();
    let repo = dir.path().join("app");
    std::fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = crate::test_support::command("git", &repo)
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("old.rs"), "fn old() {}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "initial"]);
    let initial = git(&["rev-parse", "HEAD"]);
    std::fs::write(repo.join("new.rs"), "fn new() {}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "landed"]);
    let landed = git(&["rev-parse", "HEAD"]);
    let task = ctx
        .store
        .create("Landed", Some("app"), Status::Done, "")
        .unwrap();
    ctx.store
        .log_event(
            Some(&task.id),
            "approved",
            json!({"sha":landed,"previous_sha":initial,"strategy":"rebase","run":"R1"}),
        )
        .unwrap();
    let expected = legacy(&ctx, "app");
    prepare(&ctx).unwrap();
    assert!(
        expected
            .iter()
            .any(|signal| signal.category == "landed_files" && signal.files == ["new.rs"])
    );
    let memo = ctx.store.cache_root().unwrap().join("git");
    assert!(
        memo.is_dir(),
        "fixture must populate the actual Phase 0 git memo"
    );
    std::fs::remove_dir_all(&memo).unwrap();
    let (actual, before) = measure(|| crate::lesson_signals(&ctx, "app").unwrap());
    assert_eq!(actual, expected);
    assert_eq!(before.get(Counter::Processes), 0);
    assert_eq!(before.get(Counter::EventDecodes), 0);
    assert!(!memo.exists(), "a row lookup must not recreate the memo");
    for i in 0..10 {
        ctx.store
            .log_event(None, "unrelated", json!({"i":i}))
            .unwrap();
        ctx.store
            .create("Old terminal", Some("other"), Status::Dropped, "")
            .unwrap();
    }
    prepare(&ctx).unwrap();
    let (actual, grown) = measure(|| crate::lesson_signals(&ctx, "app").unwrap());
    assert_eq!(actual, expected);
    same_cells(&before, &grown);
    assert_eq!(grown.get(Counter::Processes), 0);
    let generation = recomputations(&ctx).unwrap();
    corral_core::records::RecordArea::new("condition-counts")
        .save(&ctx.store, "example", &json!({"count":10}))
        .unwrap();
    prepare(&ctx).unwrap();
    assert_eq!(generation, recomputations(&ctx).unwrap());
    std::fs::remove_dir_all(ctx.store.cache_root().unwrap()).unwrap();
    prepare(&ctx).unwrap();
    assert_eq!(
        measure(|| crate::lesson_signals(&ctx, "app").unwrap()).0,
        expected
    );
}

#[test]
fn derived_rows_rebuild_after_a_real_feed_gap_on_both_derivations() {
    let (_dir, ctx) = fixture();
    let task = ctx
        .store
        .create("Landed", Some("app"), Status::Done, "")
        .unwrap();
    ctx.store
        .log_event(
            Some(&task.id),
            "approved",
            json!({"sha":"first","strategy":"rebase","run":"R1"}),
        )
        .unwrap();
    prepare(&ctx).unwrap();
    crate::approvals::prepare(&ctx).unwrap();
    ctx.store
        .log_event(
            Some(&task.id),
            "approved",
            json!({"sha":"second","strategy":"rebase","run":"R2"}),
        )
        .unwrap();
    let through = ctx.store.view().unwrap().seq();
    let backend =
        corral_core::sqlite::SqliteBackend::open(ctx.store.dir(), Default::default()).unwrap();
    backend.prune_changes_through(through).unwrap();
    prepare(&ctx).unwrap();
    crate::approvals::prepare(&ctx).unwrap();
    assert_eq!(
        crate::lesson_signals(&ctx, "app").unwrap(),
        legacy(&ctx, "app")
    );
    let rows = crate::approvals::lookup(
        &ctx,
        &Some("app".into()),
        &["first".into(), "second".into()].into(),
    )
    .unwrap();
    assert_eq!(rows["first"][0].id, task.id);
    assert_eq!(rows["second"][0].run.as_deref(), Some("R2"));
    let (_, counts) = measure(|| crate::lesson_signals(&ctx, "app").unwrap());
    assert_eq!(counts.get(Counter::EventDecodes), 0);
}

#[test]
fn derived_rows_never_publish_newer_events_into_an_older_snapshot() {
    let (_dir, ctx) = fixture();
    let task = ctx
        .store
        .create("Work", Some("app"), Status::Done, "")
        .unwrap();
    prepare(&ctx).unwrap();
    let frozen = Ctx {
        store: ctx.store.read_only_snapshot().unwrap(),
        ..ctx.clone()
    };
    let old = crate::lesson_signals(&frozen, "app").unwrap();
    ctx.store
        .log_event(
            Some(&task.id),
            "changes_requested",
            json!({"text":"test newly::failed ... FAILED"}),
        )
        .unwrap();
    let new = crate::lesson_signals(&ctx, "app").unwrap();
    assert_ne!(old, new);
    assert_eq!(crate::lesson_signals(&frozen, "app").unwrap(), old);
    assert_eq!(crate::lesson_signals(&ctx, "app").unwrap(), new);
}

#[test]
fn derived_recent_tail_keeps_malformed_row_bytes_when_following_events() {
    let (_dir, ctx) = fixture();
    let task = ctx
        .store
        .create("Work", Some("app"), Status::Running, "")
        .unwrap();
    ctx.store
        .log_event(
            Some(&task.id),
            "changes_requested",
            json!({"text":"test old::case ... FAILED"}),
        )
        .unwrap();
    ctx.store
        .fixture_append_events(&format!("{}\n", "x".repeat(EVENT_BYTES - 800)))
        .unwrap();
    ctx.store
        .log_event(
            Some(&task.id),
            "changes_requested",
            json!({"text":"test new::case ... FAILED"}),
        )
        .unwrap();
    assert_eq!(
        crate::lesson_signals(&ctx, "app").unwrap(),
        legacy(&ctx, "app")
    );
    ctx.store
        .log_event(None, "unrelated", json!({"padding":"x".repeat(2000)}))
        .unwrap();
    let actual = crate::lesson_signals(&ctx, "app").unwrap();
    assert_eq!(actual, legacy(&ctx, "app"));
    assert!(
        !actual
            .iter()
            .any(|signal| signal.key == "failed_test:old::case")
    );
    assert!(
        actual
            .iter()
            .any(|signal| signal.key == "failed_test:new::case")
    );
}

#[test]
fn artifact_acquisition_bounds_keys_before_decoding_under_history_growth() {
    // Given 32 retained artifacts, all newer than any history added below.
    let (_dir, ctx) = fixture();
    let mut task = ctx
        .store
        .create("Artifacts", Some("app"), Status::Running, "")
        .unwrap();
    let artifact = |i: usize| Artifact {
        run: "R1".into(),
        kind: ArtifactKind::Report,
        title: "Report".into(),
        path: format!("artifacts/report-{i:04}"),
        source: "/unused".into(),
        sha256: String::new(),
        bytes: 0,
        added: ts(i64::try_from(i).unwrap()),
        tier_override: None,
        compressed_bytes: None,
        purged_at: None,
    };
    task.artifacts.extend((1000..1032).map(artifact));
    ctx.store.fixture_put_task(&task).unwrap();
    let acquire = || {
        let head = ctx.store.head(&task.id).unwrap();
        let _scope = corral_core::work::HotScope::enter();
        let before = work::snapshot();
        let input = source(&ctx, &head).unwrap();
        let counts = work::snapshot().since(&before);
        counts.check_identity().unwrap();
        assert_eq!(counts.unattributed_hydrations(), 0);
        (input.task.artifacts, counts)
    };
    let (base, counts) = acquire();
    // When 320 older artifact keys are prepended to the same active task.
    task.artifacts.splice(0..0, (0..320).map(artifact));
    ctx.store.fixture_put_task(&task).unwrap();
    let (grown, grown_counts) = acquire();
    // Then only the same newest 32 documents are decoded, not the full accumulated vector.
    assert_eq!(base, grown);
    assert_eq!(counts.get(Counter::HistoryRowDecodes), 32);
    same_cells(&counts, &grown_counts);
    assert_eq!(
        crate::lesson_signals(&ctx, "app").unwrap(),
        legacy(&ctx, "app")
    );
}
