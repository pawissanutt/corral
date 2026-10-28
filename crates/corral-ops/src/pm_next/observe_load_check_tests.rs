use super::*;
use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use corral_core::{Artifact, ArtifactKind, Store};
use serde_json::{Value, json};
use std::sync::Arc;

const NOW: &str = "2026-01-01T00:00:00Z";
const TITLE: &str = "PM isolated test result";

// db316644's complete load_check, including the integrity checks *before* head filtering.
fn full_oracle(ctx: &Ctx, task: &Task, run: &Run, head: &str) -> Result<Option<PmObservation>> {
    for artifact in task
        .artifacts
        .iter()
        .rev()
        .filter(|a| a.run == run.id && a.title == TITLE)
    {
        let path = ctx.store.dir().join(&artifact.path);
        let bytes = crate::artifact_read(ctx, artifact)?;
        if verification::digest(&bytes) != artifact.sha256 {
            bail!("test observation artifact hash changed: {}", path.display());
        }
        let check: LoadCheck = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing {}", path.display()))?;
        if check.version != 1 || check.task != task.id || check.run != run.id || check.head != head
        {
            continue;
        }
        if check.under_load_exit != 0 {
            return Ok(Some(PmObservation::LoadFailure {
                task: task.id.clone(),
                command: check.command,
                alone_passed: check.alone_exit.map(|exit| exit == 0),
                evidence: format!(
                    "{}: load exit {}; alone exit {:?}; log {}",
                    path.display(),
                    check.under_load_exit,
                    check.alone_exit,
                    check.log
                ),
            }));
        }
    }
    Ok(None)
}

fn outcome(result: Result<Option<PmObservation>>) -> Result<Value, String> {
    result
        .map(|observation| serde_json::to_value(observation).unwrap())
        .map_err(|error| format!("{error:#}"))
}

