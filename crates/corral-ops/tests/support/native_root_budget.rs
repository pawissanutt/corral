//! Whole application sync, real root PM/assistant hosts, and an actually separate observer process.
use super::{
    native_root_fixture::{self as fixture, Roots},
    native_root_model::{self as model, Model},
};
use crate::agent::{bridge::fixture as bridge, native_test_support as native};
use corral_core::{
    records::RecordArea,
    work::{self, HotScope},
};
use serde::{Deserialize, Serialize};
use std::{path::Path, process::Stdio, sync::Arc};

const CHILD_ROOT: &str = "CORRAL_NATIVE_ROOT_BUDGET_ROOT";
const CHILD_REPORT: &str = "CORRAL_NATIVE_ROOT_BUDGET_REPORT";

#[derive(Debug, Serialize, Deserialize)]
struct Report {
    pid: u32,
    unattributed: Vec<u64>,
}

fn context(root: &Path) -> (crate::Ctx, Arc<crate::resources::PinnedReadingsProbe>) {
    let mut ctx = bridge::ctx_at(root);
    let probe = Arc::new(crate::resources::PinnedReadingsProbe::admitting());
    ctx.resource_probe = probe.clone();
    ctx.store.config.validate(root).unwrap();
    (ctx, probe)
}

fn measure(
    ctx: &crate::Ctx,
    probe: &crate::resources::PinnedReadingsProbe,
    assistant: &str,
    scale: usize,
) -> Report {
    corral_core::clock::pin_thread(Some(native::data::at(100_000)));
    crate::resources::pin_psi(Some((Some(0.25), Some(1.5))));
    crate::agent::bridge::liveness::probe_hosts_in_tests();
    // Independent oracle connection: never seed the observer Ctx's task/body caches.
    let oracle = corral_core::Store::open(ctx.store.root()).unwrap();
    let expected = Model::derive(&oracle, assistant);
    let docs = model::documents(&oracle, assistant);
    let events = oracle.events().unwrap();
    let samples = probe.samples();
    let before = work::snapshot();
    let hot = HotScope::enter();
    let changes = crate::sync_at(ctx, native::data::at(100_000)).unwrap();
    let delta = work::snapshot().since(&before);
    drop(hot);
    // Preconditions/effects are observed before the work cells are interpreted.
    assert!(changes.is_empty(), "{changes:?}");
    let resources: crate::resources::ResourceState = RecordArea::new("resources")
        .load(&ctx.store, "state")
        .unwrap()
        .unwrap();
    assert!(
        !resources.held && !resources.severe && resources.reason.is_none(),
        "{resources:?}"
    );
    assert_eq!(
        probe.samples() - samples,
        2,
        "observe and attention each run the real volume walk"
    );
    expected.volumes(&resources.snapshot);
    assert_eq!(resources.snapshot.load_one, 0.0);
    assert_eq!(resources.snapshot.mem_available, u64::MAX);
    assert!(
        resources
            .snapshot
            .disks
            .iter()
            .all(|d| d.free_bytes == u64::MAX)
    );
    assert_eq!(resources.snapshot.cpu_psi, Some(0.25));
    assert_eq!(resources.snapshot.mem_psi, Some(1.5));
    assert!(crate::settling::terminal_heads(ctx).unwrap().is_empty());
    assert_eq!(ctx.store.active_assistant_ids().unwrap(), [assistant]);
    assert_eq!(
        model::documents(&ctx.store, assistant),
        docs,
        "opaque canonical documents must be unchanged"
    );
    assert_eq!(
        ctx.store.events().unwrap(),
        events,
        "steady sync appends no events"
    );
    fixture::families(
        &corral_core::agent::pm_state::load(&ctx.store)
            .unwrap()
            .unwrap()
            .state,
        scale,
    );
    fixture::families(
        &ctx.store.get_assistant_container(assistant).unwrap().state,
        scale,
    );
    let unattributed = expected.check(&delta);
    println!(
        "ROOT whole sync pid={} scale={scale}: {delta:?}",
        std::process::id()
    );
    corral_core::clock::pin_thread(None);
    crate::resources::pin_psi(None);
    Report {
        pid: std::process::id(),
        unattributed,
    }
}

