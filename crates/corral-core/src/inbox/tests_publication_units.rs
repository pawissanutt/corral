//! Frozen envelope intents survive standalone crashes, but not an aborted parent unit.
use super::*;

#[test]
fn envelope_freeze_and_pointer_roll_back_in_an_existing_record_unit() {
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    inbox.post(post("pending", Urgency::Wake)).unwrap();
    let before = inbox.state().unwrap();
    let aborted: anyhow::Result<()> = store.records_transaction(|| {
        assert!(publish(&inbox, "parent", false).is_some());
        anyhow::bail!("parent refused")
    });
    assert!(aborted.is_err());
    assert_eq!(inbox.state().unwrap(), before);
    for kind in ["envelopes", "payloads", "message-envelope"] {
        assert!(inbox.records(kind).unwrap().is_empty(), "{kind}");
    }
    assert!(publish(&inbox, "parent", false).is_some());
}

#[test]
fn settled_unpointed_freeze_consumes_its_identity_without_reopening() {
    let (_root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    inbox.post(post("original", Urgency::Wake)).unwrap();
    super::super::header::CRASH_AT.with(|point| point.set(Some("envelope_written")));
    let frozen = inbox.publish(&binding("original"), "fixture", MAX_PAYLOAD, false);
    super::super::header::CRASH_AT.with(|point| point.set(None));
    assert!(frozen.is_err());
    inbox
        .record_receipt(1, ReceiptLevel::Observed, &receipt("external"))
        .unwrap();
    inbox.recover_publication().unwrap();
    assert_eq!(inbox.state().unwrap().open_envelope, None);
    assert_eq!(inbox.state().unwrap().next_envelope, 2);
    inbox.post(post("later", Urgency::Wake)).unwrap();
    assert_eq!(publish(&inbox, "later", false).unwrap().number, 2);
}

#[test]
fn interrupted_freeze_cannot_be_overwritten_before_recovery() {
    let (root, store) = fixture();
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    inbox.post(post("original", Urgency::Wake)).unwrap();
    super::super::header::CRASH_AT.with(|point| point.set(Some("envelope_written")));
    let frozen = inbox.publish(&binding("original"), "fixture", MAX_PAYLOAD, false);
    super::super::header::CRASH_AT.with(|point| point.set(None));
    assert!(frozen.is_err());
    let envelope = inbox.envelope(1).unwrap();
    let payload = inbox.payload(&envelope).unwrap();
    inbox.post(post("later", Urgency::Wake)).unwrap();
    assert!(
        inbox
            .publish(&binding("replacement"), "fixture", MAX_PAYLOAD, false)
            .is_err()
    );
    assert_eq!(inbox.envelope(1).unwrap(), envelope);
    assert_eq!(inbox.payload(&envelope).unwrap(), payload);
    let reopened = Store::open(root.path()).unwrap();
    let inbox = Inbox::new(&reopened, Recipient::Pm).unwrap();
    inbox.recover_publication().unwrap();
    assert_eq!(inbox.state().unwrap().open_envelope, Some(1));
    assert_eq!(inbox.state().unwrap().next_envelope, 2);
    assert_eq!(inbox.envelope(1).unwrap(), envelope);
}
