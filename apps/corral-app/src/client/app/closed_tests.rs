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
fn reopening_branch_picker_supersedes_continuation_and_does_not_retain_page_copies() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.ready = true;
    let target = DetailRef::ProjectBranches {
        project: "site".into(),
    };
    let request = |id: &str, page: Option<&str>| ClientRequest::Detail {
        request_id: id.into(),
        request: DetailGet {
            target: target.clone(),
            etag: None,
            page: page.map(str::to_owned),
        },
    };
    let mut outgoing = app.request(feed, request("first", None));
    let (_, params, tag) = outgoing.remove(0);
    let reply = |name: &str, next: Option<&str>| {
        value(DetailReply {
            etag: name.into(),
            as_of: "now".into(),
            seq: 1,
            next: next.map(str::to_owned),
            body: Some(DetailBody::ProjectBranches(
                corral_protocol::detail::ProjectBranchesPage {
                    branches: vec![name.into()],
                },
            )),
        })
    };
    app.detail_reply(feed, tag, params, Ok(reply("first", Some("next"))))
        .unwrap();
    let mut more = app.request(feed, request("more", Some("next")));
    let (_, more_params, more_tag) = more.remove(0);
    let mut reopened = app.request(feed, request("reopen", None));
    let (_, params, tag) = reopened.remove(0);
    app.detail_reply(feed, more_tag, more_params, Ok(reply("old", None)))
        .unwrap();
    app.detail_reply(feed, tag, params, Ok(reply("fresh", None)))
        .unwrap();
    feed.with_untracked(|feed| {
        assert!(feed.operations["first"].value.is_none());
        assert!(feed.operations["reopen"].value.is_none());
        let cache = feed.detail(&target).unwrap();
        assert!(matches!(cache.body(), Some(DetailBody::ProjectBranches(page)) if page.branches == ["fresh"]));
    });
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
