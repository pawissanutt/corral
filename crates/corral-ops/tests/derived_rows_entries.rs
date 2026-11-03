//! Exercise initialization, mutation and recovery through production entries, not prepare().
use std::path::Path;
use std::process::{Command, Stdio};

use corral_core::work::{self, Counter, WorkSnapshot};
use corral_core::{Project, Run, Status, Store};
use corral_ops::{Ctx, PmActionKind};
use jiff::Timestamp;
use serde_json::{Value, json};

fn now() -> Timestamp {
    "2026-10-27T12:00:00Z".parse().unwrap()
}

fn context(store: Store) -> Ctx {
    store.fixture_pin_now(now()).unwrap();
    let mut ctx = corral_ops::resources::FixedResourceProbe::ctx(store);
    ctx.herdr = corral_herdr::Herdr::with_bin("/nonexistent/derived-rows-herdr".into(), None);
    ctx
}

fn append(ctx: &Ctx, task: Option<&str>, kind: &str, detail: Value) {
    ctx.store
        .fixture_append_events(&format!(
            "{}\n",
            json!({"ts":now(),"task":task,"kind":kind,"detail":detail})
        ))
        .unwrap();
}

fn fixture(scale: usize) -> (tempfile::TempDir, Ctx, String) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("app")).unwrap();
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
    store.config.lessons.after_failures = Some(1);
    store.save_config().unwrap();
    let ctx = context(store);
    let mut active = ctx
        .store
        .create("Active", Some("app"), Status::Running, "")
        .unwrap();
    let current: Run = serde_json::from_value(json!({
        "id":"R100","agent":"generic","worktree":"/unused","branch":"work","base":"main",
        "base_sha":"abc","started":now(),"summary":"alpha"
    }))
    .unwrap();
    active.runs.push(current.clone());
    active.created = now();
    active.updated = now();
    ctx.store.fixture_put_task(&active).unwrap();
    let mut request = ctx
        .store
        .create("Previous request", Some("app"), Status::Done, "")
        .unwrap();
    request.created = now();
    request.updated = now();
    ctx.store.fixture_put_task(&request).unwrap();
    append(
        &ctx,
        Some(&request.id),
        "lessons_distill_requested",
        json!({"project":"app"}),
    );
    append(
        &ctx,
        Some(&active.id),
        "gate",
        json!({"result":"fail","run":"R100"}),
    );
    for _ in 0..scale {
        let mut task = ctx
            .store
            .create("History", Some("app"), Status::Done, "")
            .unwrap();
        task.runs.extend((0..scale).map(|i| {
            let mut run = current.clone();
            run.id = format!("R{i:04}");
            run.ended = Some(now());
            run.outcome = Some(corral_core::RunOutcome::Done);
            run
        }));
        ctx.store.fixture_put_task(&task).unwrap();
    }
    (dir, ctx, active.id)
}

