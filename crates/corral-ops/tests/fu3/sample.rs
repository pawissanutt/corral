use super::fixture::*;
use corral_core::work::{Counter, WorkSnapshot};
use corral_ops::resources::{self, PinnedReadingsProbe, ResourceProbe};
use std::sync::Arc;

#[test]
fn cargo_target_paths_remain_hot_in_fresh_processes() {
    // Given absolute and relative target directories, without mutating this process's
    // environment while other integration tests are running.
    let root = tempfile::tempdir().unwrap();
    for target in [
        root.path().join("absolute-target/deep"),
        std::path::PathBuf::from("relative-target/deep"),
    ] {
        // When a fresh process runs the complete 1x/10x resource-walk matrix.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .current_dir(root.path())
            .env("CARGO_TARGET_DIR", target)
            .args([
                "--exact",
                "fu3::sample::resource_walk_accounts_exactly_for_absent_and_existing_history",
                "--nocapture",
            ])
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        // Then its path-set and hot/FU-3 accounting assertions must all pass.
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line == "running 1 test"),
            "the subprocess must run the exact resource-walk test"
        );
    }
}

#[test]
fn resource_walk_accounts_exactly_for_absent_and_existing_history() {
    for existing in [false, true] {
        for bound in [false, true] {
            let variants = [1, 10].map(|scale| resource_variant(scale, existing, bound));
            assert_growth(&variants[0], &variants[1]);
        }
    }
}

pub(super) fn resource_variant(scale: usize, existing: bool, bound: bool) -> WorkSnapshot {
    // Given a fresh in-process store with valid defaults and pinned readings.
    let mut fixture = Fixture::new(scale, false, existing);
    if bound {
        // Observe writes its durable record; only read_admission may use a snapshot.
        let store = fixture.ctx.store.read_only_snapshot().unwrap();
        fixture.ctx.store = store;
    }
    // When the public sync-tick observation or PM admission evaluation runs.
    let (state, cells) = if bound {
        measure(|| resources::read_status(&fixture.ctx).unwrap())
    } else {
        measure(|| resources::observe(&fixture.ctx, NOW.parse().unwrap()).unwrap())
    };
    // Then preconditions must hold before accounting is inspected.
    fixture.state_preconditions(&state);
    assert_eq!(fixture.probe.samples(), 1);
    assert_sample(&cells, &fixture, 1, bound);
    assert_eq!(cells.unattributed_hydrations(), 0);
    assert_eq!(
        cells.get(Counter::FileStats),
        fixture
            .fixed_paths
            .iter()
            .map(|path| path_stats(path))
            .sum::<u64>()
    );
    cells
}

#[test]
fn read_admission_runs_the_whole_walk_on_both_store_variants() {
    for existing in [false, true] {
        let variants = [1, 10].map(|scale| {
            let fixture = Fixture::new(scale, false, existing);
            // Both evaluations start with the same absent persisted latch. Do the read
            // before observe writes a host-dependent PSI observation into that latch.
            let ((state, admission), cells) = measure(|| {
                let admission = resources::read_admission(&fixture.ctx).unwrap();
                let state = resources::observe(&fixture.ctx, NOW.parse().unwrap()).unwrap();
                (state, admission)
            });
            // Then normal admission remains open, with one full walk per evaluation.
            fixture.state_preconditions(&state);
            assert!(admission.is_none());
            assert_eq!(fixture.probe.samples(), 2);
            assert_sample(&cells, &fixture, 2, false);
            cells
        });
        assert_growth(&variants[0], &variants[1]);
    }
}

#[test]
fn high_pinned_load_reaches_the_original_admission_evaluator() {
    // Given the same real inventory and valid default maximum load.
    let mut fixture = Fixture::new(10, false, true);
    let cores = std::thread::available_parallelism().unwrap().get();
    let cores = u32::try_from(cores).unwrap();
    fixture.probe = Arc::new(PinnedReadingsProbe::new(
        2.0 * f64::from(cores) * fixture.ctx.store.config.resources.max_load_per_core,
        u64::MAX,
        u64::MAX,
    ));
    fixture.ctx.resource_probe = fixture.probe.clone();
    // When the public read-only admission entry evaluates the pinned busy machine.
    let (admission, cells) = measure(|| resources::read_admission(&fixture.ctx).unwrap());
    // Then db316644's exact refusal text survives and the full walk still ran.
    assert_eq!(fixture.probe.samples(), 1);
    assert_eq!(
        admission,
        Some(format!(
            "waiting: machine busy (load/core 2.00, max 1.00, load1 {:.2}, cores {cores})",
            2.0 * f64::from(cores),
        ))
    );
    assert_sample(&cells, &fixture, 1, false);
    assert_eq!(cells.unattributed_hydrations(), 0);
    // The probe pins readings, not thresholds or volume layout.
    let snapshot = fixture.probe.sample(&fixture.ctx.store).unwrap();
    assert_eq!(
        snapshot
            .disks
            .iter()
            .map(|d| d.path.clone())
            .collect::<std::collections::BTreeSet<_>>(),
        fixture.paths
    );
}

