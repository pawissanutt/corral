//! History-cost counters (plan T-253 section 4, "History-cost classes" and "Follow-up
//! accounting"): every counted event lands in its inclusive total and exactly one cell.

mod store_support;

use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use corral_core::{OperatingActionKind, Status, Store, TaskComment};
use store_support::*;

fn measure(f: impl FnOnce()) -> WorkSnapshot {
    let before = work::snapshot();
    f();
    let delta = work::snapshot().since(&before);
    delta.check_identity().unwrap();
    delta
}

fn one_task() -> (tempfile::TempDir, Store, String) {
    let (dir, store) = store();
    let task = rich(&store, 2, 3, 1, 2);
    (dir, store, task.id)
}

#[test]
fn nothing_is_counted_outside_a_hot_scope() {
    let (_dir, store, id) = one_task();
    let delta = measure(|| {
        store.get(&id).unwrap();
        store.list().unwrap();
        store.head(&id).unwrap().review_children("R0002").unwrap();
    });
    assert_eq!(delta, WorkSnapshot::default());
}

#[test]
fn full_hydrations_in_a_hot_scope_and_its_action_scope_are_all_unattributed() {
    // Given a store holding exactly T-001.
    let (_dir, store, id) = one_task();
    // When a hot scope runs get, list and update, then the same inside an action scope.
    let delta = measure(|| {
        let _hot = HotScope::enter();
        store.get(&id).unwrap();
        store.list().unwrap();
        store
            .update(&id, |t| {
                t.rounds += 1;
                Ok(())
            })
            .unwrap();
        let _action = work::action(&id, OperatingActionKind::Dispatch);
        store.get(&id).unwrap();
        store
            .update(&id, |t| {
                t.rounds += 1;
                Ok(())
            })
            .unwrap();
        store.list().unwrap();
    });
    // Then all six count inclusively and none is attributed.
    assert_eq!(delta.full_task_hydrations(), 6);
    assert_eq!(delta.unattributed_hydrations(), 6);
    if cfg!(debug_assertions) {
        assert_eq!(delta.sites.len(), 6);
        let labelled: Vec<_> = delta
            .sites
            .iter()
            .filter_map(|s| s.action.clone())
            .collect();
        assert_eq!(labelled, vec![(id.clone(), "dispatch".to_owned()); 3]);
        assert!(
            delta
                .sites
                .iter()
                .all(|s| s.location.file().ends_with("work_counters.rs"))
        );
    }
}

#[test]
fn output_sized_nested_pages_count_every_row_in_a_hot_scope() {
    let (_dir, store, id) = one_task();
    let head = store.head(&id).unwrap();
    let delta = measure(|| {
        let _hot = HotScope::enter();
        let children = head.review_children("R0002").unwrap();
        let entries = head.usage_entries("R0002", None).unwrap();
        assert_eq!(children.len(), 3);
        assert_eq!(entries.len(), 3);
    });
    assert_eq!(delta.get(Counter::NestedRowDecodes), 6);
    assert_eq!(delta.full_task_hydrations(), 0);
}