/// db316644's full-task, full-event schedule algorithm, without derived cache inputs.
fn oracle(ctx: &Ctx) -> Value {
    let tasks = ctx.store.list().unwrap();
    let events = ctx.store.events().unwrap();
    let mut due = Vec::new();
    for project in ctx.store.config.projects.keys() {
        let requested_index = events.iter().rposition(|event| {
            event.kind == "lessons_distill_requested"
                && event.detail["project"].as_str() == Some(project)
        });
        let requested = requested_index.map(|i| &events[i]);
        let pending = requested
            .and_then(|event| event.task.as_deref())
            .and_then(|id| tasks.iter().find(|task| task.id == id))
            .filter(|task| !matches!(task.status, Status::Done | Status::Dropped))
            .map(|task| task.id.clone());
        let relevant = events
            .iter()
            .skip(requested_index.map_or(0, |i| i + 1))
            .filter(|event| {
                event.task.as_ref().is_some_and(|id| {
                    tasks
                        .iter()
                        .any(|task| &task.id == id && task.project.as_ref() == Some(project))
                })
            })
            .collect::<Vec<_>>();
        let landings = relevant
            .iter()
            .filter(|event| {
                event.kind == "approved"
                    && matches!(
                        event.detail["strategy"].as_str(),
                        Some("fast_forward" | "rebase")
                    )
            })
            .count();
        let failures = relevant
            .iter()
            .filter(|event| {
                (event.kind == "gate" && event.detail["result"] == "fail")
                    || matches!(event.kind.as_str(), "changes_requested" | "discarded")
                    || corral_core::actionable_kind(event) == Some("escalated")
            })
            .count();
        let has_history = tasks.iter().any(|task| {
            task.project.as_ref() == Some(project)
                && (matches!(task.status, Status::Done | Status::Dropped) || !task.runs.is_empty())
        });
        let config = &ctx.store.config.lessons;
        let mut reasons = Vec::new();
        if let Some(every) = &config.every
            && has_history
            && requested.is_none_or(|event| {
                now().duration_since(event.ts) >= corral_core::parse_age(every).unwrap()
            })
        {
            reasons.push(format!("interval {every} elapsed"));
        }
        if config
            .after_landings
            .is_some_and(|n| landings >= usize::try_from(n).unwrap())
        {
            reasons.push(format!("{landings} landings since last request"));
        }
        if config
            .after_failures
            .is_some_and(|n| failures >= usize::try_from(n).unwrap())
        {
            reasons.push(format!("{failures} failure signals since last request"));
        }
        if !reasons.is_empty() || pending.is_some() {
            due.push(json!({"project":project,"reasons":reasons,"profile":config.profile,"pending":pending}));
        }
    }
    json!(due)
}

fn measured(ctx: &Ctx) -> (Value, WorkSnapshot) {
    // pm_workset owns the production preparation boundary and the measured HotScope.
    let before = work::snapshot();
    let workset = corral_ops::pm_workset(ctx).unwrap();
    let counts = work::snapshot().since(&before);
    counts.check_identity().unwrap();
    assert_eq!(counts.unattributed_hydrations(), 0);
    assert_eq!(workset.next.kind, PmActionKind::LessonsDistill);
    assert_eq!(
        counts.get(Counter::RecordDecodes),
        0,
        "prepared scheduling values must not reopen shared rows: {counts:?}"
    );
    assert_eq!(counts.get(Counter::EventDecodes), 0);
    assert_eq!(
        counts.get(Counter::ViewsOpened),
        0,
        "the prepared view is reused"
    );
    let due = serde_json::from_str::<Value>(&workset.next.evidence[0]).unwrap();
    assert_eq!(json!([due.clone()]), oracle(ctx));
    (due, counts)
}

fn same(left: &WorkSnapshot, right: &WorkSnapshot) {
    for counter in Counter::ALL {
        assert_eq!(left.get(counter), right.get(counter), "{counter:?}");
    }
    assert_eq!(
        left.unattributed_byte_remainder(),
        right.unattributed_byte_remainder()
    );
}

