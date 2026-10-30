use super::*;
use crate::inbox::{Inbox, Recipient};

fn store() -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    (tmp, store)
}

fn new(from: &str, key: Option<&str>) -> NewPmItem {
    NewPmItem {
        from: from.into(),
        kind: ItemKind::ActionRequest,
        subject: "Land whatever is ready".into(),
        body: "The owner asked to land everything ready; which tasks is unclear.".into(),
        refs: vec!["T-5".into()],
        requested_action: Some(RequestedAction::parse("approve T-5").unwrap()),
        owner_words: Some(owner_words("land whatever is ready".into(), None)),
        key: key.map(str::to_owned),
    }
}

fn code(error: &anyhow::Error) -> &'static str {
    error.downcast_ref::<PmInboxRefusal>().unwrap().code
}

fn events(store: &Store, kind: &str) -> Vec<crate::Event> {
    store
        .events_shared()
        .unwrap()
        .iter()
        .filter(|e| e.kind == kind)
        .cloned()
        .collect()
}

#[test]
fn c7_claim_and_resolve_record_pm_revisions_and_a_stale_claim_refuses() {
    // Given PMI-1 posted by assistant A-1 at revision 0.
    let (_tmp, store) = store();
    let session = store.create_assistant("pi", None, None, None).unwrap();
    let (posted, created) = store.pm_inbox_post(new("assistant:A-1", None)).unwrap();
    assert!(created);
    assert_eq!((posted.id.as_str(), posted.revision), ("PMI-1", 0));
    assert_eq!(posted.state, ItemState::Open);
    assert_eq!(
        store.get_assistant(&session.id).unwrap().inbox_items,
        ["PMI-1"]
    );
    // When the PM claims it at revision 0 and resolves it done with refs at revision 1.
    let claimed = store.pm_inbox_claim("PMI-1", 0, None).unwrap();
    let resolved = store
        .pm_inbox_resolve(
            "PMI-1",
            1,
            (Resolution::Done, "Landed T-5.", vec!["T-5".into()]),
            None,
        )
        .unwrap();
    // Then both carry PM attribution at revisions 1 and 2, with an outcome and an owed notice.
    assert_eq!((claimed.revision, claimed.state), (1, ItemState::Claimed));
    assert_eq!(claimed.claimed.as_ref().unwrap().by, "pm");
    assert_eq!(
        (resolved.revision, resolved.state),
        (2, ItemState::Resolved)
    );
    let outcome = resolved.outcome.as_ref().unwrap();
    assert_eq!(
        (outcome.resolution, outcome.by.as_str()),
        (Resolution::Done, "pm")
    );
    assert_eq!(outcome.refs, ["T-5"]);
    let history: Vec<_> = resolved
        .history
        .iter()
        .map(|t| (t.revision, t.state, t.by.as_str()))
        .collect();
    assert_eq!(
        history,
        [
            (1, ItemState::Claimed, "pm"),
            (2, ItemState::Resolved, "pm")
        ]
    );
    let notice = resolved.notice.as_ref().unwrap();
    assert_eq!(
        (notice.key.as_str(), notice.state),
        ("pmi:PMI-1:2", NoticeState::Pending)
    );
    for kind in ["pm_inbox_claimed", "pm_inbox_resolved"] {
        let event = events(&store, kind).pop().unwrap();
        assert_eq!(event.actor, Some(crate::Actor::Pm), "{kind}");
    }
    // When a second claim names revision 0 again, then it refuses and changes nothing.
    let error = store.pm_inbox_claim("PMI-1", 0, None).unwrap_err();
    assert_eq!(code(&error), "stale_revision");
    assert_eq!(store.pm_inbox_item("PMI-1").unwrap(), resolved);
}

