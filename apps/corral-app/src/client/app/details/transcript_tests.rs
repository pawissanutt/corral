use super::*;

fn page(etag: &str, rows: &[(&str, &[&str])], older: Option<&str>) -> Value {
    value(DetailReply {
        etag: etag.into(),
        as_of: "now".into(),
        seq: 1,
        next: older.map(str::to_owned),
        body: Some(DetailBody::Session(SessionPage {
            id: "B-1".into(),
            role: "build".into(),
            task: None,
            run: None,
            machine: None,
            messages: rows
                .iter()
                .map(|(id, words)| SessionMessage {
                    id: (*id).into(),
                    author: "assistant".into(),
                    at: None,
                    items: words
                        .iter()
                        .map(|word| SessionItem::Text {
                            text: (*word).into(),
                        })
                        .collect(),
                    model: None,
                    stop_reason: None,
                    usage: None,
                    input: None,
                    attachments: vec![],
                })
                .collect(),
            older: older.map(str::to_owned),
            replyable: true,
            reply_refusal: None,
            host: Default::default(),
        })),
    })
}

fn read(app: &mut AppState, feed: Signal<Feed>, page: Option<&str>) -> Vec<Outgoing> {
    app.detail_request(
        feed,
        requests::new_id(),
        DetailGet {
            target: target(),
            etag: None,
            page: page.map(str::to_owned),
        },
    )
}

fn target() -> DetailRef {
    DetailRef::Session { id: "B-1".into() }
}

fn reply(
    app: &mut AppState,
    feed: Signal<Feed>,
    outgoing: Vec<Outgoing>,
    cursor: Option<&str>,
    value: Value,
) -> Vec<Outgoing> {
    let [(_, params, tag)]: [Outgoing; 1] = outgoing.try_into().unwrap();
    assert_eq!(params.get("page").and_then(Value::as_str), cursor);
    app.detail_reply(feed, tag, params, Ok(value)).unwrap()
}

fn shown(feed: Signal<Feed>) -> Vec<(String, Vec<String>)> {
    feed.with_untracked(|feed| {
        let Some(DetailBody::Session(page)) = feed.detail(&target()).and_then(DetailCache::body)
        else {
            panic!("a served transcript");
        };
        page.messages
            .iter()
            .map(|message| {
                (
                    message.id.clone(),
                    message
                        .items
                        .iter()
                        .map(|item| match item {
                            SessionItem::Text { text } => text.clone(),
                            _ => panic!("text item"),
                        })
                        .collect(),
                )
            })
            .collect()
    })
}

#[test]
fn transcript_first_paint_waits_for_no_older_page_and_prepend_joins_split_records() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.ready = true;
    let get = read(&mut app, feed, None);
    assert!(
        reply(
            &mut app,
            feed,
            get,
            None,
            page("v1", &[("2", &["end"])], Some("older"))
        )
        .is_empty()
    );
    assert_eq!(shown(feed), [("2".to_owned(), vec!["end".to_owned()])]);
    let get = read(&mut app, feed, Some("older"));
    assert!(
        reply(
            &mut app,
            feed,
            get,
            Some("older"),
            page(
                "v1",
                &[("1", &["old"]), ("2", &["start"])],
                Some("earliest")
            )
        )
        .is_empty()
    );
    assert_eq!(
        shown(feed),
        [
            ("1".to_owned(), vec!["old".to_owned()]),
            ("2".to_owned(), vec!["start".to_owned(), "end".to_owned()])
        ]
    );
}

#[test]
fn latest_page_refresh_does_not_chase_a_displaced_record() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.ready = true;
    let get = read(&mut app, feed, None);
    reply(
        &mut app,
        feed,
        get,
        None,
        page("v1", &[("1", &["old"])], Some("older")),
    );
    let get = read(&mut app, feed, None);
    let next = reply(
        &mut app,
        feed,
        get,
        None,
        page("v2", &[("2", &["new"])], Some("v2-older")),
    );
    assert!(next.is_empty());
    assert_eq!(shown(feed), [("2".into(), vec!["new".into()])]);
}

