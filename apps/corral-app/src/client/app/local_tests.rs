use super::*;
use corral_protocol::session_state::SendPolicy;

fn send(session: &str, id: &str, text: &str) -> SessionSend {
    SessionSend {
        id: session.into(),
        cmd_id: id.into(),
        text: text.into(),
        policy: SendPolicy::Steer,
        to: None,
        typed_at: Some("2026-11-15T10:00:00Z".into()),
        attachments: vec![],
    }
}

/// A message the outbox takes retires its own session's saved draft in the same durable write,
/// and nothing else: another session's draft stays, a draft typed after the words left is not
/// consumed by a later message, and a message the outbox refuses leaves the draft alone.
#[test]
fn an_accepted_send_retires_only_its_own_draft() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.outbox = Some(Outbox::load("draft-send-handoff", "draft-device").unwrap());
    let save = |app: &mut AppState, scope: &str, text: &str| {
        app.request(
            feed,
            ClientRequest::SaveDraft {
                thread: scope.into(),
                text: text.into(),
            },
        );
    };
    save(&mut app, "A-3", "  owner");
    save(&mut app, "A-4", "another session's draft");
    let first = send("A-3", "one-owner-intent", "owner words");
    assert!(
        app.request(feed, ClientRequest::SessionSend(first.clone()))
            .is_empty(),
        "offline, the message waits in the outbox"
    );
    assert!(feed.with_untracked(|feed| !feed.drafts.contains_key("A-3")));
    assert_eq!(
        feed.with_untracked(|feed| feed.drafts["A-4"].clone()),
        "another session's draft"
    );
    assert_eq!(
        feed.with_untracked(|feed| feed.commands["one-owner-intent"].status),
        CommandStatus::Queued
    );
    let persisted = Outbox::load("draft-send-handoff", "draft-device").unwrap();
    assert!(!persisted.saved.drafts.contains_key("A-3"));
    assert_eq!(persisted.saved.drafts["A-4"], "another session's draft");
    assert_eq!(persisted.saved.messages["one-owner-intent"].request, first);
    assert!(!persisted.saved.messages["one-owner-intent"].attempted);

    // Words typed after the first message are not that message's draft.
    save(&mut app, "A-3", "a new thought");
    let second = send("A-3", "two-owner-intent", "owner words again");
    assert!(
        app.request(feed, ClientRequest::SessionSend(second))
            .is_empty()
    );
    let persisted = Outbox::load("draft-send-handoff", "draft-device").unwrap();
    assert_eq!(persisted.saved.messages.len(), 2);
    assert_eq!(persisted.saved.drafts["A-3"], "a new thought");

    // A refused message never takes the draft with it.
    save(&mut app, "A-5", "keep this draft");
    assert!(
        app.request(
            feed,
            ClientRequest::SessionSend(send("A-5", "invalid-intent", " \n"))
        )
        .is_empty()
    );
    let persisted = Outbox::load("draft-send-handoff", "draft-device").unwrap();
    assert_eq!(persisted.saved.drafts["A-5"], "keep this draft");
    assert!(!persisted.saved.messages.contains_key("invalid-intent"));
    assert!(feed.with_untracked(|feed| feed.operations["invalid-intent"].error.is_some()));
}

/// Words sent while offline survive a restart, and the restarted client still takes more: both
/// wait in the outbox and are shown queued until a connection sends them.
#[test]
fn restarted_offline_client_shows_queued_words_and_accepts_new_words() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut first = AppState::new();
    first.outbox = Some(Outbox::load("offline-restart", "device").unwrap());
    assert!(
        first
            .request(
                feed,
                ClientRequest::SessionSend(send(QUICK_THREAD, "first", "words first"))
            )
            .is_empty()
    );
    drop(first);
    let offline = signal(Feed::default());
    offline.update(|feed| {
        feed.link = Link::Down;
    });
    let mut restarted = AppState::new();
    restarted.outbox = Some(Outbox::load("offline-restart", "device").unwrap());
    restarted.project_local(offline);
    assert_eq!(
        offline.with_untracked(|feed| feed.commands["first"].status),
        CommandStatus::Queued
    );
    assert!(
        restarted
            .request(
                offline,
                ClientRequest::SessionSend(send(QUICK_THREAD, "second", "words second"))
            )
            .is_empty()
    );
    assert_eq!(
        offline.with_untracked(|feed| feed.commands["second"].status),
        CommandStatus::Queued
    );
    let saved = Outbox::load("offline-restart", "device").unwrap().saved;
    assert!(saved.messages.contains_key("first"));
    assert!(saved.messages.contains_key("second"));
    assert!(offline.with_untracked(|feed| !feed.ready() && feed.link == Link::Down));
}