#[test]
fn nested_class_scopes_send_each_event_to_the_innermost_claiming_scope() {
    let (_dir, store, id) = one_task();
    let head = store.head(&id).unwrap();
    let _hot = HotScope::enter();
    let _action = work::action(&id, OperatingActionKind::Dispatch);

    // A follow-up claims every event inside it: hydrations, rows, stats and processes.
    let delta = measure(|| {
        let _fu = work::followup("FU-3", "sample");
        store.list().unwrap();
        work::counted(Counter::FileStats, 1);
        work::counted(Counter::Processes, 1);
    });
    assert_eq!(delta.full_task_hydrations(), 1);
    assert_eq!(delta.followup_hydrations("FU-3", "sample"), 1);
    assert_eq!(delta.unattributed_hydrations(), 0);
    assert_eq!(delta.followup_stats("FU-3", "sample"), 1);
    assert_eq!(delta.followup_processes("FU-3", "sample"), 1);
    assert_eq!(delta.followup_calls("FU-3", "sample"), 1);
    assert_eq!(delta.unattributed_byte_remainder(), 0);
    assert!(delta.followup_rows("FU-3", "sample") > 0);

    // own_records claims only the named run's child rows.
    let delta = measure(|| {
        let _own = work::own_records("test", &id, "R0002");
        head.review_children("R0002").unwrap();
        head.review_children("R0001").unwrap();
        store.get(&id).unwrap();
    });
    assert_eq!(delta.own_record_rows(), 3);
    assert!(delta.own_record_bytes() > 0);
    assert_eq!(delta.unattributed_hydrations(), 1);
    assert!(delta.get(Counter::NestedRowDecodes) >= 2);

    // An emitted reader inside own_records claims only its emitted rows.
    let delta = measure(|| {
        let _own = work::own_records("test", &id, "R0002");
        let obligations = head.emitted_obligations("R0002").unwrap();
        assert_eq!(obligations.waits.len() + obligations.decisions.len(), 6);
    });
    assert_eq!(delta.emitted_rows(), 6);
    assert_eq!(delta.own_record_rows(), 0);

    // own_records inside a follow-up: the run's child rows to own-record, the rest to it.
    let delta = measure(|| {
        let _fu = work::followup("FU-3", "nested");
        let _own = work::own_records("test", &id, "R0002");
        head.review_children("R0002").unwrap();
        head.review_children("R0001").unwrap();
    });
    assert_eq!(delta.own_record_rows(), 3);
    assert!(delta.followup_rows("FU-3", "nested") >= 2);
    assert_eq!(delta.get(Counter::NestedRowDecodes), 0);

    // A follow-up inside own_records claims everything inside it.
    let delta = measure(|| {
        let _own = work::own_records("test", &id, "R0002");
        let _fu = work::followup("FU-3", "inner");
        head.review_children("R0002").unwrap();
    });
    assert_eq!(delta.own_record_rows(), 0);
    assert_eq!(delta.followup_rows("FU-3", "inner"), 3);
}

fn combined(comments: usize, nested: usize) -> WorkSnapshot {
    let (_dir, store) = store();
    let mut task = store
        .create("Combined", Some("app"), Status::Running, "")
        .unwrap();
    grow(&mut task, 1, 1 + nested, 1, 1);
    (task.created, task.updated) = (at(0), at(0));
    for n in 0..comments {
        task.comments.push(TaskComment {
            author: corral_core::CommentAuthor::Owner,
            at: at(n as i64),
            text: format!("{:<60}", n),
        });
    }
    store.fixture_put_task(&task).unwrap();
    let head = store.head(&task.id).unwrap();
    let _hot = HotScope::enter();
    measure(|| {
        let _fu = work::followup("FU-3", "sample");
        store.list_shared().unwrap();
        head.emitted_comments().unwrap();
    })
}

#[test]
fn added_comments_charge_the_emitted_and_follow_up_cells_once_each() {
    // Given a base store and one with ten added comments on the task.
    let base = combined(1, 0);
    let grown = combined(11, 0);
    // Then the emitted and follow-up bytes each grow by the added bytes and the total by twice.
    let added = grown.emitted_doc_bytes() - base.emitted_doc_bytes();
    assert!(added > 0);
    assert_eq!(
        grown.followup_doc_bytes("FU-3", "sample") - base.followup_doc_bytes("FU-3", "sample"),
        added
    );
    assert_eq!(
        grown.decoded_doc_bytes() - base.decoded_doc_bytes(),
        2 * added
    );
    assert_eq!(grown.emitted_rows() - base.emitted_rows(), 10);
    assert_eq!(
        grown.followup_rows("FU-3", "sample") - base.followup_rows("FU-3", "sample"),
        10
    );
    assert_eq!(
        grown.unattributed_byte_remainder(),
        base.unattributed_byte_remainder()
    );
    assert_eq!(grown.unattributed_hydrations(), 0);
}

#[test]
fn non_emitted_nested_growth_charges_only_the_follow_up_list() {
    let base = combined(1, 0);
    let grown = combined(1, 10);
    let followup =
        grown.followup_doc_bytes("FU-3", "sample") - base.followup_doc_bytes("FU-3", "sample");
    assert!(followup > 0);
    assert_eq!(
        grown.decoded_doc_bytes() - base.decoded_doc_bytes(),
        followup
    );
    assert_eq!(grown.emitted_rows(), base.emitted_rows());
    assert_eq!(grown.emitted_doc_bytes(), base.emitted_doc_bytes());
    assert_eq!(grown.own_record_rows(), base.own_record_rows());
    assert_eq!(
        grown.unattributed_byte_remainder(),
        base.unattributed_byte_remainder()
    );
    for counter in Counter::DECODES {
        assert_eq!(grown.get(counter), base.get(counter), "{}", counter.name());
    }
}
