use super::*;
use crate::client::app::details::transcript::tests::page;

fn intent(step: ReaderStep, anchor: Option<&str>) -> ReaderIntent {
    ReaderIntent {
        holder: 1,
        store: None,
        scope: "B-1".into(),
        resolved: "B-1".into(),
        anchor: anchor.map(str::to_owned),
        step,
    }
}
fn older(cursor: &str, anchor: Option<&str>) -> ReaderIntent {
    intent(
        ReaderStep::Older {
            cursor: cursor.into(),
        },
        anchor,
    )
}
fn reply(
    app: &mut AppState,
    feed: Signal<Feed>,
    outgoing: Vec<Outgoing>,
    cursor: Option<&str>,
    answer: Value,
) -> Vec<Outgoing> {
    let [(_, params, tag)]: [Outgoing; 1] = outgoing.try_into().unwrap();
    assert_eq!(params["page"].as_str(), cursor);
    app.detail_reply(feed, tag, params, Ok(answer)).unwrap()
}
fn shown(feed: Signal<Feed>) -> ReaderPage {
    feed.with_untracked(|f| f.transcript_windows[&1].clone())
}

#[test]
fn detached_reader_slides_three_pages_across_seven_and_latest_cancels_a_pending_shift() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.ready = true;
    app.test_reader_limit = Some(3);
    let target = target("B-1");
    let shared_head = |app: &mut AppState| {
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
    let get = shared_head(&mut app);
    reply(
        &mut app,
        feed,
        get,
        None,
        page("v1", &[("7", &["newest"])], Some("6")),
    );
    for (cursor, id, words, next) in [
        ("6", "split", "end", Some("5")),
        ("5", "split", "start", Some("4")),
        ("4", "4", "four", Some("3")),
        ("3", "3", "three", Some("2")),
        ("2", "2", "two", Some("1")),
        ("1", "1", "one", None),
    ] {
        let get = app.reader_step(feed, older(cursor, None));
        reply(
            &mut app,
            feed,
            get,
            Some(cursor),
            page("v1", &[(id, &[words])], next),
        );
        if cursor == "4" {
            let held = shown(feed);
            assert!(held.newer);
            assert_eq!(body(&held.cache).unwrap().messages[1].items.len(), 2);
            let get = app.reader_step(feed, intent(ReaderStep::Newer, Some("split")));
            reply(
                &mut app,
                feed,
                get,
                None,
                page("v1", &[("7", &["newest"])], Some("6")),
            );
            assert!(feed.with_untracked(|f| f.transcript_windows.is_empty()));
            let get = app.reader_step(feed, older("4", Some("split")));
            reply(
                &mut app,
                feed,
                get,
                Some("4"),
                page("v1", &[("4", &["four"])], Some("3")),
            );
        }
        if matches!(cursor, "4" | "3" | "2" | "1") {
            assert_eq!(shown(feed).cache.pages.len(), 3);
        }
        assert_eq!(
            feed.with_untracked(|f| f.detail(&target).unwrap().pages.len()),
            match cursor {
                "6" => 2,
                _ => 3,
            }
        );
    }
    let pending = app.reader_step(feed, intent(ReaderStep::Newer, Some("2")));
    let latest = app.reader_step(feed, intent(ReaderStep::Latest, None));
    let before = shown(feed);
    reply(
        &mut app,
        feed,
        pending,
        Some("4"),
        page("v1", &[("4", &["four"])], Some("3")),
    );
    assert_eq!(shown(feed), before);
    let fresh = page(
        "v2",
        &[
            ("split", &["start", "end"]),
            ("7", &["newest"]),
            ("8", &["live"]),
        ],
        None,
    );
    let get = shared_head(&mut app);
    reply(&mut app, feed, get, None, fresh.clone());
    assert_eq!(shown(feed), before);
    reply(&mut app, feed, latest, None, fresh);
    assert!(feed.with_untracked(|f| f.transcript_windows.is_empty()));
}
