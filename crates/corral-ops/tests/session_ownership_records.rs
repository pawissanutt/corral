//! The bounded reader must agree with the former task-local ownership walk.

use corral_core::work::{self, Counter, HotScope, Work};
use corral_core::{Run, Status, Store, Task};
use corral_ops::{Ctx, resources};
use serde_json::{Value, json};

#[path = "../src/session_scope.rs"]
mod session_scope;
#[path = "../../corral-core/tests/store_support/mod.rs"]
mod store_support;

fn context(root: &std::path::Path) -> Ctx {
    Ctx {
        store: Store::init(root).unwrap(),
        herdr: corral_herdr::Herdr::with_bin(root.join("unused-herdr"), Some("t323".into())),
        resource_probe: resources::FixedResourceProbe::arc(),
    }
}

fn reference(socket: Option<&std::path::Path>) -> Value {
    json!({"workspace_id": "w1", "pane_id": "w1:p1", "socket": socket})
}

fn fixture(ctx: &Ctx, kind: &str, socket: Option<&std::path::Path>, copies: usize) -> Task {
    let mut task = ctx.store.create(kind, None, Status::Running, "").unwrap();
    let mut run = json!({
        "id":"R1", "agent":"worker", "worktree":ctx.store.root(),
        "branch":"main", "base":"main", "base_sha":"fixture",
        "started":"2026-01-01T00:00:00Z"
    });
    let gate = json!({"result":"running", "sha":"fixture", "pane":"w1:p1",
        "socket":socket, "at":"2026-01-01T00:00:00Z"});
    match kind {
        "worker" => run["herdr"] = reference(socket),
        "review-child" => {
            let mut child = run.clone();
            child["id"] = json!("R1-review");
            child["herdr"] = reference(socket);
            run["review_runs"] = json!([child]);
        }
        "child-gate" => {
            let mut child = run.clone();
            child["id"] = json!("R1-review");
            child["gate"] = gate;
            run["review_runs"] = json!([child]);
        }
        "gate" => run["gate"] = gate,
        "legacy-gate-with-worker" => {
            run["herdr"] = reference(Some(&ctx.herdr.socket_path().unwrap()));
            run["gate"] = gate;
        }
        "local-gate" => {
            run["gate"] = gate;
            run["gate"]["pane"] = Value::Null;
        }
        "planner" => {
            let mut planner = store_support::plan_run("P1", true);
            planner["herdr"] = reference(socket);
            task.plan_runs
                .push(serde_json::from_value(planner).unwrap());
        }
        "plan-review" => {
            let mut plan = store_support::plan(&task.id, "P1", copies);
            for review in plan["reviews"].as_array_mut().unwrap() {
                review["herdr"] = reference(socket);
            }
            task.plans.push(serde_json::from_value(plan).unwrap());
        }
        "aggregate-review" => {
            let mut attempt = store_support::attempt(&task.id, "A1", copies);
            for review in attempt["reviews"].as_array_mut().unwrap() {
                review["herdr"] = reference(socket);
            }
            task.aggregate_attempts
                .push(serde_json::from_value(attempt).unwrap());
        }
        "launch" => {
            task.launch = Some(
                serde_json::from_value(json!({
                    "socket": socket, "stage": "reserved", "reserved": true
                }))
                .unwrap(),
            );
        }
        _ => panic!("unknown fixture {kind}"),
    }
    if matches!(
        kind,
        "worker"
            | "review-child"
            | "child-gate"
            | "gate"
            | "local-gate"
            | "legacy-gate-with-worker"
    ) {
        let run: Run = serde_json::from_value(run).unwrap();
        for i in 0..copies {
            let mut next = run.clone();
            next.id = format!("R{}", i + 1);
            task.runs.push(next);
        }
    }
    ctx.store.fixture_put_task(&task).unwrap();
    task
}

