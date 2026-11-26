use super::*;
use crate::model::session::transcript::mirror;

fn put(id: &str, words: &[&str], replaces: Option<&str>, open: bool) -> TranscriptOp {
    let reply: DetailReply = serde_json::from_value(page("", &[(id, words)], None)).unwrap();
    TranscriptOp::Put {
        message: session(&reply).unwrap().messages[0].clone(),
        before: Some(format!("before-{id}")),
        replaces: replaces.map(str::to_owned),
        open,
        cut: false,
    }
}

fn apply(feed: Signal<Feed>, ops: Vec<TranscriptOp>, more: bool) {
    feed.update(|feed| {
        mirror::apply(
            feed.details.get_mut(&detail_key(&target())).unwrap(),
            ops,
            more,
        );
    });
}

#[test]
fn transcript_operations_splice_replace_write_and_drop_in_order() {
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
            "a",
            &[("a", &["one"]), ("b", &["two"]), ("old-tail", &["old"])],
            None,
        ),
    );
    apply(
        feed,
        vec![TranscriptOp::Reset {
            older: Some("before-b".into()),
        }],
        true,
    );
    apply(
        feed,
        vec![
            put("b", &["new"], None, false),
            put("open", &[], None, true),
        ],
        false,
    );
    let append = |part, offset, text: &str| TranscriptOp::Append {
        id: "open".into(),
        part,
        offset,
        text: text.into(),
    };
    apply(
        feed,
        vec![
            append(TranscriptPart::Text, 0, "é"),
            append(TranscriptPart::Text, 2, "!"),
            append(TranscriptPart::Thinking, 0, "hmm"),
        ],
        false,
    );
    feed.with_untracked(|feed| {
        let Some(DetailBody::Session(page)) = feed.detail(&target()).and_then(DetailCache::body)
        else {
            panic!("session")
        };
        assert_eq!(
            page.messages
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "open"]
        );
        assert_eq!(
            page.messages[2].items,
            [
                SessionItem::Thinking { text: "hmm".into() },
                SessionItem::Text { text: "é!".into() }
            ]
        );
    });
    apply(
        feed,
        vec![
            put("record", &["é!"], Some("open"), false),
            put("record", &["updated"], None, false),
            put("aborted", &[], None, true),
            TranscriptOp::Drop {
                id: "aborted".into(),
            },
        ],
        false,
    );
    assert_eq!(
        shown(feed),
        [
            ("a".into(), vec!["one".into()]),
            ("b".into(), vec!["new".into()]),
            ("record".into(), vec!["updated".into()])
        ]
    );
    // Only the last items of a record too large to carry: what is held before it no
    // longer joins it, and its earlier items are a page back.
    let mut last = put("record", &["last"], None, false);
    if let TranscriptOp::Put { cut, .. } = &mut last {
        *cut = true;
    }
    apply(feed, vec![last], false);
    assert_eq!(shown(feed), [("record".into(), vec!["last".into()])]);
    feed.with_untracked(|feed| {
        let Some(DetailBody::Session(page)) = feed.detail(&target()).and_then(DetailCache::body)
        else {
            panic!("session")
        };
        assert_eq!(page.older.as_deref(), Some("before-record"));
    });
    apply(
        feed,
        vec![
            TranscriptOp::Reset { older: None },
            put("another-branch", &["branch"], None, false),
        ],
        false,
    );
    assert_eq!(
        shown(feed),
        [("another-branch".into(), vec!["branch".into()])]
    );
}

/// A long reply's record carries a preview of its words: the reply shrank to it when it
/// ended, and grew back once the rest was read.
#[test]
fn a_long_reply_keeps_its_words_when_its_record_carries_a_preview() {
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
        page("a", &[("a", &["one"])], None),
    );
    let words = "word ".repeat(600);
    let whole = SessionItem::Text {
        text: words.clone(),
    };
    let record = |bytes: u64| {
        let mut record = put("record", &[&words[..255]], Some("open"), false);
        if let TranscriptOp::Put { message, .. } = &mut record {
            message.items.push(SessionItem::Continuation {
                token: "rest".into(),
                bytes,
            });
        }
        record
    };
    let size = serde_json::to_vec(&whole).unwrap().len() as u64;
    let held = || {
        feed.with_untracked(|feed| {
            let Some(DetailBody::Session(page)) =
                feed.detail(&target()).and_then(DetailCache::body)
            else {
                panic!("session")
            };
            page.messages[1].items.clone()
        })
    };
    apply(
        feed,
        vec![
            put("open", &[], None, true),
            TranscriptOp::Append {
                id: "open".into(),
                part: TranscriptPart::Text,
                offset: 0,
                text: words.clone(),
            },
            record(size),
        ],
        false,
    );
    assert_eq!(held(), [whole.clone()]);
    // The record comes again when a tool result is folded into it.
    apply(feed, vec![record(size)], false);
    assert_eq!(held(), [whole]);
    // Words that are not the record's are not kept in its place.
    apply(feed, vec![record(size + 1)], false);
    assert!(matches!(
        held().as_slice(),
        [SessionItem::Text { text }, SessionItem::Continuation { .. }] if text.len() == 255
    ));
}

#[test]
fn following_a_turn_beyond_one_history_page_keeps_the_start() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    feed.update(|f| {
        f.capabilities.insert(SESSION_TRANSCRIPT_V1.into());
    });
    let mut app = AppState::new();
    app.ready = true;
    app.session_scopes.insert("B-1".into(), 1);
    let get = read(&mut app, feed, None);
    reply(
        &mut app,
        feed,
        get,
        None,
        page("a", &[("start", &["prompt"])], Some("before-start")),
    );
    let deliver = |app: &mut AppState, transcript, events, rev| {
        assert!(
            app.rows(
                feed,
                SubDelta::Session {
                    scope: "B-1".into(),
                    epoch: 1,
                    generation: Some(1),
                    events,
                    cursor: BoardCursor { rev, after: None },
                    more: false,
                    gap: None,
                    transcript,
                    fleet: None,
                    state: None,
                },
                false
            )
            .unwrap()
            .is_empty()
        );
    };
    deliver(
        &mut app,
        vec![
            TranscriptOp::Reset {
                older: Some("before-start".into()),
            },
            put("start", &["prompt"], None, false),
            put("reply-1", &["first"], None, false),
        ],
        vec![],
        1,
    );
    let history = read(&mut app, feed, Some("before-start"));
    deliver(
        &mut app,
        vec![put("reply-2", &["second"], None, false)],
        vec![SessionLiveEvent {
            seq: 2,
            generation: 1,
            at: String::new(),
            kind: SessionLiveKind::LiveTurn {
                turn: 1,
                state: SessionLiveTurnState::Ended {
                    aborted: false,
                    error: None,
                },
            },
        }],
        2,
    );
    reply(
        &mut app,
        feed,
        history,
        Some("before-start"),
        page("different-etag", &[("earlier", &["history"])], None),
    );
    let explicit_refresh = read(&mut app, feed, None);
    reply(
        &mut app,
        feed,
        explicit_refresh,
        None,
        page("b", &[("reply-2", &["second"])], Some("before-reply-2")),
    );
    assert_eq!(
        shown(feed)
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        ["earlier", "start", "reply-1", "reply-2"]
    );
}
