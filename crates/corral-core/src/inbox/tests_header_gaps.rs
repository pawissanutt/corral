//! Legacy indexing crosses reserved-but-unpublished sequence gaps, not malformed rows.
use super::*;
use crate::work;

#[test]
fn legacy_owner_index_crosses_real_standalone_gap_with_one_read_per_budget_slot() {
    let (_root, store, recipient) = fixture();
    let inbox = Inbox::new(&store, recipient.clone()).unwrap();
    crash("post_reserved", || inbox.post(post()));
    assert!(inbox.message(1).is_err());
    let published = inbox.post(post()).unwrap();
    assert_eq!(published.seq, 2);
    // An imported legacy inbox has its sequence and canonical records, but no owner index.
    store
        .update(recipient.task().unwrap(), |task| {
            recipient.state_mut(task)?.owner_header = None;
            Ok(())
        })
        .unwrap();
    let before = work::count(work::Work::InboxIndexRead);
    let mut indexed = None;
    for _ in 0..4 {
        let header = inbox.index_owner(1).unwrap();
        if header.legacy_debt.is_none() {
            indexed = Some(header);
            break;
        }
    }
    let header = indexed.expect("two slots in each of two phases must finish");
    assert_eq!(
        header.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
        vec![2]
    );
    assert_eq!(work::count(work::Work::InboxIndexRead) - before, 4);
    assert_eq!(inbox.post(post()).unwrap().seq, 3);
    assert_eq!(inbox.message(2).unwrap(), published);
}

#[test]
fn legacy_index_still_refuses_corrupt_present_identity_without_committing_header() {
    let (_root, store, recipient) = fixture();
    let inbox = Inbox::new(&store, recipient.clone()).unwrap();
    inbox.post(post()).unwrap();
    let key = format!("{:020}", 1);
    let mut row: super::super::records::MessageRecord =
        inbox.record("messages", &key).unwrap().unwrap();
    row.message.id = "foreign#1".into();
    inbox.save_record("messages", &key, &row).unwrap();
    store
        .update(recipient.task().unwrap(), |task| {
            recipient.state_mut(task)?.owner_header = None;
            Ok(())
        })
        .unwrap();
    let before = inbox.state().unwrap();
    let error = inbox.index_owner(64).unwrap_err();
    assert!(
        error.to_string().contains("message identity mismatch"),
        "{error:#}"
    );
    assert_eq!(inbox.state().unwrap(), before);
}
