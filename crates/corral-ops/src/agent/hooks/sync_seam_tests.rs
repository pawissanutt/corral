//! sync's idle rule at the pi-native seam: an idle pane past `idle_after_secs` is flagged
//! `idle_no_report` unless the sync hook reported the run's binding healthy.
use std::collections::HashMap;

use corral_core::{Run, Status, Store};
use corral_herdr::{AgentStatus, PaneInfo};
use serde_json::json;

use super::*;

#[test]
fn a_healthy_pi_native_binding_leaves_idle_no_report_unset() {
    for (healthy, flagged) in [(false, true), (true, false)] {
        // Given a Running run seen working, now idle in its pane for an hour:
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx {
            store: Store::init(tmp.path()).unwrap(),
            herdr: corral_herdr::Herdr::with_bin(tmp.path().join("no-herdr"), None),
            resource_probe: crate::resources::FixedResourceProbe::arc(),
        };
        let task = ctx.store.create("Idle", None, Status::Running, "").unwrap();
        let socket = ctx.herdr.socket_path().unwrap();
        let run: Run = serde_json::from_value(json!({
            "id": "R1", "agent": "pi", "harness": "pi-native", "worktree": tmp.path(),
            "branch": "main", "base": "main", "base_sha": "fixture",
            "started": "2026-10-15T00:00:00Z", "herdr": {"workspace_id": "w1", "pane_id": "w1:p1", "socket": socket},
            "seen_pane": true, "seen_working": true, "idle_since": "2026-10-15T00:00:00Z"
        }))
        .unwrap();
        let task = ctx
            .store
            .update(&task.id, |t| {
                t.runs.push(run);
                Ok(())
            })
            .unwrap();
        let pane = PaneInfo {
            pane_id: "w1:p1".into(),
            workspace_id: "w1".into(),
            tab_id: "t1".into(),
            agent: None,
            status: AgentStatus::Idle,
            title: None,
            cwd: None,
        };
        let panes = HashMap::from([("w1:p1".to_owned(), pane)]);
        let now: Timestamp = "2026-10-15T01:00:00Z".parse().unwrap();
        // When sync reconciles it with and without a healthy hook report:
        let snapshot = crate::pane_liveness::Snapshot::injected(&panes);
        let processes = SystemProcesses;
        reconcile(&ctx, &task, snapshot, now, &processes, healthy).unwrap();
        // Then only the unreported run is flagged.
        let run = ctx.store.get(&task.id).unwrap().runs[0].clone();
        assert_eq!(run.idle_no_report, flagged, "healthy={healthy}");
    }
}
