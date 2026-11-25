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

fn fixture() -> (AppState, Signal<Feed>) {
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
    (app, feed)
}

fn assert_rows(cache: &DetailCache, expected: &[&str]) {
    let rows: Vec<_> = body(cache)
        .unwrap()
        .messages
        .iter()
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(rows, expected);
}

#[test]
fn detached_reader_slides_three_pages_across_seven_and_latest_cancels_a_pending_shift() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let (mut app, feed) = fixture();
    let target = target("B-1");
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
    assert_rows(&shown(feed).cache, &["1", "2", "3"]);
    assert!(body(&shown(feed).cache).unwrap().older.is_none());
    for (cursor, anchor, id, words, next, expected) in [
        // In a short viewport its top still sees the oldest page when the
        // user presses Show newer. That intended eviction must not block the step.
        ("4", "1", "4", "four", "3", &["2", "3", "4"][..]),
        ("5", "3", "split", "start", "4", &["3", "4", "split"][..]),
        ("6", "split", "split", "end", "5", &["4", "split"][..]),
    ] {
        let get = app.reader_step(feed, intent(ReaderStep::Newer, Some(anchor)));
        assert!(
            reply(
                &mut app,
                feed,
                get,
                Some(cursor),
                page("v1", &[(id, &[words])], Some(next))
            )
            .is_empty()
        );
        let held = shown(feed);
        assert!(held.newer);
        assert_eq!(held.cache.pages.len(), 3);
        assert_rows(&held.cache, expected);
    }
    let get = app.reader_step(feed, intent(ReaderStep::Newer, Some("split")));
    let head = page("v1", &[("7", &["newest"])], Some("6"));
    assert!(reply(&mut app, feed, get, None, head.clone()).is_empty());
    feed.with_untracked(|f| {
        assert!(f.transcript_windows.is_empty());
        assert_rows(f.detail(&target).unwrap(), &["split", "7"]);
    });

    // Detach again to exercise cancellation without interrupting the full return trip.
    let get = app.reader_step(feed, older("4", Some("split")));
    assert!(
        reply(
            &mut app,
            feed,
            get,
            Some("4"),
            page("v1", &[("4", &["four"])], Some("3"))
        )
        .is_empty()
    );
    let pending = app.reader_step(feed, intent(ReaderStep::Newer, Some("split")));
    let latest = app.reader_step(feed, intent(ReaderStep::Latest, None));
    let before = shown(feed);
    assert!(reply(&mut app, feed, pending, None, head).is_empty());
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
    let get = app.detail_request(
        feed,
        requests::new_id(),
        DetailGet {
            target: target.clone(),
            etag: None,
            page: None,
        },
    );
    reply(&mut app, feed, get, None, fresh.clone());
    assert_eq!(shown(feed), before);
    reply(&mut app, feed, latest, None, fresh);
    assert!(feed.with_untracked(|f| f.transcript_windows.is_empty()));
}

#[test]
fn newer_reseek_keeps_the_source_it_finds_on_the_oldest_staged_page() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let (mut app, feed) = fixture();
    for row in (1..=6).rev() {
        let cursor = row.to_string();
        let next = (row - 1).to_string();
        let get = app.reader_step(feed, older(&cursor, None));
        reply(
            &mut app,
            feed,
            get,
            Some(&cursor),
            page(
                "v1",
                &[(&cursor, &["saved"])],
                (row > 1).then_some(next.as_str()),
            ),
        );
    }
    let get = app.reader_step(feed, intent(ReaderStep::Newer, Some("2")));
    let held = shown(feed);
    let mut get = reply(
        &mut app,
        feed,
        get,
        Some("4"),
        page("v2", &[("4", &["saved"])], Some("fresh-3")),
    );
    for row in (2..=8).rev() {
        let id = row.to_string();
        let cursor = format!("fresh-{row}");
        let next = format!("fresh-{}", row - 1);
        assert_eq!(shown(feed), held);
        get = reply(
            &mut app,
            feed,
            get,
            (row != 8).then_some(cursor.as_str()),
            page("v2", &[(&id, &["saved"])], Some(&next)),
        );
    }
    assert!(
        get.is_empty(),
        "the recovered window already moved one page newer"
    );
    let recovered = shown(feed);
    assert_rows(&recovered.cache, &["2", "3", "4"]);
    assert!(recovered.cache.error.is_none());
}

#[test]
fn changed_etag_reseeks_from_head_and_resumes_older_once() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let (mut app, feed) = fixture();
    for (cursor, id, words, next) in [
        ("6", "split", "end", "5"),
        ("5", "split", "start", "4"),
        ("4", "4", "four", "3"),
    ] {
        let get = app.reader_step(feed, older(cursor, None));
        assert!(
            reply(
                &mut app,
                feed,
                get,
                Some(cursor),
                page("v1", &[(id, &[words])], Some(next))
            )
            .is_empty()
        );
    }
    let get = app.reader_step(feed, older("3", Some("4")));
    let before = shown(feed);
    assert_rows(&before.cache, &["4", "split"]);
    let mut get = reply(
        &mut app,
        feed,
        get,
        Some("3"),
        page("v2", &[("3", &["discard this response"])], None),
    );
    for (cursor, id, words, next) in [
        (None, "8", "live", "fresh-6"),
        (Some("fresh-6"), "split", "replacement", "fresh-4"),
        (Some("fresh-4"), "4", "four", "fresh-3"),
    ] {
        assert_eq!(shown(feed), before);
        get = reply(
            &mut app,
            feed,
            get,
            cursor,
            page("v2", &[(id, &[words])], Some(next)),
        );
    }
    assert_eq!(shown(feed), before);
    assert!(
        reply(
            &mut app,
            feed,
            get,
            Some("fresh-3"),
            page("v2", &[("3", &["three"])], Some("fresh-2")),
        )
        .is_empty()
    );
    let after = shown(feed);
    assert_rows(&after.cache, &["3", "4", "split"]);
    assert_eq!(after.cache.pages.len(), 3);
    assert!(after.cache.pages.iter().all(|p| p.etag == "v2"));
    assert!(!after.cache.loading);
    assert!(!after.cache.stale);
    assert!(after.cache.error.is_none());
}
