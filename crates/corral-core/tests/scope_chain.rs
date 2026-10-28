//! T-332 (PM decision 2026-10-28): a fresh scope-history chain over a task's scope history
//! equals the chain its head stores, and the head's operating contract revision equals the
//! full form's, however the history was written.

mod store_support;

use corral_core::{ApprovedScopeRevision, Status, TaskRef};

fn revision(n: u64) -> ApprovedScopeRevision {
    ApprovedScopeRevision {
        revision: n,
        approved_scope: None,
        superseded_at: "2026-10-18T12:00:00Z".parse().unwrap(),
    }
}

#[test]
fn a_fresh_scope_history_chain_equals_the_stored_head_chain() {
    let (_dir, store) = store_support::store();
    for n in [0_u64, 1, 10] {
        // Given a scope history written half through the full form, half through head
        // mutations.
        let id = store
            .create(
                &format!("Scoped {n}"),
                Some("app"),
                Status::Backlog,
                "Spec\n",
            )
            .unwrap()
            .id;
        let mut task = store.get(&id).unwrap();
        task.approved_scope_history
            .extend((1..=n / 2).map(revision));
        store.fixture_put_task(&task).unwrap();
        for i in n / 2 + 1..=n {
            store
                .update_head(&id, |head| head.push_scope_revision(revision(i)))
                .unwrap();
        }
        // When the chain is computed afresh over the full history.
        let full = store.get(&id).unwrap();
        let head = store.head(&id).unwrap();
        assert_eq!(
            full.approved_scope_history.len(),
            usize::try_from(n).unwrap()
        );
        let fresh = corral_core::scope_history_chain(&full.approved_scope_history).unwrap();
        // Then it is the head's stored chain, empty exactly without history, and the head's
        // contract revision is the full form's.
        assert_eq!(
            head.history_summary().unwrap().scope_history_chain,
            fresh,
            "x{n}"
        );
        assert_eq!(fresh.is_empty(), n == 0, "x{n}");
        assert_eq!(
            head.contract_revision().unwrap(),
            corral_core::operating_contract_revision(&full).unwrap(),
            "x{n}"
        );
    }
}