#[test]
fn changed_transcript_refresh_revalidates_loaded_history_before_publishing() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.ready = true;
    let get = read(&mut app, feed, None);
    reply(
        &mut app,
        feed,
        get,
        None,
        page("v1", &[("2", &["old leaf"])], Some("older")),
    );
    let get = read(&mut app, feed, Some("older"));
    reply(
        &mut app,
        feed,
        get,
        Some("older"),
        page("v1", &[("1", &["old prefix"])], None),
    );
    let before = shown(feed);

    let get = read(&mut app, feed, None);
    assert_eq!(get[0].1["etag"], "v1");
    let unchanged = value(DetailReply {
        etag: "v1".into(),
        as_of: "later".into(),
        seq: 2,
        next: None,
        body: None,
    });
    assert!(reply(&mut app, feed, get, None, unchanged).is_empty());
    assert_eq!(shown(feed), before);

    let get = read(&mut app, feed, None);
    let get = reply(
        &mut app,
        feed,
        get,
        None,
        page("v2", &[("2", &["new leaf"])], Some("new-older")),
    );
    assert_eq!(
        shown(feed),
        before,
        "keep displaying the old view while checking its depth"
    );
    assert!(
        reply(
            &mut app,
            feed,
            get,
            Some("new-older"),
            page("v2", &[("1", &["current prefix"])], None)
        )
        .is_empty()
    );
    assert_eq!(
        shown(feed),
        [
            ("1".to_owned(), vec!["current prefix".to_owned()]),
            ("2".to_owned(), vec!["new leaf".to_owned()])
        ]
    );
}

#[test]
fn a_moving_seam_cannot_merge_away_a_previously_shown_anchor() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.ready = true;
    let newest = &[("2", &["seam reply"][..]), ("3", &["tail reply"][..])];
    let older = &[("1", &["earlier reply"][..])];
    let get = read(&mut app, feed, None);
    reply(&mut app, feed, get, None, page("v1", newest, Some("older")));
    let get = read(&mut app, feed, Some("older"));
    reply(&mut app, feed, get, Some("older"), page("v1", older, None));
    let anchors = || {
        use crate::model::session::{Ui, transcript::transcript};
        feed.with_untracked(|feed| {
            transcript(&Id::from("B-1"), &Ui::EMPTY, feed.detail(&target()), feed)
                .messages
                .into_iter()
                .map(|message| message.anchor)
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(anchors(), ["1", "2"]);
    let before = shown(feed);

    // Every old record and item still fits, but the new page starts at 3 instead of 2.
    // Replaying the new chain would merge the previous anchor 2 into message 1.
    let get = read(&mut app, feed, None);
    let get = reply(
        &mut app,
        feed,
        get,
        None,
        page("v2", &newest[1..], Some("new-older")),
    );
    assert!(
        reply(
            &mut app,
            feed,
            get,
            Some("new-older"),
            page("v2", &[older[0], newest[0]], None),
        )
        .is_empty()
    );
    assert_eq!(shown(feed), before);
    assert_eq!(anchors(), ["1", "2"]);
    feed.with_untracked(|feed| {
        let cache = feed.detail(&target()).unwrap();
        assert_eq!(cache.error.as_ref().unwrap().code, HISTORY_CHANGED);
        assert!(cache.pages.iter().all(|page| page.etag == "v1"));
    });

    let get = app.latest_transcript(feed, "B-1".into());
    assert!(
        reply(
            &mut app,
            feed,
            get,
            None,
            page("v2", &newest[1..], Some("new-older")),
        )
        .is_empty()
    );
    assert_eq!(anchors(), ["3"]);
    feed.with_untracked(|feed| assert!(feed.detail(&target()).unwrap().error.is_none()));
}

fn numbered_page(etag: &str, row: usize, has_older: bool) -> Value {
    let cursor = has_older.then(|| format!("{etag}:{}", row.saturating_sub(1)));
    page(
        etag,
        &[(&row.to_string(), &["recorded"])],
        cursor.as_deref(),
    )
}

#[test]
fn appending_to_a_deep_transcript_keeps_the_old_snapshot_until_its_depth_is_restored() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.ready = true;
    for row in (1..=10).rev() {
        let cursor = (row < 10).then(|| format!("v1:{row}"));
        let get = read(&mut app, feed, cursor.as_deref());
        assert!(
            reply(
                &mut app,
                feed,
                get,
                cursor.as_deref(),
                numbered_page("v1", row, true),
            )
            .is_empty()
        );
    }
    let before = shown(feed);
    assert_eq!(before.first().unwrap().0, "1");
    let mut get = read(&mut app, feed, None);
    for row in (1..=11).rev() {
        let cursor = (row < 11).then(|| format!("v2:{row}"));
        get = reply(
            &mut app,
            feed,
            get,
            cursor.as_deref(),
            numbered_page("v2", row, true),
        );
        if row != 1 {
            assert_eq!(
                shown(feed),
                before,
                "must not publish a partial new snapshot"
            );
        }
    }
    assert!(get.is_empty());
    assert_eq!(&shown(feed)[..10], before);
    assert_eq!(shown(feed).last().unwrap().0, "11");
    feed.with_untracked(|feed| {
        let cache = feed.detail(&target()).unwrap();
        assert!(cache.error.is_none());
        assert!(cache.pages.iter().all(|page| page.etag == "v2"));
    });
}

#[test]
fn an_unreconciled_snapshot_survives_the_page_cap_until_explicit_latest() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.ready = true;
    let get = read(&mut app, feed, None);
    assert!(
        reply(
            &mut app,
            feed,
            get,
            None,
            page(
                "v1",
                &[("old branch", &["kept for reading"])],
                Some("old cursor")
            ),
        )
        .is_empty()
    );
    let get = read(&mut app, feed, Some("old cursor"));
    reply(
        &mut app,
        feed,
        get,
        Some("old cursor"),
        page("v1", &[("earlier", &["earlier"])], Some("earliest")),
    );
    let before = shown(feed);
    let mut get = read(&mut app, feed, None);
    for row in (1..=MAX_PAGES).rev() {
        let cursor = (row < MAX_PAGES).then(|| format!("v2:{row}"));
        get = reply(
            &mut app,
            feed,
            get,
            cursor.as_deref(),
            numbered_page("v2", row, true),
        );
    }
    assert!(
        get.is_empty(),
        "no continuation once the bounded rebuild fails"
    );
    assert_eq!(shown(feed), before);
    let error_code = || {
        feed.with_untracked(|feed| {
            feed.detail(&target())
                .unwrap()
                .error
                .as_ref()
                .map(|error| error.code.clone())
        })
    };
    assert_eq!(error_code().as_deref(), Some(HISTORY_CHANGED));
    assert!(read(&mut app, feed, Some("earliest")).is_empty());

    let get = app.latest_transcript(feed, "B-1".into());
    let [(_, params, tag)]: [Outgoing; 1] = get.try_into().unwrap();
    assert!(params.get("etag").is_none_or(Value::is_null));
    assert!(app.latest_transcript(feed, "B-1".into()).is_empty());
    assert!(
        app.detail_reply(
            feed,
            tag,
            params,
            Err((ErrorCode::Conflict, "raced".into()))
        )
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        error_code().as_deref(),
        Some(HISTORY_CHANGED),
        "explicit latest remains retryable"
    );
    assert_eq!(shown(feed), before);

    let get = app.latest_transcript(feed, "B-1".into());
    assert!(reply(&mut app, feed, get, None, numbered_page("v3", 3, true)).is_empty());
    assert_eq!(shown(feed)[0].0, "3");
    assert_eq!(shown(feed).len(), 1);
    assert_eq!(error_code(), None);
}

