use super::*;

fn zero_usage_run(id: usize) -> Run {
    let mut run = super::run(id, "");
    run.usage = None;
    run
}

fn registration(kind: ArtifactKind, path: String, added: i64) -> Artifact {
    Artifact {
        run: "R1".into(),
        kind,
        title: "Report".into(),
        path,
        source: "/unused".into(),
        sha256: String::new(),
        bytes: 0,
        added: ts(added),
        tier_override: None,
        compressed_bytes: None,
        purged_at: None,
    }
}

fn append(ctx: &Ctx, task: Option<&str>, kind: &str, detail: serde_json::Value) {
    ctx.store
        .fixture_append_events(&format!(
            "{}\n",
            json!({"ts":ts(0),"task":task,"kind":kind,"detail":detail})
        ))
        .unwrap();
}

fn history_fixture(scale: usize) -> (tempfile::TempDir, Ctx, Task) {
    let (dir, ctx) = fixture();
    // Creation stamps status_since too; later created/updated resets do not replace it.
    ctx.store.fixture_pin_now(ts(0)).unwrap();
    let mut task = ctx
        .store
        .create("Current", Some("app"), Status::Running, "")
        .unwrap();
    task.created = ts(0);
    task.updated = ts(100);
    assert_eq!(task.status_since, Some(ts(0)));
    task.runs
        .extend((0..32 * scale).map(|i| zero_usage_run(i + 1000)));
    task.runs.extend((0..31).map(|i| zero_usage_run(i + 3000)));
    task.runs.push(zero_usage_run(1));
    for i in 0..32 * scale {
        task.artifacts.push(registration(
            ArtifactKind::Log,
            format!("artifacts/old-{i:04}"),
            0,
        ));
    }
    std::fs::create_dir_all(ctx.store.dir().join("artifacts")).unwrap();
    for i in 0..32 {
        let path = format!("artifacts/retained-{i:04}");
        std::fs::write(ctx.store.dir().join(&path), "Not verified: network\n").unwrap();
        task.artifacts.push(registration(
            ArtifactKind::Report,
            path,
            1000 + i64::from(i),
        ));
    }
    for i in 0..32 * scale {
        task.artifacts.push(registration(
            ArtifactKind::Plan,
            format!("artifacts/plan-{i:04}"),
            2000,
        ));
    }
    ctx.store.fixture_put_task(&task).unwrap();
    for i in 0..2 * scale {
        let mut old = ctx
            .store
            .create("Other terminal", Some("other"), Status::Done, "")
            .unwrap();
        old.created = ts(0);
        old.updated = ts(0);
        old.runs
            .extend((0..2 * scale).map(|j| zero_usage_run(j + 1)));
        ctx.store.fixture_put_task(&old).unwrap();
        append(
            &ctx,
            Some(&old.id),
            "approved",
            json!({"sha":format!("other-{i:04}"),"run":"R1","strategy":"rebase"}),
        );
    }
    // Keep sequence-number byte widths equal; the growing history precedes a fixed
    // baseline observation window, just as the growing runs/artifacts precede their caps.
    let line = format!("{}\n", json!({"ts":ts(0),"kind":"unrelated","detail":{}}));
    ctx.store
        .fixture_append_events(&line.repeat(1024 + 32 * scale))
        .unwrap();
    append(
        &ctx,
        None,
        "lessons_distill_requested",
        json!({"project":"app"}),
    );
    ctx.store
        .fixture_append_events(&format!("{}\n", "x".repeat(EVENT_BYTES + 64)))
        .unwrap();
    append(
        &ctx,
        Some(&task.id),
        "gate",
        json!({"result":"fail","run":"R1"}),
    );
    (dir, ctx, task)
}

fn evaluation(ctx: &Ctx) -> serde_json::Value {
    // This is the production PM/board preparation boundary and public readers, with a
    // fresh view on every call. No test-only preparation stands outside measurement.
    let snapshot = Ctx {
        store: ctx.store.read_only_snapshot().unwrap(),
        ..ctx.clone()
    };
    prepare_evaluation(&snapshot).unwrap();
    json!({
        "signals":crate::lesson_signals(&snapshot, "app").unwrap(),
        "due":crate::lessons_due(&snapshot, ts(3600)).unwrap()
    })
}

fn whole_call(ctx: &Ctx) -> (serde_json::Value, WorkSnapshot) {
    let _whole = work::HotScope::enter();
    measure(|| evaluation(ctx))
}

fn assert_oracle(ctx: &Ctx, value: &serde_json::Value) {
    assert_eq!(
        value["signals"],
        serde_json::to_value(legacy(ctx, "app")).unwrap()
    );
    assert_eq!(
        value["due"],
        json!([{
            "project":"app","reasons":["1 failure signals since last request"],
            "profile":null,"pending":null
        }])
    );
}

#[test]
fn warm_second_evaluation_counts_preparation_without_history_sized_work() {
    // Given 1x/10x terminal tasks, runs, same-run artifacts, old events and approvals.
    let (_base_dir, base, _) = history_fixture(1);
    let (_grown_dir, grown, _) = history_fixture(10);
    evaluation(&base);
    evaluation(&grown);
    // When fresh second evaluations include all preparation inside the outer HotScope.
    let (expected, counts) = whole_call(&base);
    let (actual, grown_counts) = whole_call(&grown);
    // Then the complete call, not merely its post-preparation reader, has equal work.
    assert_oracle(&base, &expected);
    assert_oracle(&grown, &actual);
    assert_eq!(expected, actual);
    assert!(
        counts.get(Counter::RecordDecodes) >= 3,
        "preparation must be counted"
    );
    assert_eq!(counts.get(Counter::ViewsOpened), 1);
    assert_eq!(counts.get(Counter::HistoryRowDecodes), 0);
    assert_eq!(counts.get(Counter::EventDecodes), 0);
    same_cells(&counts, &grown_counts);
}

#[test]
fn warm_changed_evaluation_counts_incremental_preparation_without_history_growth() {
    // Given initialized derivations at both history sizes, without sharing a pass cache.
    let (_base_dir, base, mut task) = history_fixture(1);
    let (_grown_dir, grown, mut grown_task) = history_fixture(10);
    evaluation(&base);
    evaluation(&grown);
    let before = recomputations(&base).unwrap();
    let grown_before = recomputations(&grown).unwrap();
    // When one current run changes before the measured second evaluation.
    for (ctx, task) in [(&base, &mut task), (&grown, &mut grown_task)] {
        task.runs.last_mut().unwrap().summary = Some("Updated current run".into());
        task.updated = ts(200);
        ctx.store.fixture_put_task(task).unwrap();
    }
    let (expected, counts) = whole_call(&base);
    let (actual, grown_counts) = whole_call(&grown);
    // Then preparation follows that change, acquiring only 32 runs and eligible artifacts.
    assert_oracle(&base, &expected);
    assert_oracle(&grown, &actual);
    assert_eq!(expected, actual);
    assert_eq!(recomputations(&base).unwrap(), before + 1);
    assert_eq!(recomputations(&grown).unwrap(), grown_before + 1);
    assert!(counts.get(Counter::RecordDecodes) > 3);
    assert_eq!(counts.get(Counter::HistoryRowDecodes), 64);
    assert_eq!(counts.get(Counter::ViewsOpened), 1);
    same_cells(&counts, &grown_counts);
}