#[test]
fn walk_paths_already_contributed_by_fixed_inputs_stay_unattributed() {
    // Given a coding run whose worktree is also the fixed store path.
    let tmp = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(tmp.path()).unwrap();
    let task = store
        .create("Volume collision", None, corral_core::Status::Done, "")
        .unwrap();
    store
        .update(&task.id, |t| {
            t.runs.push(run("R1".into(), store.dir().into()));
            Ok(())
        })
        .unwrap();
    let fixed = fixed_paths(&store);
    let probe = Arc::new(PinnedReadingsProbe::admitting());
    let ctx = corral_ops::Ctx {
        resource_probe: probe.clone(),
        ..corral_ops::Ctx::from_store(store)
    };
    // When the real sampler deduplicates paths from both input classes.
    let (state, cells) = measure(|| resources::read_status(&ctx).unwrap());
    // Then the store's probe is hot work, while only its target belongs to the walk.
    assert!(!state.held && state.reason.is_none());
    let mut expected = fixed.clone();
    expected.insert(ctx.store.dir().join("target"));
    assert_eq!(
        state
            .snapshot
            .disks
            .iter()
            .map(|d| d.path.clone())
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );
    assert_eq!(probe.samples(), 1);
    assert_eq!(cells.followup_calls(FU, "sample"), 1);
    assert_eq!(
        cells.followup_stats(FU, "sample"),
        1 + expected
            .difference(&fixed)
            .map(|path| path_stats(path))
            .sum::<u64>()
    );
    assert_eq!(
        cells.get(Counter::FileStats),
        fixed.iter().map(|path| path_stats(path)).sum::<u64>()
    );
}

#[test]
fn scoped_pm_next_reaches_workset_read_admission() {
    for existing in [false, true] {
        let variants = [1, 10].map(|scale| workset_variant(scale, existing));
        assert_growth(&variants[0], &variants[1]);
    }
}

pub(super) fn workset_variant(scale: usize, existing: bool) -> WorkSnapshot {
    // Given a scoped backlog task without dispatch authority.
    let mut fixture = Fixture::new(scale, false, existing);
    fixture
        .ctx
        .store
        .update(&fixture.idea.id, |task| {
            task.status = corral_core::Status::Backlog;
            Ok(())
        })
        .unwrap();
    let selection = corral_core::OperatingSelection {
        tasks: [fixture.idea.id.clone()].into(),
        ..Default::default()
    };
    let preview = fixture
        .ctx
        .store
        .preview_operating_policy(&selection)
        .unwrap();
    fixture.ctx.store.config = fixture
        .ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", NOW.parse().unwrap())
        .unwrap();
    // Disable the independent free_slots sample, not resource admission.
    fixture.ctx.store.config.dispatch.paused = true;
    fixture.ctx.store.save_config().unwrap();
    pin_task_times(&fixture.ctx.store);
    fixture.tasks = fixture.ctx.store.list().unwrap();
    // When the ordinary PM workset explains its scoped blocked task.
    let ((state, workset), cells) = measure_evaluation(|| {
        // Admission sees an absent latch at both sizes, not observe's host-dependent
        // PSI and growing volume inventory (as in the direct admission case above).
        let workset = corral_ops::pm_workset(&fixture.ctx).unwrap();
        let (state, _) =
            measure(|| resources::observe(&fixture.ctx, NOW.parse().unwrap()).unwrap());
        (state, workset)
    });
    // Then the scoped-workset branch ran and did not take an action.
    fixture.state_preconditions(&state);
    assert!(workset.actions.is_empty());
    assert!(
        workset
            .waits
            .iter()
            .any(|a| a.task.as_deref() == Some(&fixture.idea.id))
    );
    assert_eq!(fixture.probe.samples(), 2);
    assert_eq!(cells.followup_calls(FU, "sample"), 2);
    assert_eq!(cells.followup_hydrations(FU, "sample"), 0);
    assert_eq!(cells.followup_rows(FU, "sample"), 0);
    assert_eq!(cells.followup_doc_bytes(FU, "sample"), 0);
    assert_eq!(cells.followup_views(FU, "sample"), 1);
    assert_eq!(
        cells.followup_stats(FU, "sample"),
        2 * fixture.sample_stats()
    );
    assert_eq!(cells.followup_processes(FU, "sample"), 0);
    cells
}

