use super::*;

fn query(app: &mut AppState, feed: Signal<Feed>, id: &str, target: DetailRef) {
    let outgoing = app.request(
        feed,
        ClientRequest::Detail {
            request_id: id.into(),
            request: DetailGet {
                target,
                etag: None,
                page: None,
            },
        },
    );
    assert_eq!(outgoing.len(), 1);
}

#[test]
fn an_older_closed_page_retires_the_newest_closed_page_but_no_other_list() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.ready = true;
    let artifacts = DetailRef::Artifacts {
        filter: None,
        narrowing: None,
    };
    let newest = DetailRef::Closed { before: None };
    let older = DetailRef::Closed {
        before: Some("anchor-1".into()),
    };
    query(&mut app, feed, "artifacts", artifacts.clone());
    query(&mut app, feed, "newest", newest.clone());
    query(&mut app, feed, "older", older.clone());
    feed.with_untracked(|feed| {
        assert!(!feed.details.contains_key(&detail_key(&newest)));
        assert!(feed.details.contains_key(&detail_key(&older)));
        assert!(feed.details.contains_key(&detail_key(&artifacts)));
        assert!(!feed.operations["newest"].loading);
    });
    assert!(!app.detail_inflight.contains_key(&detail_key(&newest)));
    assert_eq!(app.detail_inflight[&detail_key(&artifacts)], "artifacts");
}
