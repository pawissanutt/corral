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

/// Scope amendments on the parent and terminal tasks on the board, neither of which a
/// publication reads.
fn grow_scope_and_board(ctx: &Ctx, id: &str, n: u64) {
    let mut task = ctx.store.get(id).unwrap();
    task.approved_scope_history.extend((1..=n).map(|revision| {
        corral_core::ApprovedScopeRevision {
            revision,
            approved_scope: None,
            superseded_at: "2026-10-18T08:00:00Z".parse().unwrap(),
        }
    }));
    ctx.store.fixture_put_task(&task).unwrap();
    // The owner grants the amended contract again, as after any scope amendment.
    let selection = corral_core::OperatingSelection {
        tasks: [id.to_owned()].into(),
        plan_scopes: [(id.to_owned(), ["a".into(), "b".into()].into())].into(),
        authority: corral_core::OperatingAuthority {
            dispatch: true,
            plan_accept: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let preview = ctx.store.preview_operating_policy(&selection).unwrap();
    ctx.store
        .confirm_operating_policy(&preview, "owner", "2026-10-18T08:00:00Z".parse().unwrap())
        .unwrap();
    for i in 0..n {
        ctx.store
            .create(&format!("Basalt {i}"), None, Status::Done, "")
            .unwrap();
    }
}

#[test]
fn interrupted_automatic_publication_recovers_without_scope_history_or_board_reads() {
    use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
    let mut measured: Vec<WorkSnapshot> = Vec::new();
    // Stamps written during the calls are pinned, so their encodings do not vary.
    corral_core::clock::pin_thread(Some("2026-10-18T12:00:00Z".parse().unwrap()));
    for n in [1, 10] {
        // Given an automatic two-scope publication interrupted after its journal and first
        // child, on a parent with n scope amendments and a board with n terminal tasks.
        let (_dir, ctx, id) = scoped_fixture(&["a", "b"]);
        ctx.store
            .fixture_pin_now("2026-10-18T12:00:00Z".parse().unwrap())
            .unwrap();
        grow_scope_and_board(&ctx, &id, n);
        let blocked = format!("T-{:03}", ctx.store.next_task_number().unwrap() + 1);
        ctx.store.fixture_block_task(&blocked, true).unwrap();
        assert!(apply_auto(&ctx, &id, "P1").is_err(), "x{n}");
        ctx.store.fixture_block_task(&blocked, false).unwrap();
        // When automatic acceptance recovers it.
        let recovery = {
            let _hot = HotScope::enter();
            let before = work::snapshot();
            apply_auto(&ctx, &id, "P1").unwrap();
            work::snapshot().since(&before)
        };
        // Then each child and the tracker are published exactly once, and no task is hydrated.
        assert_eq!(ctx.store.get(&id).unwrap().children.len(), 2, "x{n}");
        let board = ctx.store.list().unwrap().len();
        assert_eq!(
            board,
            usize::try_from(n).unwrap() + 4,
            "x{n}: parent, terminal, children, tracker"
        );
        {
            let delta = &recovery;
            delta.check_identity().unwrap();
            let sites: Vec<String> = delta
                .sites
                .iter()
                .filter(|site| site.followup.is_none())
                .map(|site| format!("{}:{}", site.location.file(), site.location.line()))
                .collect();
            assert!(sites.is_empty(), "x{n}: counted hydrations at {sites:?}");
            assert_eq!(delta.unattributed_hydrations(), 0, "x{n}");
        }
        measured.push(recovery);
    }
    // And recovery decodes no more documents of any class as the scope history and the board
    // grow. Its byte totals vary by a few bytes between identical runs (an unpinned stamp of
    // this fixture's store); exact byte accounting is the action budget's.
    let (small, large) = (&measured[0], &measured[1]);
    for counter in Counter::DECODES {
        assert_eq!(small.get(counter), large.get(counter), "{counter:?}");
    }
}
