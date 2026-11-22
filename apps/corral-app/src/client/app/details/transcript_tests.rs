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
fn mounted_readers_protect_a_rolling_first_page_until_the_last_release() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.ready = true;
    let reading = |app: &mut AppState, scope: &str, active| {
        assert!(
            app.request(
                feed,
                ClientRequest::TranscriptReading {
                    scope: scope.into(),
                    active,
                }
            )
            .is_empty(),
            "reading is a local intent"
        );
    };
    reading(&mut app, "B-1", true);
    let get = read(&mut app, feed, None);
    assert!(
        reply(&mut app, feed, get, None, numbered_page("v1", 2, true)).is_empty(),
        "even a resumed reader paints after one page"
    );
    let before = shown(feed);

    reading(&mut app, "B-1", true);
    reading(&mut app, "B-1", false);
    reading(&mut app, "B-1/child/a2", true);
    let get = read(&mut app, feed, None);
    let older = reply(&mut app, feed, get, None, numbered_page("v2", 3, true));
    assert_eq!(
        shown(feed),
        before,
        "one view is still reading the first page"
    );
    assert!(
        reply(
            &mut app,
            feed,
            older,
            Some("v2:2"),
            numbered_page("v2", 2, true)
        )
        .is_empty()
    );
    assert_eq!(shown(feed).first(), before.first());
    assert_eq!(anchors(feed), ["2", "3"]);

    reading(&mut app, "B-1", false);
    assert!(!app.transcript_readers.contains_key("B-1"));
    let get = read(&mut app, feed, None);
    assert!(reply(&mut app, feed, get, None, numbered_page("v3", 4, true)).is_empty());
    assert_eq!(
        shown(feed),
        [("4".into(), vec!["recorded".into()])],
        "auto-reconstructed depth does not pin a follower; a child is a different scope"
    );
    reading(&mut app, "B-1/child/a2", false);
    assert!(app.transcript_readers.is_empty());
}

#[test]
fn an_uncovered_cut_on_the_first_page_can_rejoin_at_the_tail() {
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
        page(
            "v1",
            &[("two", &["first item", "second item"])],
            Some("earlier"),
        ),
    );
    let before = shown(feed);

    let reading = |app: &mut AppState, active| app.transcript_reading(feed, "B-1".into(), active);
    reading(&mut app, true);
    let get = read(&mut app, feed, None);
    let latest = || page("v2", &[("two", &["second item"])], None);
    assert!(reply(&mut app, feed, get, None, latest()).is_empty());
    assert_eq!(
        shown(feed),
        before,
        "the same ID does not cover a missing item"
    );
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
    assert!(
        app.transcript_tail(feed, "B-1".into()).is_empty(),
        "a second view at the tail cannot displace the active reader"
    );

    reading(&mut app, false);
    let tail = |app: &mut AppState| {
        app.request(
            feed,
            ClientRequest::TranscriptTail {
                scope: "B-1".into(),
            },
        )
    };
    let [(_, params, tag)]: [Outgoing; 1] = tail(&mut app).try_into().unwrap();
    app.detail_reply(
        feed,
        tag,
        params,
        Err((ErrorCode::Conflict, "raced".into())),
    )
    .unwrap();
    assert_eq!(error_code().as_deref(), Some(HISTORY_CHANGED));
    let get = tail(&mut app);
    assert!(reply(&mut app, feed, get, None, latest()).is_empty());
    assert_eq!(shown(feed), [("two".into(), vec!["second item".into()])]);
    assert!(feed.with_untracked(|feed| feed.detail(&target()).unwrap().error.is_none()));
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

fn anchors(feed: Signal<Feed>) -> Vec<String> {
    use crate::model::session::{Ui, transcript::transcript};
    feed.with_untracked(|feed| {
        transcript(&Id::from("B-1"), &Ui::EMPTY, feed.detail(&target()), feed)
            .messages
            .into_iter()
            .map(|message| message.anchor)
            .collect()
    })
}

#[test]
fn append_with_a_moved_seam_keeps_anchors_and_can_read_older() {
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
    reply(
        &mut app,
        feed,
        get,
        Some("older"),
        page("v1", older, Some("earlier")),
    );
    assert_eq!(anchors(feed), ["1", "2"]);

    // Every old record and item still fits, but the new page starts at 3 instead of 2.
    // Replaying the new chain would merge the previous anchor 2 into message 1.
    let get = read(&mut app, feed, None);
    let get = reply(
        &mut app,
        feed,
        get,
        None,
        page(
            "v2",
            &[newest[1], ("4", &["live append"])],
            Some("new-older"),
        ),
    );
    assert!(
        reply(
            &mut app,
            feed,
            get,
            Some("new-older"),
            page("v2", &[older[0], newest[0]], Some("new-earliest")),
        )
        .is_empty()
    );
    assert_eq!(shown(feed).last().unwrap().0, "4");
    assert_eq!(anchors(feed), ["1", "2"]);
    feed.with_untracked(|feed| {
        use crate::model::session::Ui;
        let cache = feed.detail(&target()).unwrap();
        assert!(cache.error.is_none());
        assert!(cache.pages.iter().all(|page| page.etag == "v2"));
        let view = replay::transcript(&Id::from("B-1"), &Ui::EMPTY, Some(cache), feed);
        assert!(
            view.messages[1].keys.iter().any(|key| key == "3|1"),
            "the moved cut must not split off an old item inside the held bubble"
        );
    });
    let get = read(&mut app, feed, Some("new-earliest"));
    assert!(
        reply(
            &mut app,
            feed,
            get,
            Some("new-earliest"),
            page("v2", &[("0", &["older still"])], None)
        )
        .is_empty()
    );
    assert_eq!(anchors(feed), ["0", "1", "2"]);
}

