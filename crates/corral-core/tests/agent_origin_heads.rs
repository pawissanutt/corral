//! Capture-marker lookup uses the existing task-edge index, not a whole-board hydration.
mod store_support;
use corral_core::work::{self, Counter, HotScope};
use corral_core::{Store, agent::AgentOrigin, backend::HeadQuery};

#[test]
fn capture_marker_selects_first_head_and_tracks_full_and_head_edits() {
    let mut costs = Vec::new();
    for scale in [1, 10] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        store.fixture_pin_now(store_support::at(10)).unwrap();
        for _ in 0..2 * scale {
            store_support::rich(&store, 2 * scale, 2 * scale, 2 * scale, 2 * scale);
        }
        let tasks = store
            .create_batch(2, false, |_, tasks| {
                for task in tasks {
                    task.title = "Capture".into();
                    task.agent_origin = Some(AgentOrigin {
                        request_id: "captured".into(),
                        session_id: "pm".into(),
                        generation: 1,
                        at: store_support::at(0),
                    });
                }
                Ok(Vec::new())
            })
            .unwrap();
        let query = |request: &str| HeadQuery::EdgeTo {
            kind: "agent_origin".into(),
            other: request.into(),
        };
        let hot = HotScope::enter();
        let before = work::snapshot();
        let found = store.heads_page(&query("captured"), 1).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, tasks[0].id);
        assert!(store.heads_page(&query("absent"), 1).unwrap().is_empty());
        let delta = work::snapshot().since(&before);
        drop(hot);
        delta.check_identity().unwrap();
        assert_eq!(delta.unattributed_hydrations(), 0);
        costs.push(delta);
        store
            .update_head(&tasks[0].id, |head| {
                head.agent_origin = None;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            store.heads_page(&query("captured"), 1).unwrap()[0].id,
            tasks[1].id
        );
        store
            .update(&tasks[1].id, |task| {
                task.agent_origin.as_mut().unwrap().request_id = "new".into();
                Ok(())
            })
            .unwrap();
        let reopened = Store::open(root.path()).unwrap();
        assert!(
            reopened
                .heads_page(&query("captured"), 1)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            reopened.heads_page(&query("new"), 1).unwrap()[0].id,
            tasks[1].id
        );
    }
    for counter in Counter::ALL {
        assert_eq!(costs[0].get(counter), costs[1].get(counter), "{counter:?}");
    }
}