fn fixture() -> (tempfile::TempDir, Ctx, Task, String) {
    let root = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ],
    ] {
        assert!(
            crate::test_support::command("git", root.path())
                .arg("-C")
                .arg(root.path())
                .args(args)
                .stdin(Stdio::null())
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    let store = Store::init(root.path()).unwrap();
    let head = corral_git::rev_sha(root.path(), "HEAD").unwrap();
    let mut task = store.create("Load", None, Status::Review, "").unwrap();
    task.runs.push(
        serde_json::from_value(json!({
            "id":"R1", "agent":"worker", "worktree":root.path(), "branch":"main",
            "base":"main", "base_sha":head, "started":NOW
        }))
        .unwrap(),
    );
    store.fixture_put_task(&task).unwrap();
    let ctx = Ctx {
        store,
        herdr: corral_herdr::Herdr::with_bin(root.path().join("unused-herdr"), None),
        resource_probe: Arc::new(crate::resources::PinnedReadingsProbe::admitting()),
    };
    (root, ctx, task, head)
}

fn check(task: &Task, head: &str, exit: i32, n: usize) -> Value {
    json!({
        "version":1, "task":task.id, "run":"R1", "head":head,
        "command":format!("cargo test report_{n:04}"), "under_load_exit":exit,
        "alone_exit":0, "log":"test.log"
    })
}

fn attach(ctx: &Ctx, task: &Task, value: Value, legacy: bool) -> Artifact {
    let bytes = serde_json::to_vec(&value).unwrap();
    let path = ctx.store.dir().join("state/report.json");
    std::fs::write(&path, &bytes).unwrap();
    if !legacy {
        return crate::add_artifact(
            ctx,
            &task.id,
            &path,
            Some(ArtifactKind::Report),
            Some(TITLE),
        )
        .unwrap();
    }
    let mut task = ctx.store.get(&task.id).unwrap();
    let artifact: Artifact = serde_json::from_value(json!({
        "run":"R1", "kind":"report", "title":TITLE, "source":path,
        "path":format!("artifacts/{}/R1/check-{:04}.json", task.id, task.artifacts.len()),
        "sha256":verification::digest(&bytes), "bytes":bytes.len(), "added":NOW
    }))
    .unwrap();
    let target = ctx.store.dir().join(&artifact.path);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(target, bytes).unwrap();
    task.artifacts.push(artifact.clone());
    ctx.store.fixture_put_task(&task).unwrap();
    artifact
}

fn pin(ctx: &Ctx, id: &str) -> Task {
    let mut task = ctx.store.get(id).unwrap();
    task.created = NOW.parse().unwrap();
    task.updated = NOW.parse().unwrap();
    for artifact in &mut task.artifacts {
        artifact.added = NOW.parse().unwrap();
    }
    ctx.store.fixture_put_task(&task).unwrap();
    // Retention/fixture metadata edits do not alter publication order. Refresh publication
    // metadata using the same deduplication entry point, not a test-only prepare call.
    for (index, artifact) in task.artifacts.iter().enumerate() {
        let source = ctx.store.dir().join("state/report.json");
        std::fs::write(&source, crate::artifact_read(ctx, artifact).unwrap()).unwrap();
        if ctx
            .store
            .record_row(
                load_checks::REPORTS,
                &format!("{id}/{}", corral_core::store::codec::ordinal_key(index + 1)),
            )
            .unwrap()
            .is_some()
        {
            crate::add_artifact(ctx, id, &source, Some(ArtifactKind::Report), Some(TITLE)).unwrap();
        }
    }
    ctx.store.fixture_remove_events().unwrap();
    task
}

fn frozen(ctx: &Ctx) -> Ctx {
    Ctx {
        store: ctx.store.read_only_snapshot().unwrap(),
        ..ctx.clone()
    }
}

fn equal_cells(base: &WorkSnapshot, grown: &WorkSnapshot) {
    for counter in Counter::ALL {
        assert_eq!(base.get(counter), grown.get(counter), "{counter:?}");
    }
    for report in [base, grown] {
        assert_eq!(report.unattributed_hydrations(), 0);
        report.check_identity().unwrap();
    }
}

fn growth(legacy: bool, failure: bool) {
    let mut reports = Vec::new();
    for factor in [1, 10] {
        // Given 1x/10x same-title successes and other-head failures in this very run.
        let (_root, ctx, task, head) = fixture();
        if failure {
            attach(&ctx, &task, check(&task, &head, 101, 0), legacy);
        }
        for n in 1..=4 * factor {
            attach(&ctx, &task, check(&task, &head, 0, n), legacy);
            attach(&ctx, &task, check(&task, &"a".repeat(40), 101, n), legacy);
        }
        let task = pin(&ctx, &task.id);
        let run = task.current_run().unwrap();
        let expected = outcome(full_oracle(&ctx, &task, run, &head));
        assert_eq!(expected.as_ref().unwrap().is_null(), !failure);
        let ctx = frozen(&ctx);
        let before = work::snapshot();
        // When the real PM entry performs cold recovery and then its hot evaluation.
        let workset = crate::pm_workset(&ctx).unwrap();
        let report = work::snapshot().since(&before);
        assert_eq!(
            workset.next.kind == crate::PmActionKind::LoadFailure,
            failure
        );
        assert_eq!(outcome(load_check(&ctx, &task, run, &head)), expected);
        reports.push(report);
    }
    // Then every unattributed cell, not just hydrations, is history independent.
    equal_cells(&reports[0], &reports[1]);
}

#[test]
fn published_same_title_success_and_other_head_growth_is_bounded() {
    growth(false, false);
}

#[test]
fn published_failure_behind_same_title_growth_remains_selected() {
    growth(false, true);
}

#[test]
fn legacy_same_title_success_and_other_head_growth_is_prepared_cold() {
    growth(true, false);
}

#[test]
fn legacy_failure_behind_same_title_growth_remains_selected() {
    growth(true, true);
}

#[test]
fn integrity_errors_and_failure_precedence_equal_full_task_oracle() {
    // Given damaged successes/other heads before or after the matching failure.
    for legacy in [false, true] {
        for damage in [
            "hash",
            "missing",
            "json",
            "unknown-field",
            "directory",
            "purged",
        ] {
            for damaged_newer in [false, true] {
                let (_root, ctx, task, head) = fixture();
                let damaged;
                if damaged_newer {
                    attach(&ctx, &task, check(&task, &head, 1, 0), legacy);
                    damaged = attach(&ctx, &task, check(&task, &"a".repeat(40), 0, 1), legacy);
                } else {
                    damaged = attach(&ctx, &task, check(&task, &"a".repeat(40), 0, 1), legacy);
                    attach(&ctx, &task, check(&task, &head, 1, 0), legacy);
                }
                damage_report(&ctx, &task, &head, &damaged, damage, legacy);
                if !legacy {
                    // Simulate damage present at the publication/ingestion boundary. The
                    // same hook is called by add_artifact while holding these two guards.
                    let _lock = crate::artifact_storage::storage_lock(&ctx).unwrap();
                    ctx.store
                        .update(&task.id, |t| {
                            let index = t
                                .artifacts
                                .iter()
                                .position(|a| a.path == damaged.path)
                                .unwrap();
                            load_checks::published(&ctx, t, index, false)
                        })
                        .unwrap();
                }
                let task = ctx.store.get(&task.id).unwrap();
                let run = task.current_run().unwrap();
                let expected = outcome(full_oracle(&ctx, &task, run, &head));
                assert_eq!(expected.is_err(), damaged_newer, "{damage}");
                let ctx = frozen(&ctx);
                let workset = crate::pm_workset(&ctx).unwrap();
                let before = work::snapshot();
                let actual = {
                    let _hot = HotScope::enter();
                    outcome(load_check(&ctx, &task, run, &head))
                };
                assert_eq!(actual, expected, "{legacy}/{damage}/{damaged_newer}");
                assert_eq!(
                    workset.next.kind == crate::PmActionKind::Owner,
                    damaged_newer
                );
                let report = work::snapshot().since(&before);
                assert_eq!(report.unattributed_hydrations(), 0);
                report.check_identity().unwrap();
            }
        }
    }
}

fn damage_report(
    ctx: &Ctx,
    task: &Task,
    head: &str,
    damaged: &Artifact,
    damage: &str,
    legacy: bool,
) {
    let path = ctx.store.dir().join(&damaged.path);
    match damage {
        "hash" => std::fs::write(&path, b"tampered").unwrap(),
        "missing" => {
            std::fs::remove_file(&path).unwrap();
            if !legacy {
                // A missing mirror alone legitimately falls back to its live blob,
                // even after the last reference is removed, until GC reclaims it.
                let hash = corral_core::blob_ref::BlobHash::parse(&damaged.sha256).unwrap();
                std::fs::remove_file(ctx.store.blob_store().unwrap().path_of(&hash)).unwrap();
            }
        }
        "directory" => {
            std::fs::remove_file(&path).unwrap();
            std::fs::create_dir(&path).unwrap();
        }
        "json" | "unknown-field" => {
            let bytes = if damage == "json" {
                b"{".to_vec()
            } else {
                let mut value = check(task, head, 0, 3);
                value["unexpected"] = true.into();
                serde_json::to_vec(&value).unwrap()
            };
            std::fs::write(&path, &bytes).unwrap();
            ctx.store
                .update(&task.id, |t| {
                    t.artifacts
                        .iter_mut()
                        .find(|a| a.path == damaged.path)
                        .unwrap()
                        .sha256 = verification::digest(&bytes);
                    Ok(())
                })
                .unwrap();
        }
        "purged" => {
            ctx.store
                .update(&task.id, |t| {
                    t.artifacts
                        .iter_mut()
                        .find(|a| a.path == damaged.path)
                        .unwrap()
                        .purged_at = Some(NOW.parse().unwrap());
                    Ok(())
                })
                .unwrap();
        }
        _ => unreachable!(),
    }
}

#[test]
fn snapshots_retain_their_own_first_consumption_after_new_publication() {
    // Given a frozen PM view not yet prepared or consumed.
    let (_root, ctx, task, head) = fixture();
    attach(&ctx, &task, check(&task, &head, 1, 0), false);
    let old = frozen(&ctx);
    attach(&ctx, &task, check(&task, &head, 2, 1), false);
    let fresh = frozen(&ctx);
    crate::pm_workset(&fresh).unwrap();
    // When the older evaluation first prepares after the newer one.
    crate::pm_workset(&old).unwrap();
    // Then neither its metadata nor its cold integrity result advances generations.
    for ctx in [&old, &fresh] {
        let task = ctx.store.get(&task.id).unwrap();
        assert_eq!(
            outcome(load_check(ctx, &task, task.current_run().unwrap(), &head)),
            outcome(full_oracle(ctx, &task, task.current_run().unwrap(), &head))
        );
    }
}

#[test]
fn publication_keeps_newest_matching_failure_across_success_and_older_deduplication() {
    // Given two failures for this head, a later success, and another head's failure.
    let (_root, ctx, task, head) = fixture();
    let first = check(&task, &head, 1, 0);
    attach(&ctx, &task, first.clone(), false);
    attach(&ctx, &task, check(&task, &head, 2, 1), false);
    attach(&ctx, &task, check(&task, &head, 0, 2), false);
    attach(&ctx, &task, check(&task, &"a".repeat(40), 3, 3), false);
    // When the original failure is published again through the normal deduplication path.
    attach(&ctx, &task, first, false);
    let key = verification::digest(&serde_json::to_vec(&(&task.id, "R1", &head)).unwrap());
    let selected: Value = ctx
        .store
        .record(load_checks::FAILURES, &key)
        .unwrap()
        .unwrap();
    // Then the maintained pointer, before any preparation, still names the second failure.
    assert_eq!(selected["row"], "0000000002");
    let workset = crate::pm_workset(&ctx).unwrap();
    assert_eq!(workset.next.kind, crate::PmActionKind::LoadFailure);
    let task = ctx.store.get(&task.id).unwrap();
    let expected = crate::pm_card(
        &full_oracle(&ctx, &task, task.current_run().unwrap(), &head)
            .unwrap()
            .unwrap(),
        "corral",
    );
    assert_eq!(workset.next.evidence[0], expected.evidence[0]);
}

#[test]
fn wrong_identity_and_version_reports_do_not_hide_the_matching_failure() {
    // Given newer, structurally valid reports with wrong attribution/version and all alone states.
    for alone in [Value::Null, json!(0), json!(13)] {
        let (_root, ctx, task, head) = fixture();
        let mut failure = check(&task, &head, 101, 0);
        failure["alone_exit"] = alone;
        attach(&ctx, &task, failure, false);
        for (field, value) in [
            ("version", json!(2)),
            ("task", json!("T-999")),
            ("run", json!("R2")),
        ] {
            let mut report = check(&task, &head, 1, 1);
            report[field] = value;
            attach(&ctx, &task, report, false);
        }
        let task = ctx.store.get(&task.id).unwrap();
        let ctx = frozen(&ctx);
        // When production prepares and selects the current candidate's report.
        crate::pm_workset(&ctx).unwrap();
        // Then output, including the tri-state isolated result, equals the full-task oracle.
        assert_eq!(
            outcome(load_check(&ctx, &task, task.current_run().unwrap(), &head)),
            outcome(full_oracle(&ctx, &task, task.current_run().unwrap(), &head))
        );
    }
}

#[test]
fn missing_mirror_uses_the_live_blob_like_the_full_task_oracle() {
    // Given a published matching failure whose legacy mirror has been reclaimed.
    let (_root, ctx, task, head) = fixture();
    let report = attach(&ctx, &task, check(&task, &head, 1, 0), false);
    std::fs::remove_file(ctx.store.dir().join(&report.path)).unwrap();
    let task = ctx.store.get(&task.id).unwrap();
    let ctx = frozen(&ctx);
    // When production prepares the blob-backed report.
    let workset = crate::pm_workset(&ctx).unwrap();
    // Then the mirror's absence is not a missing-evidence error.
    assert_eq!(workset.next.kind, crate::PmActionKind::LoadFailure);
    assert_eq!(
        outcome(load_check(&ctx, &task, task.current_run().unwrap(), &head)),
        outcome(full_oracle(&ctx, &task, task.current_run().unwrap(), &head))
    );
}

#[test]
fn whole_warm_pm_evaluation_including_preparation_is_history_independent() {
    for legacy in [false, true] {
        for failure in [false, true] {
            let mut reports = Vec::new();
            for factor in [1, 10] {
                // Given either a published checkpoint or a legacy run needing one backfill.
                let (_root, ctx, task, head) = fixture();
                if failure {
                    attach(&ctx, &task, check(&task, &head, 101, 0), legacy);
                }
                for n in 1..=4 * factor {
                    attach(&ctx, &task, check(&task, &head, 0, n), legacy);
                    attach(&ctx, &task, check(&task, &"a".repeat(40), 1, n), legacy);
                }
                pin(&ctx, &task.id);
                crate::pm_workset(&ctx).unwrap();
                // Reopen: no per-snapshot or in-process preparation cache can mask a rescan.
                let warm = Ctx {
                    store: Store::open(ctx.store.root()).unwrap(),
                    ..ctx.clone()
                };
                let before = work::snapshot();
                // When the *entire* second production evaluation, preparation included, is hot.
                let workset = {
                    let _hot = HotScope::enter();
                    crate::pm_workset(&warm).unwrap()
                };
                // Then every unattributed cell, including bytes and stats, stays identical.
                assert_eq!(
                    workset.next.kind == crate::PmActionKind::LoadFailure,
                    failure
                );
                reports.push(work::snapshot().since(&before));
            }
            equal_cells(&reports[0], &reports[1]);
        }
    }
}

#[test]
fn selected_report_integrity_is_rechecked_after_publication() {
    for damage in [
        "hash",
        "missing",
        "json",
        "unknown-field",
        "directory",
        "purged",
    ] {
        // Given a selected report damaged after its successful publication.
        let (_root, ctx, task, head) = fixture();
        let selected = attach(&ctx, &task, check(&task, &head, 101, 0), false);
        damage_report(&ctx, &task, &head, &selected, damage, false);
        let task = ctx.store.get(&task.id).unwrap();
        let ctx = frozen(&ctx);
        // When it is selected by the real PM entry.
        crate::pm_workset(&ctx).unwrap();
        // Then publication metadata cannot waive the original byte-level diagnostics.
        assert_eq!(
            outcome(load_check(&ctx, &task, task.current_run().unwrap(), &head)),
            outcome(full_oracle(&ctx, &task, task.current_run().unwrap(), &head)),
            "{damage}"
        );
    }
}

#[test]
fn skipped_report_corruption_after_publication_is_not_rescanned() {
    // Given a valid failure and a newer, successfully ingested skipped report.
    let (_root, ctx, task, head) = fixture();
    attach(&ctx, &task, check(&task, &head, 101, 0), false);
    let skipped = attach(&ctx, &task, check(&task, &"a".repeat(40), 0, 1), false);
    std::fs::write(ctx.store.dir().join(&skipped.path), b"out of band").unwrap();
    let task = ctx.store.get(&task.id).unwrap();
    // When a later evaluation reads the maintained selection.
    let workset = crate::pm_workset(&ctx).unwrap();
    // Then only selected bytes are rechecked: this is the intentional semantic boundary.
    assert_eq!(workset.next.kind, crate::PmActionKind::LoadFailure);
    assert!(full_oracle(&ctx, &task, task.current_run().unwrap(), &head).is_err());
}

#[test]
fn legacy_backfill_generations_are_idempotent_and_do_not_advance_old_snapshots() {
    // Given two older, unconsumed snapshots and a newer legacy append.
    let (_root, ctx, task, head) = fixture();
    attach(&ctx, &task, check(&task, &head, 1, 0), true);
    let skipped = attach(&ctx, &task, check(&task, &"a".repeat(40), 0, 1), true);
    let old = frozen(&ctx);
    let old_again = frozen(&ctx);
    attach(&ctx, &task, check(&task, &head, 2, 2), true);
    let fresh = frozen(&ctx);
    crate::pm_workset(&fresh).unwrap();
    crate::pm_workset(&old).unwrap();
    let old_task = old.store.get(&task.id).unwrap();
    let expected = outcome(load_check(
        &old,
        &old_task,
        old_task.current_run().unwrap(),
        &head,
    ));
    std::fs::write(ctx.store.dir().join(&skipped.path), b"after backfill").unwrap();
    // When another snapshot of the same old revision first prepares.
    crate::pm_workset(&old_again).unwrap();
    // Then it reuses its immutable generation, not newer shared pointers or report bytes.
    assert_eq!(
        outcome(load_check(
            &old_again,
            &old_task,
            old_task.current_run().unwrap(),
            &head
        )),
        expected
    );
    let fresh_task = fresh.store.get(&task.id).unwrap();
    let fresh_result = outcome(load_check(
        &fresh,
        &fresh_task,
        fresh_task.current_run().unwrap(),
        &head,
    ));
    assert_ne!(fresh_result, expected);
    assert_eq!(
        fresh_result,
        outcome(full_oracle(
            &fresh,
            &fresh_task,
            fresh_task.current_run().unwrap(),
            &head
        ))
    );
}