#[test]
fn a_raced_reconciliation_does_not_restart_or_mix_snapshots() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.ready = true;
    let get = read(&mut app, feed, None);
    assert!(reply(&mut app, feed, get, None, numbered_page("v1", 1, true)).is_empty());
    let before = shown(feed);

    let get = read(&mut app, feed, Some("v1:0"));
    let [(_, params, tag)]: [Outgoing; 1] = get.try_into().unwrap();
    let get = app
        .detail_reply(
            feed,
            tag,
            params,
            Err((ErrorCode::Conflict, "raced".into())),
        )
        .unwrap();
    let get = reply(&mut app, feed, get, None, numbered_page("v2", 2, true));
    assert!(
        read(&mut app, feed, None).is_empty(),
        "do not supersede the staged read"
    );
    assert!(
        reply(
            &mut app,
            feed,
            get,
            Some("v2:1"),
            numbered_page("v3", 1, true)
        )
        .is_empty()
    );
    assert_eq!(shown(feed), before);
    feed.with_untracked(|feed| {
        let cache = feed.detail(&target()).unwrap();
        assert_eq!(cache.error.as_ref().unwrap().code, "conflict");
        assert!(!cache.loading);
        assert!(cache.pages.iter().all(|page| page.etag == "v1"));
    });
}