fn with_tool(mut page: Value, row: usize, part: &str) -> Value {
    use corral_protocol::detail::SessionToolState;
    let body = &mut page["body"]["body"]["messages"][row];
    if part == "result" {
        body["author"] = value("toolResult");
        body["items"] = Value::Array(vec![]);
    }
    body["items"]
        .as_array_mut()
        .unwrap()
        .push(value(SessionItem::ToolCall {
            call_id: "call-1".into(),
            name: "bash".into(),
            arguments: if part == "result" { "" } else { "{}" }.into(),
            summary: String::new(),
            state: if part == "call" {
                SessionToolState::Running
            } else {
                SessionToolState::Done
            },
            paired: part == "paired",
            call: (part != "result").then(|| "call JSON".into()),
            result: (part != "call").then(|| "result JSON".into()),
            output: (part != "call").then(|| "answer".into()),
            elapsed_ms: None,
            diff: None,
            content: None,
        }));
    page
}

#[test]
fn a_continued_tool_needs_no_extra_history_when_its_result_moves() {
    let part = |stamp, id, kind, continued| {
        let mut wire = with_tool(page(stamp, &[(id, &[])], Some("more history")), 0, kind);
        if continued {
            wire["body"]["body"]["messages"][0]["items"]
                .as_array_mut()
                .unwrap()
                .push(value(SessionItem::Continuation {
                    token: "fragment".into(),
                    bytes: 3000,
                }));
        }
        decode::<DetailReply>(wire).unwrap()
    };
    let paired = part("old", "call", "paired", true);
    let held = DetailCache {
        target: target(),
        transcript_seams: Default::default(),
        merged: paired.body.clone(),
        pages: vec![paired],
        loading: false,
        stale: false,
        error: None,
    };
    let mut pages = vec![part("fresh", "result", "result", true)];
    assert!(history_missing(&held, &pages, session(&pages[0]).unwrap()));
    pages.push(part("fresh", "call", "call", false));
    assert!(!history_missing(&held, &pages, session(&pages[0]).unwrap()));
}

#[test]
fn regrouped_tool_result_keeps_its_anchor_and_can_read_older() {
    use crate::model::session::{
        Ui,
        transcript::{MessageItem, transcript},
    };
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
        with_tool(
            page(
                "v1",
                &[("result", &[]), ("next", &["reply"])],
                Some("older"),
            ),
            0,
            "result",
        ),
    );
    let get = read(&mut app, feed, Some("older"));
    reply(
        &mut app,
        feed,
        get,
        Some("older"),
        with_tool(
            page("v1", &[("call", &["start"])], Some("earliest")),
            0,
            "call",
        ),
    );
    assert_eq!(anchors(feed), ["call", "result", "next"]);

    let get = read(&mut app, feed, None);
    let get = reply(
        &mut app,
        feed,
        get,
        None,
        page(
            "v2",
            &[("next", &["reply"]), ("append", &["append"])],
            Some("new-older"),
        ),
    );
    assert!(
        reply(
            &mut app,
            feed,
            get,
            Some("new-older"),
            with_tool(
                page("v2", &[("call", &["start"])], Some("new-earliest")),
                0,
                "paired"
            )
        )
        .is_empty()
    );
    let verify = || {
        feed.with_untracked(|feed| {
            let cache = feed.detail(&target()).unwrap();
            assert!(cache.error.is_none());
            assert!(cache.pages.iter().all(|page| page.etag == "v2"));
            let raw = replay::page(Some(cache)).unwrap();
            assert!(
                raw.messages.iter().all(|m| m.id != "result"),
                "never reconstruct raw history"
            );
            let view = transcript(&Id::from("B-1"), &Ui::EMPTY, Some(cache), feed);
            let tools: Vec<_> = view
                .messages
                .iter()
                .flat_map(|m| {
                    m.items.iter().filter_map(|item| match item {
                        MessageItem::Tool(tool) => Some((m.anchor.as_str(), tool.state)),
                        _ => None,
                    })
                })
                .collect();
            assert_eq!(tools, [("result", replay::ToolState::Ok)]);
        })
    };
    assert_eq!(anchors(feed), ["call", "result", "next"]);
    verify();
    let get = read(&mut app, feed, Some("new-earliest"));
    assert!(
        reply(
            &mut app,
            feed,
            get,
            Some("new-earliest"),
            page("v2", &[("older", &["earlier still"])], None)
        )
        .is_empty()
    );
    assert_eq!(anchors(feed), ["older", "call", "result", "next"]);
    verify();
}

#[test]
fn a_rewrite_with_a_lost_seam_keeps_the_previous_snapshot() {
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
        page("v1", &[("2", &["seam"]), ("3", &["tail"])], Some("older")),
    );
    let get = read(&mut app, feed, Some("older"));
    reply(
        &mut app,
        feed,
        get,
        Some("older"),
        page("v1", &[("1", &["earlier"])], None),
    );
    let before = shown(feed);
    let get = read(&mut app, feed, None);
    let get = reply(
        &mut app,
        feed,
        get,
        None,
        page("v2", &[("3", &["tail"])], Some("new-older")),
    );
    assert!(
        reply(
            &mut app,
            feed,
            get,
            Some("new-older"),
            page("v2", &[("1", &["earlier"]), ("2", &["rewritten"])], None)
        )
        .is_empty()
    );
    assert_eq!(shown(feed), before);
    assert_eq!(anchors(feed), ["1", "2"]);
    feed.with_untracked(|feed| {
        let cache = feed.detail(&target()).unwrap();
        assert_eq!(cache.error.as_ref().unwrap().code, HISTORY_CHANGED);
        assert!(cache.pages.iter().all(|page| page.etag == "v1"));
    });
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
