use super::*;

#[test]
fn aggregate_retention_plateaus_without_evicting_readers_or_unresolved_sends() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.ready = true;
    app.scopes.insert("active".into());
    app.session_scopes.insert("session".into(), 1);
    let selected = DetailRef::Plan {
        id: "selected".into(),
    };
    app.watch_detail(feed, selected.clone(), true);
    app.detail_request(
        feed,
        "plan-read".into(),
        DetailGet {
            target: selected.clone(),
            etag: None,
            page: None,
        },
    );
    let history = DetailRef::Session {
        id: "session".into(),
    };
    let mut outgoing = app.detail_request(
        feed,
        "history-read".into(),
        DetailGet {
            target: history.clone(),
            etag: None,
            page: Some("older-cursor".into()),
        },
    );
    let (_, params, tag) = outgoing.remove(0);
    let body = DetailBody::Session(SessionPage {
        id: "session".into(),
        role: "worker".into(),
        task: None,
        run: None,
        machine: None,
        messages: vec![],
        older: None,
        replyable: false,
        reply_refusal: None,
    });
    app.detail_reply(
        feed,
        tag,
        params,
        Ok(value(DetailReply {
            etag: "old-page".into(),
            as_of: "now".into(),
            seq: 1,
            next: None,
            body: Some(body),
        })),
    )
    .unwrap();
    let history_key = detail_key(&history);
    feed.with_untracked(|feed| {
        let detail = &feed.details[&history_key];
        let payload = bytes(&detail.pages) + bytes(&detail.merged);
        assert!(app.retained.entries[&Key::Detail(history_key.clone())].1 >= payload);
    });
    feed.update(|feed| {
        for scope in ["active", "pending"] {
            feed.threads
                .insert(Id::new(scope), ThreadSnapshot::default());
        }
        feed.live.insert(Id::new("session"), Default::default());
        feed.items
            .insert("item".into(), ItemFetch::Failed("retained".into()));
        feed.commands.insert(
            Id::new("pending-send"),
            CommandState {
                device: None,
                method: ASSISTANT_SEND.into(),
                request: json!({"thread":"pending"}),
                status: CommandStatus::Done,
                error: None,
                entry: Some(Id::new("not-yet-echoed")),
            },
        );
    });
    app.touch_thread(feed, "active");
    app.touch_thread(feed, "pending");
    app.touch_session(feed, "session");
    app.touch_item(feed, "session", "item");
    for index in 0..MAX_ENTRIES * 2 {
        let scope = format!("closed-{index}");
        app.scopes.insert(scope.clone());
        feed.update(|feed| {
            feed.threads
                .insert(Id::from(&scope), ThreadSnapshot::default());
            feed.topics.insert(
                topic_key(Topic::Thread, Some(&scope)),
                TopicState::default(),
            );
        });
        app.touch_thread(feed, &scope);
        app.scopes.remove(&scope);
        app.trim_retained(feed);
        assert!(app.retained.entries.len() <= MAX_ENTRIES);
    }
    feed.with_untracked(|feed| {
        assert!(feed.threads.contains_key("active"));
        assert!(feed.threads.contains_key("pending"));
        assert!(!feed.threads.contains_key("closed-0"));
        assert!(
            !feed
                .topics
                .contains_key(&topic_key(Topic::Thread, Some("closed-0")))
        );
        assert!(feed.details.contains_key(&detail_key(&selected)));
        assert!(feed.details.contains_key(&history_key));
        assert!(feed.live.contains_key("session"));
        assert!(feed.items.contains_key("item"));
    });
    let (oldest, next_oldest) = {
        let mut inactive = feed.with_untracked(|feed| {
            app.retained
                .entries
                .iter()
                .filter(|(key, _)| !app.retained_pinned(key, feed))
                .map(|(key, (stamp, _))| (*stamp, key.clone()))
                .collect::<Vec<_>>()
        });
        inactive.sort_unstable_by_key(|(stamp, _)| *stamp);
        (inactive[0].1.clone(), inactive[1].1.clone())
    };
    app.retained.touch(oldest.clone(), 1);
    app.retained.touch(Key::Thread("newest".into()), 1);
    app.trim_retained(feed);
    assert!(app.retained.entries.contains_key(&oldest));
    assert!(!app.retained.entries.contains_key(&next_oldest));
    // A byte-heavy inactive entry consumes the same budget as every other cache kind.
    let last_scope = format!("closed-{}", MAX_ENTRIES * 2 - 1);
    app.retained
        .touch(Key::Thread(last_scope.clone()), MAX_BYTES + 1);
    app.trim_retained(feed);
    assert!(
        app.retained
            .entries
            .values()
            .map(|(_, size)| size)
            .sum::<usize>()
            <= MAX_BYTES
    );
    assert!(feed.with_untracked(|feed| !feed.threads.contains_key(last_scope.as_str())));
    app.disconnected(feed);
    app.ready = true;
    let refreshed = app.refresh_details(feed);
    assert!(refreshed.iter().any(|(_, request, _)| {
        request["kind"] == "session" && request["page"] == "older-cursor"
    }));
}
