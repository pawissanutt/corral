use super::*;

fn artifact(ctx: &Ctx, index: usize, kind: ArtifactKind, added: i64) -> Artifact {
    let path = format!("artifacts/report-{index:04}");
    std::fs::create_dir_all(ctx.store.dir().join("artifacts")).unwrap();
    let text = format!("Not verified: unique-network-probe-{index:04}\n");
    std::fs::write(ctx.store.dir().join(&path), &text).unwrap();
    Artifact {
        run: "R1".into(),
        kind,
        title: "Report".into(),
        path,
        source: "/unused".into(),
        sha256: String::new(),
        bytes: u64::try_from(text.len()).unwrap(),
        added: ts(added),
        tier_override: None,
        compressed_bytes: None,
        purged_at: None,
    }
}

#[test]
fn appended_plan_artifacts_do_not_evict_eligible_lesson_evidence() {
    // Given a Done task with one eligible Report.
    let (_dir, ctx) = fixture();
    let mut task = ctx
        .store
        .create("Done", Some("app"), Status::Done, "")
        .unwrap();
    task.artifacts
        .push(artifact(&ctx, 0, ArtifactKind::Report, 0));
    ctx.store.fixture_put_task(&task).unwrap();
    let expected = crate::lesson_signals(&ctx, "app").unwrap();
    assert!(
        expected
            .iter()
            .any(|signal| signal.key == "not_verified:unique-network-probe-0000")
    );
    // When 32 newer excluded Plans are appended.
    task.artifacts
        .extend((1..=32).map(|i| artifact(&ctx, i, ArtifactKind::Plan, i64::try_from(i).unwrap())));
    ctx.store.fixture_put_task(&task).unwrap();
    // Then selection and the emitted bytes match the full-task baseline.
    let actual = crate::lesson_signals(&ctx, "app").unwrap();
    assert_eq!(actual, expected);
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(&legacy(&ctx, "app")).unwrap()
    );
}

#[test]
fn mixed_kinds_and_nonappend_timestamps_match_baseline_lesson_ordering() {
    // Given more than 32 eligible artifacts in mixed kinds and reversed timestamp order.
    let (_dir, ctx) = fixture();
    let mut task = ctx
        .store
        .create("Mixed", Some("app"), Status::Done, "")
        .unwrap();
    let kinds = [
        ArtifactKind::Log,
        ArtifactKind::Summary,
        ArtifactKind::Report,
        ArtifactKind::Diff,
        ArtifactKind::Evidence,
        ArtifactKind::Plan,
    ];
    for i in 0..80 {
        // Repeated timestamps also exercise the descending path tie breaker.
        task.artifacts.push(artifact(
            &ctx,
            i,
            kinds[i % kinds.len()],
            40 - i64::try_from(i / 2).unwrap(),
        ));
    }
    ctx.store.fixture_put_task(&task).unwrap();
    // When the production reader acquires its bounded source set.
    let actual = crate::lesson_signals(&ctx, "app").unwrap();
    // Then both exact serialized signals and source membership equal the full-task oracle.
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(&legacy(&ctx, "app")).unwrap()
    );
    let mut expected: Vec<_> = task
        .artifacts
        .iter()
        .filter(|a| a.kind != ArtifactKind::Plan)
        .cloned()
        .collect();
    expected.sort_by(|a, b| (b.added, &b.path).cmp(&(a.added, &a.path)));
    expected.truncate(32);
    let selected = source(&ctx, &ctx.store.head(&task.id).unwrap()).unwrap();
    assert_eq!(selected.task.artifacts, expected);
}

#[test]
fn eligible_artifact_acquisition_and_production_signals_ignore_tenfold_history_growth() {
    // Given 32 retained eligible artifacts after older eligible and excluded history.
    let (_dir, ctx) = fixture();
    let mut task = ctx
        .store
        .create("Artifacts", Some("app"), Status::Running, "")
        .unwrap();
    task.runs.extend((0..32).map(|i| run(i + 1000, "")));
    task.artifacts
        .extend((0..32).map(|i| artifact(&ctx, i, ArtifactKind::Report, 0)));
    task.artifacts
        .extend((1000..1032).map(|i| artifact(&ctx, i, ArtifactKind::Evidence, 1000)));
    task.artifacts
        .extend((2000..2032).map(|i| artifact(&ctx, i, ArtifactKind::Plan, 2000)));
    ctx.store.fixture_put_task(&task).unwrap();
    let acquire = || {
        let head = ctx.store.head(&task.id).unwrap();
        let _hot = work::HotScope::enter();
        measure(|| source(&ctx, &head).unwrap().task.artifacts)
    };
    let (selected, counts) = acquire();
    let (signals, production_counts) = measure(|| crate::lesson_signals(&ctx, "app").unwrap());
    assert_eq!(signals, legacy(&ctx, "app"));
    // When both excluded and older eligible same-run history grow tenfold, with noise elsewhere.
    task.artifacts
        .extend((32..320).map(|i| artifact(&ctx, i, ArtifactKind::Log, 0)));
    task.artifacts
        .extend((2032..2320).map(|i| artifact(&ctx, i, ArtifactKind::Plan, 2000)));
    task.runs.splice(0..0, (0..288).map(|i| run(i + 2000, "")));
    ctx.store.fixture_put_task(&task).unwrap();
    for i in 0..10 {
        let mut old = ctx
            .store
            .create("Other terminal", Some("other"), Status::Done, "")
            .unwrap();
        old.runs.extend((0..10).map(|j| run(j + 1, "")));
        ctx.store.fixture_put_task(&old).unwrap();
        ctx.store
            .log_event(None, "unrelated", json!({"i":i}))
            .unwrap();
    }
    let (grown, grown_counts) = acquire();
    let (grown_signals, grown_production_counts) =
        measure(|| crate::lesson_signals(&ctx, "app").unwrap());
    // Then only the same 32 eligible documents are acquired and public output is byte-identical.
    assert_eq!(selected, grown);
    assert_eq!(counts.get(Counter::HistoryRowDecodes), 64); // 32 runs and 32 artifacts.
    same_cells(&counts, &grown_counts);
    same_cells(&production_counts, &grown_production_counts);
    assert_eq!(
        serde_json::to_vec(&signals).unwrap(),
        serde_json::to_vec(&grown_signals).unwrap()
    );
    assert_eq!(grown_signals, legacy(&ctx, "app"));
}
