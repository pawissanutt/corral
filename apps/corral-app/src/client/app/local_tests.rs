use super::*;

#[test]
fn restarted_offline_client_shows_queued_words_accepts_new_words_and_cancels() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let make = |id: &str| AssistantSend {
        cmd_id: id.into(),
        thread: QUICK_THREAD.into(),
        target: AskTarget::Assistant,
        text: format!("words {id}"),
        typed_at: "2026-11-15T10:00:00Z".into(),
        context: vec![],
        attachments: vec![],
    };
    let mut first = AppState::new();
    first.outbox = Some(Outbox::load("offline-restart", "device").unwrap());
    assert!(
        first
            .request(feed, ClientRequest::AssistantSend(make("first")))
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
            .request(offline, ClientRequest::AssistantSend(make("second")))
            .is_empty()
    );
    assert_eq!(
        offline.with_untracked(|feed| feed.commands["second"].status),
        CommandStatus::Queued
    );
    assert!(
        restarted
            .request(offline, ClientRequest::AssistantCancel("first".into()))
            .is_empty()
    );
    let saved = Outbox::load("offline-restart", "device").unwrap().saved;
    assert!(!saved.messages.contains_key("first"));
    assert!(saved.messages.contains_key("second"));
    assert!(saved.cancelled.contains("first"));
    assert!(offline.with_untracked(|feed| !feed.ready() && feed.link == Link::Down));
}