#[test]
fn outcome_notice_reaches_the_author_inbox_once_across_repeats_and_a_lost_acknowledgement() {
    // Given a resolved item owed to live session A-1.
    let (_tmp, store) = store();
    store.create_assistant("pi", None, None, None).unwrap();
    store.pm_inbox_post(new("assistant:A-1", None)).unwrap();
    store.pm_inbox_claim("PMI-1", 0, None).unwrap();
    store
        .pm_inbox_resolve(
            "PMI-1",
            1,
            (Resolution::Declined, "Out of scope.", vec![]),
            None,
        )
        .unwrap();
    let inbox = Inbox::new(
        &store,
        Recipient::Assistant {
            session: "A-1".into(),
        },
    )
    .unwrap();
    // When the publication ran once before the item recorded it, as after a crash.
    let item = store.pm_inbox_item("PMI-1").unwrap();
    let early = inbox
        .post(Post {
            kind: NOTICE_KIND,
            urgency: Urgency::Wake,
            author: "pm",
            body: &notice_text(&item, "pmi:PMI-1:2"),
            binding: MessageBinding::default(),
            subject: Some(json!({"key": "pmi:PMI-1:2", "item": "PMI-1", "revision": 2})),
            max_payload: NATIVE_INPUT_MAX,
        })
        .unwrap();
    // And it is repeated twice more.
    let first = store.pm_inbox_publish_notice("PMI-1").unwrap().unwrap();
    let second = store.pm_inbox_publish_notice("PMI-1").unwrap().unwrap();
    // Then the one message is reused and recorded without a new revision.
    assert_eq!(first, early);
    assert_eq!(second, early);
    assert_eq!(inbox.messages().unwrap(), vec![early.clone()]);
    assert!(early.body.contains("pmi:PMI-1:2"));
    let item = store.pm_inbox_item("PMI-1").unwrap();
    assert_eq!(item.revision, 2);
    let notice = item.notice.unwrap();
    assert_eq!(notice.state, NoticeState::Posted);
    assert_eq!(notice.message, Some(early.id));
    assert_eq!(events(&store, "pm_inbox_notified").len(), 1);
}

#[test]
fn an_ended_author_keeps_the_outcome_in_the_item_and_gets_no_message() {
    // Given a resolved item whose author session ended before the notice was published.
    let (_tmp, store) = store();
    store.create_assistant("pi", None, None, None).unwrap();
    store.pm_inbox_post(new("assistant:A-1", None)).unwrap();
    store.pm_inbox_claim("PMI-1", 0, None).unwrap();
    store
        .pm_inbox_resolve(
            "PMI-1",
            1,
            (Resolution::Done, "Filed T-9.", vec!["T-9".into()]),
            None,
        )
        .unwrap();
    store.end_assistant("A-1").unwrap();
    // When the notice is published.
    assert!(store.pm_inbox_publish_notice("PMI-1").unwrap().is_none());
    // Then no message exists; the item keeps its outcome and the session still lists it.
    let inbox = Inbox::new(
        &store,
        Recipient::Assistant {
            session: "A-1".into(),
        },
    )
    .unwrap();
    assert!(inbox.messages().unwrap().is_empty());
    let item = store.pm_inbox_item("PMI-1").unwrap();
    assert_eq!(item.notice.unwrap().state, NoticeState::Kept);
    assert!(item.outcome.is_some());
    assert_eq!(store.get_assistant("A-1").unwrap().inbox_items, ["PMI-1"]);
}

#[test]
fn withdraw_is_for_the_author_or_the_owner_and_only_while_pending() {
    // Given two sessions and an item from A-1 plus one from the owner.
    let (_tmp, store) = store();
    store.create_assistant("pi", None, None, None).unwrap();
    store.create_assistant("pi", None, None, None).unwrap();
    store.pm_inbox_post(new("assistant:A-1", None)).unwrap();
    store.pm_inbox_post(new("owner", None)).unwrap();
    // When A-2 tries to withdraw A-1's item, then it refuses and nothing changes.
    let before = store.pm_inbox_item("PMI-1").unwrap();
    let error = store
        .pm_inbox_withdraw("PMI-1", 0, "assistant:A-2", "")
        .unwrap_err();
    assert_eq!(code(&error), "not_author");
    assert_eq!(store.pm_inbox_item("PMI-1").unwrap(), before);
    // When the author withdraws its claimed item, then it is withdrawn at the next revision.
    store.pm_inbox_claim("PMI-1", 0, None).unwrap();
    let withdrawn = store
        .pm_inbox_withdraw("PMI-1", 1, "assistant:A-1", "owner answered it")
        .unwrap();
    assert_eq!(
        (withdrawn.revision, withdrawn.state),
        (2, ItemState::Withdrawn)
    );
    assert_eq!(withdrawn.withdrawn.unwrap().by, "assistant:A-1");
    assert!(withdrawn.notice.is_none());
    // When anyone moves a withdrawn item again, then it refuses on its state.
    let error = store.pm_inbox_claim("PMI-1", 2, None).unwrap_err();
    assert_eq!(code(&error), "item_state");
    // When the owner withdraws the owner's item, then it is withdrawn.
    let owner = store.pm_inbox_withdraw("PMI-2", 0, "owner", "").unwrap();
    assert_eq!(owner.state, ItemState::Withdrawn);
}