#[cfg(unix)]
#[test]
fn busy_sync_walks_but_does_not_promote_an_authorized_idea() {
    use std::os::unix::fs::PermissionsExt;
    // Given an otherwise eligible, owner-authorized automatic promotion and a private fake engine.
    let mut fixture = Fixture::new(10, false, true);
    fixture
        .ctx
        .store
        .update(&fixture.active.id, |t| {
            t.status = corral_core::Status::Done;
            Ok(())
        })
        .unwrap();
    fixture.ctx.store.config.dispatch.promote_ideas = corral_core::PromoteIdeas::Auto;
    fixture.ctx.store.save_config().unwrap();
    let selection = corral_core::OperatingSelection {
        tasks: [fixture.idea.id.clone()].into(),
        authority: corral_core::OperatingAuthority {
            dispatch: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let preview = fixture
        .ctx
        .store
        .preview_operating_policy(&selection)
        .unwrap();
    fixture.ctx.store.config = fixture
        .ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", NOW.parse().unwrap())
        .unwrap();
    fixture.ctx.store.save_config().unwrap();
    let engine = fixture.ctx.store.root().join("fake-herdr");
    std::fs::write(&engine, r#"#!/bin/sh
if [ "$1" = "--session" ]; then shift 2; fi
printf '%s\n' "$*" >> "$0.calls"
case "$1 $2" in
  "workspace list") printf '%s\n' '{"result":{"workspaces":[]}}' ;;
  "pane list") printf '%s\n' '{"result":{"panes":[{"pane_id":"fixture:p1","workspace_id":"fixture","tab_id":"fixture:t1"}]}}' ;;
  "agent list") printf '%s\n' '{"result":{"agents":[]}}' ;;
  *) exit 19 ;;
esac
"#).unwrap();
    std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o755)).unwrap();
    fixture.ctx.herdr =
        corral_herdr::Herdr::with_bin(engine.clone(), Some("private-fixture".into()));
    assert!(fixture.ctx.herdr.available());
    assert_eq!(
        corral_ops::promotion_shortlist(&fixture.ctx).unwrap().len(),
        1
    );
    let policy = fixture.ctx.store.effective_operating_policy().unwrap();
    assert!(
        policy.allows(&fixture.idea.id, corral_core::OperatingActionKind::Dispatch),
        "{policy:?}"
    );
    let cores = u32::try_from(std::thread::available_parallelism().unwrap().get()).unwrap();
    fixture.probe = Arc::new(PinnedReadingsProbe::new(
        2.0 * f64::from(cores) * fixture.ctx.store.config.resources.max_load_per_core,
        u64::MAX,
        u64::MAX,
    ));
    fixture.ctx.resource_probe = fixture.probe.clone();
    fixture.tasks = fixture.ctx.store.list().unwrap();
    // When the real sync tick reaches automatic admission with pinned CPU pressure.
    let (changes, cells) =
        measure(|| corral_ops::sync_at(&fixture.ctx, NOW.parse().unwrap()).unwrap());
    // Then the normal nonempty-inventory path ran, but neither promotion nor launch happened.
    let calls = std::fs::read_to_string(engine.with_extension("calls")).unwrap();
    assert!(calls.lines().any(|line| line == "pane list"));
    assert!(!calls.lines().any(|line| line.starts_with("pane run ")));
    assert!(changes.is_empty(), "{changes:?}");
    let idea = fixture.ctx.store.get(&fixture.idea.id).unwrap();
    assert_eq!(idea.status, corral_core::Status::Idea);
    assert!(idea.runs.is_empty() && idea.launch.is_none());
    assert!(
        fixture
            .ctx
            .store
            .events_of(None, &["dispatch_policy"])
            .unwrap()
            .is_empty()
    );
    let state = corral_core::records::RecordArea::<corral_core::ResourceState>::new("resources")
        .load(&fixture.ctx.store, "state")
        .unwrap()
        .unwrap();
    assert!(state.held && !state.severe);
    assert_eq!(state.snapshot.load_per_core, 2.0);
    assert_eq!(
        state
            .snapshot
            .disks
            .iter()
            .map(|d| d.path.clone())
            .collect::<std::collections::BTreeSet<_>>(),
        fixture.paths
    );
    assert_eq!(
        state.reason,
        Some(format!(
            "load/core 2.00, max 1.00, load1 {:.2}, cores {cores}",
            2.0 * f64::from(cores)
        ))
    );
    assert_eq!(fixture.probe.samples(), 2);
    assert_sample(&cells, &fixture, 2, false);
}
