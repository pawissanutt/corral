use super::*;

fn query(app: &mut AppState, feed: Signal<Feed>, id: &str, target: DetailRef) -> Outgoing {
    let mut outgoing = app.request(
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
    outgoing.remove(0)
}
fn reply(app: &mut AppState, feed: Signal<Feed>, outgoing: Outgoing, title: &str) {
    let (method, params, tag) = outgoing;
    let body = FilingsPage {
        total: 1,
        older: None,
        newer: None,
        rows: vec![FiledIdea {
            id: "A-3/g1/i1".into(),
            seq: 1,
            task: "T-1".into(),
            session: "A-3".into(),
            quick: true,
            device: None,
            at: "2026-11-14T10:10:00Z".into(),
            title: title.into(),
            status: "idea".into(),
            decision: None,
        }],
    };
    assert!(
        app.event(
            feed,
            Event::Reply {
                method,
                params,
                tag,
                error_data: None,
                outcome: Ok(value(DetailReply {
                    etag: title.into(),
                    as_of: "2026-11-14T10:10:00Z".into(),
                    seq: 1,
                    next: None,
                    body: Some(DetailBody::Filings(body))
                })),
            }
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn switching_filings_pages_retires_old_requests_and_late_replies_without_refetch_fanout() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.ready = true;
    let task = DetailRef::Task { id: "T-1".into() };
    let task_key = detail_key(&task);
    let task_request = query(&mut app, feed, "task-detail", task);
    let newest = DetailRef::Filings { before: None };
    let older = DetailRef::Filings { before: Some(1) };
    let newest_key = detail_key(&newest);
    let older_key = detail_key(&older);
    let old_request = query(&mut app, feed, "old-newest", newest.clone());
    let older_request = query(&mut app, feed, "older-page", older);
    assert!(feed.with_untracked(|feed| !feed.details.contains_key(&newest_key)));
    assert!(!app.detail_inflight.contains_key(&newest_key));
    assert!(!app.detail_pages.contains_key(&newest_key));
    assert!(!app.detail_bytes.contains_key(&newest_key));
    assert!(feed.with_untracked(|feed| !feed.operations["old-newest"].loading));
    assert!(feed.with_untracked(|feed| feed.details.contains_key(&task_key)));

    let current_request = query(&mut app, feed, "current-newest", newest.clone());
    assert!(feed.with_untracked(|feed| !feed.details.contains_key(&older_key)));
    // This old reply has the current selector, but not its current correlation identity.
    reply(&mut app, feed, old_request, "late old title");
    reply(&mut app, feed, older_request, "late older title");
    assert!(feed.with_untracked(|feed| feed.details[&newest_key].loading));
    assert!(feed.with_untracked(|feed| feed.details[&newest_key].body().is_none()));
    assert!(feed.with_untracked(|feed| !feed.details.contains_key(&older_key)));
    reply(&mut app, feed, current_request, "current title");
    feed.with_untracked(|feed| {
        let Some(DetailBody::Filings(page)) = feed.details[&newest_key].body() else {
            panic!("the current filings page was not published");
        };
        assert_eq!(page.total, 1);
        assert_eq!(page.rows[0].title, "current title");
    });

    let board = app
        .event(
            feed,
            Event::Board {
                epoch: 1,
                rows: Vec::new(),
                left: Vec::new(),
                replacement: true,
            },
        )
        .unwrap();
    let assistant = app
        .event(
            feed,
            Event::Rows {
                replacement: true,
                delta: SubDelta::Assistant {
                    delta: RowsDelta {
                        epoch: 1,
                        rows: Vec::new(),
                        left: Vec::new(),
                        cursor: Default::default(),
                        more: false,
                    },
                },
            },
        )
        .unwrap();
    assert_eq!(board.len(), 1);
    assert_eq!(assistant.len(), 1);
    let current_tag = assistant[0].2.clone();
    let mut prepared = vec![task_request];
    prepared.extend(board);
    prepared.extend(assistant);
    app.filter_outgoing(&mut prepared);
    assert_eq!(prepared.len(), 2);
    assert_eq!(prepared[0].2.as_deref(), Some("task-detail"));
    assert_eq!(prepared[1].2, current_tag);
    assert_eq!(prepared[1].1["before"], Value::Null);
    assert_eq!(
        feed.with_untracked(|feed| feed
            .details
            .values()
            .filter(|cache| matches!(cache.target, DetailRef::Filings { .. }))
            .count()),
        1
    );
    assert_eq!(app.detail_inflight[&task_key], "task-detail");
}
