//! Canonical full/head writes and snapshot behavior of the payload-free location reader.
mod store_support;
#[path = "task_ref_support/mod.rs"]
mod task_ref_support;

use corral_core::{Run, Status};
use serde_json::json;

fn run(id: &str, path: &str) -> Run {
    serde_json::from_value(json!({
        "id":id,"agent":"generic","worktree":path,"branch":"main","base":"main",
        "base_sha":"fixture","started":"2026-01-01T00:00:00Z","outcome":"done"
    }))
    .unwrap()
}

#[test]
fn locations_match_full_tasks_and_stay_on_their_pinned_view() {
    for (name, _dir, store) in task_ref_support::backends() {
        let task = store.create("Terminal", None, Status::Done, "").unwrap();
        store
            .update(&task.id, |t| {
                t.runs = vec![run("R1", "/old"), run("R2", "/current")];
                Ok(())
            })
            .unwrap();
        let frozen = store.read_only_snapshot().unwrap();
        let before = frozen.view().unwrap().run_worktrees(true).unwrap();
        assert_eq!(before.len(), 1, "{name}");
        assert_eq!(before[0].run, "R2");
        store
            .update_head(&task.id, |head| {
                head.current_run_mut().unwrap().worktree = "/moved".into();
                Ok(())
            })
            .unwrap();
        let full = store.get(&task.id).unwrap();
        let current = store.view().unwrap().run_worktrees(true).unwrap();
        assert_eq!(
            current[0].worktree,
            full.current_run().unwrap().worktree,
            "{name}"
        );
        let mut all: Vec<_> = store
            .view()
            .unwrap()
            .run_worktrees(false)
            .unwrap()
            .into_iter()
            .map(|r| (r.run, r.worktree))
            .collect();
        all.sort();
        assert_eq!(
            all,
            full.runs
                .iter()
                .map(|r| (r.id.clone(), r.worktree.clone()))
                .collect::<Vec<_>>(),
            "{name}"
        );
        assert_eq!(
            frozen.view().unwrap().run_worktrees(true).unwrap(),
            before,
            "{name}"
        );
        store
            .update(&task.id, |t| {
                t.runs.clear();
                Ok(())
            })
            .unwrap();
        assert!(
            store
                .view()
                .unwrap()
                .run_worktrees(false)
                .unwrap()
                .is_empty(),
            "{name}"
        );
    }
}