fn rpc(roots: &Roots) {
    for (server, hellos) in [(&roots.pm, 2), (&roots.assist, 0)] {
        let frames = server.commands();
        for (command, count) in [("hello", hellos), ("get_state", 1), ("events_since", 1)] {
            assert_eq!(
                frames.iter().filter(|f| f["command"] == command).count(),
                count,
                "{frames:?}"
            );
        }
        assert_eq!(
            frames.len(),
            hellos + 2,
            "no submit, cancel, missed probe, or unexpected RPC: {frames:?}"
        );
    }
}

#[test]
fn native_root_sync_fresh_process_child() {
    let Some(root) = std::env::var_os(CHILD_ROOT) else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let scale = std::env::var("CORRAL_NATIVE_ROOT_BUDGET_SCALE")
        .unwrap()
        .parse()
        .unwrap();
    let (ctx, probe) = context(&root);
    assert_ne!(
        std::env::var("CORRAL_NATIVE_ROOT_PARENT_PID").unwrap(),
        std::process::id().to_string()
    );
    // First actual sync in this process: no primer, warmed Ctx or parent work-ledger state.
    let report = measure(&ctx, &probe, "A-1", scale);
    std::fs::write(
        std::env::var_os(CHILD_REPORT).unwrap(),
        serde_json::to_vec(&report).unwrap(),
    )
    .unwrap();
}

#[test]
fn whole_root_pm_and_assistant_sync_has_exact_history_cost_in_warm_and_fresh_processes() {
    // Hold real identities fixed across histories: decimal PID/start-time widths are
    // fixture inputs to bounded root reads, not additional history work.
    let pm_process = bridge::HostProc::spawn(true);
    let assistant_process = bridge::HostProc::spawn(true);
    let mut costs = Vec::new();
    for scale in [1, 10] {
        let roots = fixture::roots(scale, &pm_process, &assistant_process);
        let (ctx, probe) = context(roots.fx.root());
        corral_core::clock::pin_thread(Some(native::data::at(100_000)));
        crate::resources::pin_psi(Some((Some(0.25), Some(1.5))));
        crate::agent::bridge::liveness::probe_hosts_in_tests();
        // Real durable initialization and one actual owning sync establish the steady state.
        crate::settling::recover(&ctx).unwrap();
        crate::sync_at(&ctx, native::data::at(100_000)).unwrap();
        rpc(&roots);
        assert!(crate::settling::terminal_heads(&ctx).unwrap().is_empty());
        costs.push(measure(&ctx, &probe, &roots.assistant, scale));
        rpc(&roots);
        let report = roots.fx.root().join("root-budget-child-report.json");
        let child = crate::test_support::command(std::env::current_exe().unwrap(), roots.fx.root())
            .args([
                "--exact",
                "submission_paths_tests::native_root_budget::native_root_sync_fresh_process_child",
                "--nocapture",
            ])
            .env(CHILD_ROOT, roots.fx.root())
            .env(CHILD_REPORT, &report)
            .env("CORRAL_NATIVE_ROOT_BUDGET_SCALE", scale.to_string())
            .env(
                "CORRAL_NATIVE_ROOT_PARENT_PID",
                std::process::id().to_string(),
            )
            .env("RUST_TEST_THREADS", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "fresh child: {}\n{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        assert!(
            String::from_utf8_lossy(&child.stdout).contains("1 passed; 0 failed;"),
            "must execute exactly one child test"
        );
        let observed: Report = serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap();
        assert_ne!(observed.pid, std::process::id());
        println!("Fresh root sync child pid={} scale={scale}", observed.pid);
        costs.push(observed);
        rpc(&roots);
    }
    for sample in &costs[1..] {
        assert_eq!(
            sample.unattributed, costs[0].unattributed,
            "exact every-cell history/restart equality: {costs:?}"
        );
    }
}
