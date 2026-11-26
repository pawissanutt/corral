use super::*;
use crate::client::app::details::transcript::tests::page;
use corral_protocol::subscription::{SessionLiveEvent, SessionLiveKind, SessionLiveTurnState};

fn answer(
    app: &mut AppState,
    feed: Signal<Feed>,
    read: Vec<Outgoing>,
    page: Value,
) -> Vec<Outgoing> {
    let [(_, params, tag)]: [Outgoing; 1] = read.try_into().unwrap();
    assert!(params["page"].is_null());
    app.detail_reply(feed, tag, params, Ok(page)).unwrap()
}

#[test]
fn legacy_private_latest_does_not_swallow_a_later_turn_end_refresh() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.ready = true;
    let target = target("B-1");
    let shared = |app: &mut AppState| {
        app.detail_request(
            feed,
            requests::new_id(),
            DetailGet {
                target: target.clone(),
                etag: None,
                page: None,
            },
        )
    };
    let read = shared(&mut app);
    assert!(answer(&mut app, feed, read, page("old", &[], None)).is_empty());
    let event = |seq, kind| SessionLiveEvent {
        seq,
        generation: 1,
        at: String::new(),
        kind,
    };
    feed.update(|f| {
        f.live.entry("B-1".into()).or_default().apply_page(
            Some(1),
            vec![event(
                1,
                SessionLiveKind::LiveTurn {
                    turn: 1,
                    state: SessionLiveTurnState::Started,
                },
            )],
            false,
        );
    });
    let latest = app.reader_step(
        feed,
        ReaderIntent {
            holder: 1,
            store: None,
            scope: "B-1".into(),
            resolved: "B-1".into(),
            anchor: None,
            step: ReaderStep::Latest,
        },
    );
    feed.update(|f| {
        f.live.get_mut("B-1").unwrap().apply_page(
            Some(1),
            vec![event(
                2,
                SessionLiveKind::LiveTurn {
                    turn: 1,
                    state: SessionLiveTurnState::Ended {
                        aborted: false,
                        error: None,
                    },
                },
            )],
            false,
        );
    });
    let pending = shared(&mut app);
    assert_eq!(pending[0].1["etag"], "old");
    let refresh = answer(&mut app, feed, latest, page("private", &[], None));
    let assert_retained = || {
        feed.with_untracked(|f| {
            assert_eq!(f.detail(&target).unwrap().pages[0].etag, "private");
        })
    };
    assert_retained();
    assert_eq!(refresh.len(), 1);
    assert_eq!(refresh[0].1["etag"], "private");
    assert_ne!(refresh[0].2, pending[0].2);

    let mut not_modified = page("old", &[], None);
    not_modified["body"] = Value::Null;
    assert!(answer(&mut app, feed, pending, not_modified).is_empty());
    assert_retained();
    assert!(
        answer(
            &mut app,
            feed,
            refresh,
            page("recorded", &[("output", &["later output"])], None)
        )
        .is_empty()
    );
    feed.with_untracked(|f| {
        assert_eq!(f.detail(&target).unwrap().pages[0].etag, "recorded");
    });
}