#[test]
fn production_pm_recovers_and_follows_mutations_without_history_sized_hot_work() {
    // Given the same active run and request, with 1x/10x terminal tasks and history.
    let (_base_dir, base, base_id) = fixture(1);
    let (_grown_dir, grown, grown_id) = fixture(10);
    let (expected, counts) = measured(&base);
    let (actual, grown_counts) = measured(&grown);
    assert_eq!(expected, actual);
    same(&counts, &grown_counts);
    // When the real current run changes and a new scheduling event arrives.
    for (ctx, id) in [(&base, &base_id), (&grown, &grown_id)] {
        ctx.store
            .update_head(id, |head| {
                head.current_run_mut().unwrap().summary = Some("omega".into());
                Ok(())
            })
            .unwrap();
        // Wall-clock precision is not under test; keep mutated-head bytes deterministic.
        let mut task = ctx.store.get(id).unwrap();
        task.updated = now();
        ctx.store.fixture_put_task(&task).unwrap();
        append(ctx, Some(id), "gate", json!({"result":"fail","run":"R100"}));
    }
    let (expected, counts) = measured(&base);
    let (actual, grown_counts) = measured(&grown);
    assert_eq!(expected, actual);
    same(&counts, &grown_counts);
    // Then deletion is recovered by the same production entry, without any test preparation.
    for ctx in [&base, &grown] {
        std::fs::remove_dir_all(ctx.store.cache_root().unwrap()).unwrap();
    }
    let (recovered, recovered_counts) = measured(&base);
    let (grown_recovered, grown_recovered_counts) = measured(&grown);
    assert_eq!(expected, recovered);
    assert_eq!(expected, grown_recovered);
    same(&counts, &recovered_counts);
    same(&counts, &grown_recovered_counts);
    // Earlier cache versions used JSON numbers. Production preparation upgrades those
    // cursors rather than making existing caches unreadable.
    for ctx in [&base, &grown] {
        let seq = ctx.store.view().unwrap().seq().0;
        let event = ctx.store.event_tail().unwrap();
        // Approval attribution is now maintained in the committed backend, not this cache.
        std::fs::write(
            ctx.store
                .cache_root()
                .unwrap()
                .join("lesson-schedule-v1/cursor.json"),
            serde_json::to_vec(&json!({
                "change":seq,"event":event,"config":"legacy","recomputations":0
            }))
            .unwrap(),
        )
        .unwrap();
    }
    let (upgraded, upgraded_counts) = measured(&base);
    let (grown_upgraded, grown_upgraded_counts) = measured(&grown);
    assert_eq!(expected, upgraded);
    assert_eq!(expected, grown_upgraded);
    same(&counts, &upgraded_counts);
    same(&counts, &grown_upgraded_counts);
}

