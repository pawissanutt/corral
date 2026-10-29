//! `HeadQuery::UnreleasedPanes` (T-334): the tasks, open or finished, that still hold a pane the
//! cleanup ledger has not released, on SQLite and on the fake; released and pane-less tasks are
//! not returned.

mod store_support;

use std::sync::Arc;

use corral_core::backend::fake::FakeBackend;
use corral_core::backend::{BackendConfig, FaultInjector, HeadQuery, ManualClock};
use corral_core::{Run, Status};
use serde_json::json;
use store_support::{at, store};

fn run(id: &str, pane: Option<&str>) -> Run {
    let mut run = json!({
        "id": id, "agent": "fake", "worktree": "/tmp/unused", "branch": "corral/x",
        "base": "main", "base_sha": "0".repeat(40), "started": at(0),
    });
    if let Some(pane) = pane {
        run["herdr"] = json!({"workspace_id": format!("w-{pane}"), "pane_id": pane});
    }
    serde_json::from_value(run).unwrap()
}

#[test]
fn unreleased_pane_owners_on_both_backends() {
    for fake in [false, true] {
        let (_dir, base) = store();
        let store = if fake {
            base.with_backend(Box::new(FakeBackend::new(
                BackendConfig::default(),
                Arc::new(ManualClock::new(at(0))),
                Arc::new(FaultInjector::default()),
            )))
        } else {
            base
        };
        let add = |status: Status, pane: Option<&str>| {
            let task = store.create("pane owner", None, status, "").unwrap();
            store
                .update(&task.id, |t| {
                    t.runs.push(run("R1", pane));
                    Ok(())
                })
                .unwrap();
            task.id
        };
        let open = add(Status::Running, Some("p-open"));
        let finished = add(Status::Done, Some("p-done"));
        add(Status::Done, None);
        let ids = || -> Vec<String> {
            store
                .heads(&HeadQuery::UnreleasedPanes)
                .unwrap()
                .into_iter()
                .map(|head| head.id.clone())
                .collect()
        };
        assert_eq!(ids(), vec![open.clone(), finished.clone()], "fake={fake}");

        let keys: Vec<String> = store
            .head(&finished)
            .unwrap()
            .unreleased_panes(false)
            .unwrap()
            .into_iter()
            .map(|pane| pane.key)
            .collect();
        assert!(!keys.is_empty());
        store
            .update_head(&finished, |head| head.release_panes(&keys, &[]))
            .unwrap();
        assert_eq!(
            ids(),
            vec![open],
            "a released pane leaves the set; fake={fake}"
        );
    }
}
