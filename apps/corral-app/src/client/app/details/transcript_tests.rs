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
fn changed_transcript_refresh_revalidates_loaded_history_without_splicing_branches() {
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
        page("v2", &[("3", &["new leaf"])], Some("new-older")),
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
            ("3".to_owned(), vec!["new leaf".to_owned()])
        ]
    );
}