#[test]
fn summary_matches_full_walk_for_every_reference_family() {
    for kind in [
        "worker",
        "review-child",
        "child-gate",
        "gate",
        "planner",
        "plan-review",
        "aggregate-review",
        "launch",
        "local-gate",
        "legacy-gate-with-worker",
    ] {
        for owner in ["saved", "legacy", "foreign"] {
            // Given nested, historical and current references of one ownership family.
            let temp = tempfile::tempdir().unwrap();
            let ctx = context(temp.path());
            let actual = ctx.herdr.socket_path().unwrap();
            let foreign = temp.path().join("foreign.sock");
            let socket = match owner {
                "saved" => Some(actual.as_path()),
                "legacy" => None,
                "foreign" => Some(foreign.as_path()),
                _ => unreachable!(),
            };
            fixture(&ctx, kind, socket, 3);
            fixture(&ctx, "worker", Some(&actual), 1);
            let tasks = ctx.store.list().unwrap();
            let expected = tasks
                .iter()
                .all(|task| session_scope::owns_task(&ctx, task).unwrap());

            // When the bounded inventory and multi-task consumer evaluate the same state.
            let inventory = session_scope::owns_inventory(&ctx).unwrap();
            let consumed = session_scope::owns_tasks(&ctx, &ctx.store, &tasks).unwrap();

            // Then both agree with the full walk, including unknown legacy gate ownership.
            assert_eq!(inventory, expected, "{kind}/{owner}");
            assert_eq!(consumed, expected, "{kind}/{owner}");
            assert_eq!(
                session_scope::check_launch(&ctx).is_ok(),
                expected,
                "{kind}/{owner}"
            );
        }
    }
}

#[test]
fn task_local_check_does_not_inherit_an_unrelated_foreign_owner() {
    // Given a mixed board with one local and one foreign task.
    let temp = tempfile::tempdir().unwrap();
    let ctx = context(temp.path());
    let local = fixture(&ctx, "worker", Some(&ctx.herdr.socket_path().unwrap()), 1);
    fixture(&ctx, "worker", Some(&temp.path().join("foreign.sock")), 1);

    // When checking only the local task at its mutation boundary.
    let owned = session_scope::owns_tasks(&ctx, &ctx.store, &[local]).unwrap();

    // Then task-local semantics remain local even though board inventory is refused.
    assert!(owned);
    assert!(!session_scope::owns_inventory(&ctx).unwrap());
}

#[test]
fn explicit_binding_resolves_legacy_references_in_every_container() {
    for kind in [
        "worker",
        "review-child",
        "child-gate",
        "gate",
        "planner",
        "plan-review",
        "aggregate-review",
        "launch",
        "legacy-gate-with-worker",
    ] {
        // Given unknown legacy references and the operator's matching launch evidence.
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        let task = fixture(&ctx, kind, None, 3);
        let socket = ctx.herdr.socket_path().unwrap();

        // When the exclusive binding operation records that evidence.
        ctx.store
            .bind_herdr_session(&socket, "launch record")
            .unwrap();

        // Then both the summary and the original task-local walk recognize the owner.
        assert!(session_scope::owns_task(&ctx, &task).unwrap(), "{kind}");
        assert!(session_scope::owns_inventory(&ctx).unwrap(), "{kind}");
    }
}