#[test]
fn resolve_needs_a_claim_and_posts_refuse_other_authors_and_ended_sessions() {
    let (_tmp, store) = store();
    store.create_assistant("pi", None, None, None).unwrap();
    store.pm_inbox_post(new("owner", None)).unwrap();
    // An open item cannot be resolved without a claim.
    let error = store
        .pm_inbox_resolve("PMI-1", 0, (Resolution::Done, "done", vec![]), None)
        .unwrap_err();
    assert_eq!(code(&error), "item_state");
    // Only the owner and open assistant sessions post.
    let error = store.pm_inbox_post(new("pm", None)).unwrap_err();
    assert_eq!(code(&error), "not_author");
    store.end_assistant("A-1").unwrap();
    let error = store.pm_inbox_post(new("assistant:A-1", None)).unwrap_err();
    assert_eq!(code(&error), "assistant_ended");
    // An owner-authored item owes no notice when resolved.
    store.pm_inbox_claim("PMI-1", 0, None).unwrap();
    let resolved = store
        .pm_inbox_resolve(
            "PMI-1",
            1,
            (Resolution::EscalatedToOwner, "Needs you.", vec![]),
            None,
        )
        .unwrap();
    assert!(resolved.notice.is_none());
    assert!(store.pm_inbox_publish_notice("PMI-1").unwrap().is_none());
    assert_eq!(store.pm_inbox_items().unwrap().len(), 1);
    assert_eq!(
        code(&store.pm_inbox_item("PMI-9").unwrap_err()),
        "unknown_item"
    );
}

#[test]
fn a_retry_key_returns_the_authors_earlier_item() {
    let (_tmp, store) = store();
    store.create_assistant("pi", None, None, None).unwrap();
    let (first, created) = store
        .pm_inbox_post(new("assistant:A-1", Some("retry-1")))
        .unwrap();
    let (again, repeated) = store
        .pm_inbox_post(new("assistant:A-1", Some("retry-1")))
        .unwrap();
    let (owner, owner_created) = store.pm_inbox_post(new("owner", Some("retry-1"))).unwrap();
    assert!(created && !repeated && owner_created);
    assert_eq!(again, first);
    assert_eq!(owner.id, "PMI-2");
    assert_eq!(events(&store, "pm_inbox_posted").len(), 2);
}

#[test]
fn the_pm_wake_for_an_open_item_is_published_once() {
    let (_tmp, store) = store();
    store.pm_inbox_post(new("owner", None)).unwrap();
    let first = store.pm_inbox_publish_wake("PMI-1").unwrap().unwrap();
    assert!(store.pm_inbox_publish_wake("PMI-1").unwrap().is_none());
    let inbox = Inbox::new(&store, Recipient::Pm).unwrap();
    assert_eq!(inbox.messages().unwrap(), vec![first.clone()]);
    assert_eq!(first.subject.unwrap()["key"], "pmi:PMI-1:0");
    assert_eq!(
        store.pm_inbox_item("PMI-1").unwrap().pm_wake,
        Some(first.id)
    );
}

#[test]
fn assistant_recipients_have_a_canonical_inbox_key_and_no_task() {
    let recipient = Recipient::Assistant {
        session: "A-12".into(),
    };
    assert_eq!(recipient.key().unwrap(), "assistant_A-12");
    assert_eq!(Recipient::parse_key("assistant_A-12").unwrap(), recipient);
    assert!(recipient.task().is_none());
    assert_eq!(
        RequestedAction::parse("move T-3 dropped --force").unwrap(),
        RequestedAction {
            kind: "move".into(),
            target: Some("T-3".into()),
            args: vec!["dropped".into(), "--force".into()],
        }
    );
    assert!(RequestedAction::parse("Approve T-5").is_err());
}
