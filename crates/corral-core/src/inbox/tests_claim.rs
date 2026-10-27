//! The bounded owner read finds retry claims through their failed-slot mirror.
use std::fs;

use serde_json::json;

use super::tests_retry::{post, publication_fixture, refuses, succeeds};
use super::*;
use crate::Store;

#[test]
fn mirrored_retry_claim_is_read_within_five_probes_and_survives_observation() {
    // Given a retry whose replacement identity committed but whose body could not publish.
    let (_root, store, recipient) = publication_fixture(true);
    let inbox = Inbox::new(&store, recipient).unwrap();
    let original = post(&inbox, "reserved replacement");
    inbox
        .mark_terminal(original.seq, TerminalState::Cancelled, "fixture", "test")
        .unwrap();
    fs::create_dir(inbox.message_path(2)).unwrap();
    assert!(inbox.retry(original.seq).is_err());
    fs::remove_dir(inbox.message_path(2)).unwrap();
    let owner_read = |inbox: &Inbox<'_>| {
        let state = inbox.state().unwrap();
        let reads = crate::work::count(crate::work::Work::InboxRecordRead);
        let entries = inbox.owner_entries(&state).unwrap();
        let used = crate::work::count(crate::work::Work::InboxRecordRead) - reads;
        (entries.into_iter().next().unwrap(), used)
    };
    // When the bounded owner read runs (two retained entries), then the exact claim and
    // successor are visible at five probes each.
    let (entry, used) = owner_read(&inbox);
    assert_eq!(used, 10);
    assert!(entry.retry_claimed_at.is_some());
    assert_eq!(entry.retry_successor, Some(format!("{}#2", inbox.key())));
    // And a later observation keeps the claim, as the marker file does.
    inbox
        .record_receipt(
            original.seq,
            ReceiptLevel::Observed,
            &Receipt {
                envelope: None,
                binding: None,
                observed_at: "2026-10-25T00:00:00Z".parse().unwrap(),
                via: "fixture".into(),
                detail: json!({}),
            },
        )
        .unwrap();
    let (entry, used) = owner_read(&inbox);
    assert_eq!(used, 10);
    assert!(entry.observed.is_some() && entry.terminal.is_none());
    assert!(entry.retry_claimed_at.is_some());
    assert!(inbox.record_path(original.seq, "retried").exists());
}

#[test]
fn premirror_claims_are_backfilled_before_absence_is_trusted_for_primary_and_overflow() {
    for position in [1u64, 9] {
        // Given a retained claim written before the mirror existed, in a primary or overflow entry.
        let (root, store, recipient) = publication_fixture(true);
        let inbox = Inbox::new(&store, recipient.clone()).unwrap();
        let mut posted = Vec::new();
        for _ in 0..position {
            posted.push(post(&inbox, "claimed"));
        }
        let original = posted.pop().unwrap();
        inbox
            .mark_terminal(original.seq, TerminalState::Cancelled, "fixture", "test")
            .unwrap();
        fs::create_dir(inbox.message_path(position + 1)).unwrap();
        assert!(inbox.retry(original.seq).is_err());
        fs::remove_dir(inbox.message_path(position + 1)).unwrap();
        {
            let _lock = inbox.lock().unwrap();
            inbox.mirror_retry_claim(original.seq, None).unwrap();
            inbox
                .update_state(|state| {
                    for entry in state.owner_header.as_mut().unwrap().iter_mut() {
                        entry.claim_mirrored = false;
                    }
                    Ok(())
                })
                .unwrap();
        }
        let reopened = Store::open(root.path()).unwrap();
        let inbox = Inbox::new(&reopened, recipient).unwrap();
        let find = |inbox: &Inbox<'_>| {
            inbox
                .owner_entries(&inbox.state().unwrap())
                .unwrap()
                .into_iter()
                .find(|entry| entry.message.seq == original.seq)
                .unwrap()
        };
        // Then an absent mirror is not trusted as an absent claim before the backfill.
        assert!(!find(&inbox).retry_eligible());
        // When bounded maintenance runs, then the exact claim and successor are visible again.
        inbox
            .maintain_owner_header("2026-10-25T00:00:00Z".parse().unwrap())
            .unwrap();
        let entry = find(&inbox);
        assert!(entry.message.claim_mirrored);
        assert!(entry.retry_claimed_at.is_some());
        assert_eq!(
            entry.retry_successor,
            Some(format!("{}#{}", inbox.key(), position + 1))
        );
        assert!(!entry.retry_eligible());
        refuses(&inbox, original.seq, "already_retried");
    }
}

#[test]
fn an_interruption_between_claim_mirror_and_marker_stays_visible_and_retryable() {
    // Given a retry interrupted after its mirror but before the authoritative marker.
    let (_root, store, recipient) = publication_fixture(true);
    let inbox = Inbox::new(&store, recipient).unwrap();
    let original = post(&inbox, "interrupted");
    inbox
        .mark_terminal(original.seq, TerminalState::Failed, "fixture", "test")
        .unwrap();
    super::header::CRASH_AT.with(|point| point.set(Some("retry_claim_mirrored")));
    let result = inbox.retry(original.seq);
    super::header::CRASH_AT.with(|point| point.set(None));
    assert!(result.is_err());
    // Then the claim is conservatively visible to the bounded read, never hidden.
    let entry = inbox.owner_entries(&inbox.state().unwrap()).unwrap()[0].clone();
    assert!(entry.retry_claimed_at.is_some());
    assert!(!entry.retry_eligible());
    assert!(!inbox.record_path(original.seq, "retried").exists());
    // And the exact retry still completes, replacing the orphaned mirror with the real claim.
    succeeds(&inbox, &original);
    let entry = inbox.owner_entries(&inbox.state().unwrap()).unwrap()[0].clone();
    assert_eq!(entry.retry_successor, Some(format!("{}#2", inbox.key())));
}
