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

/// An unchanged first commit page kept every older page, whose links and ref labels were never
/// read again, under the new as-of as if current.
#[test]
fn an_unchanged_first_commit_page_drops_the_older_pages_it_did_not_read_again() {
    use corral_protocol::detail::{GitCommit, GitHistoryPage};
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.ready = true;
    let target = DetailRef::GitHistory {
        project: "corral".into(),
        branch: None,
    };
    let mut ask = |id: &str, etag: Option<&str>, page: Option<&str>, reply: DetailReply| {
        let mut outgoing = app.request(
            feed,
            ClientRequest::Detail {
                request_id: id.into(),
                request: DetailGet {
                    target: target.clone(),
                    etag: etag.map(str::to_owned),
                    page: page.map(str::to_owned),
                },
            },
        );
        let (_, params, tag) = outgoing.remove(0);
        app.detail_reply(feed, tag, params, Ok(value(reply)))
            .unwrap();
    };
    let page = |etag: &str, sha: &str, next: Option<&str>| DetailReply {
        etag: etag.into(),
        as_of: "then".into(),
        seq: 1,
        next: next.map(str::to_owned),
        body: Some(DetailBody::GitHistory(GitHistoryPage {
            reference: "HEAD".into(),
            tip: Some("tip".into()),
            skip: 0,
            commits: vec![GitCommit {
                sha: sha.into(),
                parents: Vec::new(),
                subject: String::new(),
                author: String::new(),
                authored: String::new(),
                committed: String::new(),
                refs: Vec::new(),
                links: Vec::new(),
            }],
        })),
    };
    ask("first", None, None, page("p0", "tip", Some("1")));
    ask("older", None, Some("1"), page("p1", "older", None));
    feed.with_untracked(|feed| assert_eq!(feed.detail(&target).unwrap().pages.len(), 2));
    let unchanged = DetailReply {
        etag: "p0".into(),
        as_of: "now".into(),
        seq: 2,
        next: Some("1".into()),
        body: None,
    };
    ask("refresh", Some("p0"), None, unchanged);
    feed.with_untracked(|feed| {
        let cache = feed.detail(&target).unwrap();
        assert_eq!(cache.pages.len(), 1);
        assert_eq!(cache.pages[0].as_of, "now");
        let Some(DetailBody::GitHistory(walk)) = cache.body() else {
            panic!("history was not kept");
        };
        let shas: Vec<&str> = walk.commits.iter().map(|c| c.sha.as_str()).collect();
        assert_eq!(shas, ["tip"]);
    });
}