#[test]
fn snapshot_summary_does_not_mix_later_binding_journal_or_task_owners() {
    // Given a snapshot of saved local panes, not yet evaluated for ownership.
    let temp = tempfile::tempdir().unwrap();
    let ctx = context(temp.path());
    let mut task = fixture(&ctx, "worker", Some(&ctx.herdr.socket_path().unwrap()), 3);
    let snapshot = Ctx {
        store: ctx.store.read_only_snapshot().unwrap(),
        ..ctx.clone()
    };
    let foreign = temp.path().join("foreign.sock");

    // When later commits replace the task owner and install foreign journal and binding records.
    for run in &mut task.runs {
        run.herdr.as_mut().unwrap().socket = Some(foreign.clone());
    }
    ctx.store.fixture_put_task(&task).unwrap();
    ctx.store
        .save_record("review-launch", "current", &json!({"socket": foreign}))
        .unwrap();
    ctx.store
        .bind_herdr_session(&foreign, "foreign launch record")
        .unwrap();

    // Then first evaluation still sees the original complete snapshot, never mixed commits.
    assert!(session_scope::owns_inventory(&snapshot).unwrap());
    let old = snapshot.store.session_view().unwrap();
    assert!(old.binding.is_none());
    assert!(old.pending.is_none());
    assert!(!session_scope::owns_inventory(&ctx).unwrap());
}

fn measured(copies: usize) -> (u64, u64, Vec<u64>) {
    let temp = tempfile::tempdir().unwrap();
    let ctx = context(temp.path());
    let socket = ctx.herdr.socket_path().unwrap();
    for _ in 0..copies {
        fixture(&ctx, "review-child", Some(&socket), copies);
        fixture(&ctx, "plan-review", Some(&socket), copies);
    }
    let tasks = ctx.store.list().unwrap();
    let pass = Ctx {
        store: ctx.store.read_only_snapshot().unwrap(),
        ..ctx
    };
    let before = work::snapshot();
    let binding = work::count(Work::SessionBindingRead);
    let scans = work::count(Work::OwnershipScan);
    let _hot = HotScope::enter();
    for _ in 0..3 {
        assert!(session_scope::owns_inventory(&pass).unwrap());
        assert!(session_scope::owns_tasks(&pass, &pass.store, &tasks).unwrap());
    }
    assert!(session_scope::check_launch_in(&pass, &pass.store).is_ok());
    let after = work::snapshot();
    let delta = Counter::ALL
        .iter()
        .map(|counter| after.total(*counter) - before.total(*counter))
        .collect();
    assert_eq!(
        after.total(Counter::FullTaskHydrations),
        before.total(Counter::FullTaskHydrations)
    );
    (
        work::count(Work::SessionBindingRead) - binding,
        work::count(Work::OwnershipScan) - scans,
        delta,
    )
}

#[test]
fn ownership_work_does_not_grow_with_board_or_nested_history() {
    // Given one and ten times the task and nested-reference history.
    // When repeated pass guards and a launch boundary evaluate each fixture.
    let one = measured(1);
    let ten = measured(10);

    // Then all measured work stays equal, without full task hydration.
    assert_eq!(one, ten);
    assert_eq!((one.0, one.1), (2, 2));
}

#[test]
fn singleton_ownership_uses_bounded_nested_container_facts() {
    let mut measurements = Vec::new();
    for copies in [1, 10] {
        // Given one consumed task whose current plan has different amounts of nested history.
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        let socket = ctx.herdr.socket_path().unwrap();
        let mut task = fixture(&ctx, "plan-review", Some(&socket), copies);
        task.created = store_support::at(0);
        task.updated = store_support::at(0);
        ctx.store.fixture_put_task(&task).unwrap();
        let store = ctx.store.read_only_snapshot().unwrap();
        let expected = session_scope::owns_task(&ctx, &task).unwrap();

        // When the singleton consumer checks ownership under a hot scope.
        let before = work::snapshot();
        let actual = {
            let _hot = HotScope::enter();
            session_scope::owns_tasks(&ctx, &store, &[task]).unwrap()
        };
        let delta = work::snapshot().since(&before);

        // Then it matches the original walk without decoding nested history or full tasks.
        assert!(actual);
        assert_eq!(actual, expected);
        assert_eq!(delta.full_task_hydrations(), 0);
        assert_eq!(delta.get(Counter::NestedRowDecodes), 0);
        delta.check_identity().unwrap();
        measurements.push(delta);
    }
    assert_eq!(measurements[0], measurements[1]);
}