fn child(root: &Path, sync: bool) -> Value {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "derived_rows_child", "--nocapture"])
        .env("CORRAL_DERIVED_CHILD", root)
        .env("CORRAL_DERIVED_SYNC_CHILD", if sync { "1" } else { "0" })
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    serde_json::from_str(
        stdout
            .lines()
            .find_map(|line| line.strip_prefix("DERIVED:"))
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn derived_rows_child() {
    let Some(root) = std::env::var_os("CORRAL_DERIVED_CHILD") else {
        return;
    };
    let ctx = context(Store::discover(Path::new(&root)).unwrap());
    if std::env::var("CORRAL_DERIVED_SYNC_CHILD").as_deref() == Ok("1") {
        let expected = oracle(&ctx);
        let before = work::snapshot();
        corral_ops::sync_at(&ctx, now()).unwrap();
        let counts = work::snapshot().since(&before);
        counts.check_identity().unwrap();
        assert_eq!(counts.unattributed_hydrations(), 0);
        let announcements = ctx
            .store
            .events_of(None, &["lessons_due"])
            .unwrap()
            .into_iter()
            .filter(|event| event.detail["project"] == "app" && event.detail["request"] == "T-002")
            .count();
        assert_eq!(announcements, 1);
        assert_eq!(
            serde_json::to_value(corral_ops::lessons_due(&ctx, now()).unwrap()).unwrap(),
            expected
        );
        println!(
            "DERIVED:{}",
            json!({"announcements":announcements,"cells":Counter::ALL.map(|counter|counts.get(counter))})
        );
        return;
    }
    let (due, counts) = measured(&ctx);
    println!(
        "DERIVED:{}",
        json!({"due":due,"cells":Counter::ALL.map(|counter|counts.get(counter))})
    );
}

#[test]
fn fresh_process_pm_entries_recover_deleted_caches_at_both_history_sizes() {
    // Given cold stores, each child starts without process-local memoized derivations.
    let (base_dir, base, _) = fixture(1);
    let (grown_dir, grown, _) = fixture(10);
    let expected = child(base_dir.path(), false);
    assert_eq!(expected, child(grown_dir.path(), false));
    // When durable caches are removed between fresh processes.
    for ctx in [&base, &grown] {
        std::fs::remove_dir_all(ctx.store.cache_root().unwrap()).unwrap();
    }
    // Then real PM entries rebuild before their own HotScopes and return the full-task oracle.
    assert_eq!(expected, child(base_dir.path(), false));
    assert_eq!(expected, child(grown_dir.path(), false));
}

#[test]
fn fresh_process_sync_announcement_reads_recover_deleted_cache() {
    // Given two due requests with different amounts of announcement and task history.
    let (base_dir, base, _) = fixture(1);
    let (grown_dir, grown, _) = fixture(10);
    // Equal-width event positions: sync stores its PM event cursor as a record.
    for _ in 0..10 {
        append(&base, None, "padding", json!({}));
    }
    for (ctx, scale) in [(&base, 1), (&grown, 10)] {
        ctx.store
            .update("T-001", |task| {
                task.status = Status::Done;
                task.current_run_mut().unwrap().ended = Some(now());
                task.current_run_mut().unwrap().outcome = Some(corral_core::RunOutcome::Done);
                Ok(())
            })
            .unwrap();
        for i in 0..scale {
            for project in ["app", "other"] {
                append(
                    ctx,
                    None,
                    "lessons_due",
                    json!({"project":project,"request":format!("old-{i}")}),
                );
            }
        }
    }
    // When cold processes enter sync, both announce exactly once with equal measured work.
    assert_eq!(child(base_dir.path(), true), child(grown_dir.path(), true));
    for ctx in [&base, &grown] {
        std::fs::remove_dir_all(ctx.store.cache_root().unwrap()).unwrap();
    }
    // Then fresh processes recover the keyed announcement fact and do not reannounce.
    assert_eq!(child(base_dir.path(), true), child(grown_dir.path(), true));
}

#[test]
fn sync_announces_unseen_request_with_constant_hot_work_under_announcement_growth() {
    let mut samples = Vec::new();
    for scale in [1, 10] {
        // Given a due, unannounced request and past announcements for both projects.
        let (_dir, ctx, _) = fixture(scale);
        // Sync has no active pane to reconcile; the historical run still supplies history.
        ctx.store
            .update("T-001", |task| {
                task.status = Status::Done;
                task.current_run_mut().unwrap().ended = Some(now());
                task.current_run_mut().unwrap().outcome = Some(corral_core::RunOutcome::Done);
                Ok(())
            })
            .unwrap();
        for i in 0..scale {
            for project in ["app", "other"] {
                append(
                    &ctx,
                    None,
                    "lessons_due",
                    json!({"project":project,"request":format!("old-{i}")}),
                );
            }
        }
        let expected = oracle(&ctx);
        assert_eq!(expected[0]["pending"], Value::Null);
        // When the public sync entry runs, it reaches the real announcement preparation boundary.
        let before = work::snapshot();
        corral_ops::sync_at(&ctx, now()).unwrap();
        let counts = work::snapshot().since(&before);
        counts.check_identity().unwrap();
        assert_eq!(counts.unattributed_hydrations(), 0);
        let matching = || {
            ctx.store
                .events_of(None, &["lessons_due"])
                .unwrap()
                .into_iter()
                .filter(|event| {
                    event.detail["project"] == "app" && event.detail["request"] == "T-002"
                })
                .count()
        };
        assert_eq!(matching(), 1);
        assert_eq!(
            serde_json::to_value(corral_ops::lessons_due(&ctx, now()).unwrap()).unwrap(),
            expected
        );
        corral_ops::sync_at(&ctx, now()).unwrap();
        assert_eq!(matching(), 1, "the same generation is announced once");
        samples.push(counts);
    }
    // Then neither same-project history nor other projects' announcements add hot reads.
    same(&samples[0], &samples[1]);
}
