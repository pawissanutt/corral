use super::*;

/// History a decomposition never reads: earlier planning runs and plans before the current
/// one, owner comments, events and terminal tasks.
fn grow_parent(ctx: &Ctx, id: &str, n: usize) {
    ctx.store
        .update(id, |task| {
            let plan = task.plans.last().unwrap().clone();
            for i in 0..n {
                let mut earlier = plan.clone();
                earlier.id = format!("H{i:03}");
                earlier.reviews.clear();
                earlier.approval = None;
                let at = task.plans.len() - 1;
                task.plans.insert(at, earlier);
                task.comments.push(corral_core::TaskComment {
                    at: "2026-10-18T08:00:00Z".parse().unwrap(),
                    author: corral_core::CommentAuthor::Owner,
                    text: format!("Earlier note {i}"),
                });
            }
            Ok(())
        })
        .unwrap();
    for i in 0..n {
        ctx.store
            .log_event(Some(id), "fixture_history", serde_json::json!({"n": i}))
            .unwrap();
        ctx.store
            .create(&format!("Zirconium {i}"), None, Status::Done, "")
            .unwrap();
    }
}

/// The published tracker and children, timestamps normalized.
fn published(ctx: &Ctx, id: &str) -> Vec<serde_json::Value> {
    let parent = ctx.store.get(id).unwrap();
    let child = ctx.store.get(&parent.children[0]).unwrap();
    let tracker = child.parent.clone().unwrap();
    std::iter::once(tracker)
        .chain(parent.children.iter().cloned())
        .map(|task| {
            let mut task = serde_json::to_value(ctx.store.get(&task).unwrap()).unwrap();
            task["created"] = serde_json::Value::Null;
            task["updated"] = serde_json::Value::Null;
            task
        })
        .collect()
}

#[test]
fn automatic_multi_scope_publication_reads_the_parent_as_a_head() {
    use corral_core::work::{self, Counter, HotScope};
    let mut measured = Vec::new();
    for n in [1, 10] {
        // Given the same accepted two-scope plan on two stores with the same parent history.
        let (_owner_dir, owner, owner_id) = scoped_fixture(&["a", "b"]);
        let (_auto_dir, auto, id) = scoped_fixture(&["a", "b"]);
        grow_parent(&owner, &owner_id, n);
        grow_parent(&auto, &id, n);
        // When the owner decomposes in full and automatic acceptance decomposes on heads.
        apply(&owner, &owner_id, "P1").unwrap();
        let delta = {
            let _hot = HotScope::enter();
            let before = work::snapshot();
            apply_auto(&auto, &id, "P1").unwrap();
            work::snapshot().since(&before)
        };
        // Then both publish the same tracker and children, and the automatic one hydrates
        // no task.
        assert_eq!(published(&auto, &id), published(&owner, &owner_id), "x{n}");
        delta.check_identity().unwrap();
        let sites: Vec<String> = delta
            .sites
            .iter()
            .filter(|site| site.followup.is_none())
            .map(|site| format!("{}:{}", site.location.file(), site.location.line()))
            .collect();
        assert!(sites.is_empty(), "x{n}: counted hydrations at {sites:?}");
        assert_eq!(delta.unattributed_hydrations(), 0, "x{n}");
        measured.push(delta);
    }
    // And its decoded rows do not follow the parent's history.
    for counter in Counter::DECODES {
        assert_eq!(
            measured[0].get(counter),
            measured[1].get(counter),
            "{counter:?}"
        );
    }
}
