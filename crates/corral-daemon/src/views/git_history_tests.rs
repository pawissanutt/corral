use corral_engine::{Outcome, Role, Run, SessionRef};
use corral_protocol::detail::{
    DetailBody, DetailGet, DetailRef, GIT_HISTORY_PAGE, GitCommit, GitHistoryPage, GitLinkRelation,
};

use super::{
    detail,
    tests::{Fixture, git},
};

fn head(repo: &std::path::Path, rev: &str) -> String {
    let out = std::process::Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", rev])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn run(task: &str, role: Role, outcome: Option<Outcome>, head: &str, session: bool) -> Run {
    Run {
        id: "R1".into(),
        task: task.into(),
        role,
        profile: "worker".into(),
        harness: String::new(),
        model: None,
        effort: None,
        worktree: Default::default(),
        branch: String::new(),
        base: String::new(),
        base_sha: String::new(),
        session: session.then(|| SessionRef::Process {
            pid: 1,
            dir: Default::default(),
        }),
        started: "2026-02-02T08:00:00Z".parse().unwrap(),
        ended: None,
        outcome,
        summary: None,
        head_sha: Some(head.into()),
        quiet_since: None,
    }
}

/// The walk is Git's own ancestry from a pinned tip across pages (a merge keeps both parents,
/// a ref moving mid-walk changes nothing until the first page is read again), and a commit is
/// linked only through a run of this project's task that recorded exactly that commit.
#[test]
fn history_pages_follow_git_from_a_pinned_tip_and_link_only_recorded_commits() {
    let fixture = Fixture::new();
    fixture.project();
    let repo = fixture.shared.root().join("repo");
    git(&repo, &["checkout", "-q", "-b", "feature"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "side"]);
    let side = head(&repo, "HEAD");
    git(&repo, &["checkout", "-q", "main"]);
    for n in 0..GIT_HISTORY_PAGE {
        git(
            &repo,
            &["commit", "-q", "--allow-empty", "-m", &format!("main {n}")],
        );
    }
    let plain = head(&repo, "HEAD");
    git(&repo, &["merge", "-q", "--no-ff", "-m", "merge", "feature"]);
    let merge = head(&repo, "HEAD");

    let landed = fixture.named_task("Landed work", Some("demo"));
    let candidate = fixture.named_task("Candidate work", Some("demo"));
    let elsewhere = fixture.named_task("Other project", None);
    let engine = fixture.shared.engine().unwrap();
    let store = &engine.store;
    store
        .insert_run(&run(
            &landed,
            Role::Worker,
            Some(Outcome::Landed),
            &merge,
            true,
        ))
        .unwrap();
    store
        .insert_run(&run(&candidate, Role::Worker, None, &side, false))
        .unwrap();
    store
        .insert_run(&run(
            &elsewhere,
            Role::Worker,
            Some(Outcome::Landed),
            &plain,
            true,
        ))
        .unwrap();

    // The landed task first submitted an older commit, which landing later rebased away.
    let submitted = head(&repo, "main~3");
    store
        .transaction(|store| {
            store.append_event(&corral_engine::events::NewEvent {
                task: Some(landed.clone()),
                run: Some("R1".into()),
                kind: corral_engine::EventKind::Done,
                actor: corral_engine::model::Actor::Owner,
                detail: serde_json::json!({"sha": submitted, "project": "demo"}),
            })
        })
        .unwrap();

    let get = |branch: Option<&str>, page: Option<String>| {
        detail(
            &fixture.shared,
            DetailGet {
                target: DetailRef::GitHistory {
                    project: "demo".into(),
                    branch: branch.map(str::to_owned),
                },
                page,
                etag: None,
            },
        )
    };
    let body = |reply: &corral_protocol::detail::DetailReply| -> GitHistoryPage {
        let Some(DetailBody::GitHistory(page)) = reply.body.clone() else {
            panic!("expected git history");
        };
        page
    };
    let first = get(Some("main"), None).unwrap();
    let page = body(&first);
    assert_eq!(page.reference, "refs/heads/main");
    assert_eq!(page.tip.as_deref(), Some(merge.as_str()));
    assert_eq!(page.commits.len(), GIT_HISTORY_PAGE);

    // The ref moves; the continuation still walks the pinned tip's history.
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "later"]);
    let second = get(Some("main"), first.next.clone()).unwrap();
    assert!(second.next.is_none());
    let walked: Vec<GitCommit> = page
        .commits
        .into_iter()
        .chain(body(&second).commits)
        .collect();
    let lines: Vec<String> = walked
        .iter()
        .map(|c| {
            std::iter::once(c.sha.as_str())
                .chain(c.parents.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    let expected = std::process::Command::new("git")
        .current_dir(&repo)
        .args(["rev-list", "--topo-order", "--parents", &merge])
        .output()
        .unwrap();
    assert_eq!(
        lines.join("\n") + "\n",
        String::from_utf8(expected.stdout).unwrap()
    );
    assert!(walked.last().is_some_and(|c| c.parents.is_empty()));

    let at = |sha: &str| walked.iter().find(|c| c.sha == sha).unwrap();
    let merged = at(&merge);
    assert_eq!(merged.parents, [plain.clone(), side.clone()]);
    assert_eq!(merged.refs, ["HEAD", "main"]);
    let [link] = &merged.links[..] else {
        panic!("one landed link");
    };
    assert_eq!(link.relation, GitLinkRelation::Landed);
    assert_eq!(link.task, landed);
    assert_eq!(
        link.session.as_deref(),
        Some(format!("{landed}_R1").as_str())
    );
    let [link] = &at(&side).links[..] else {
        panic!("one candidate link");
    };
    assert_eq!(
        (link.relation, link.session.clone()),
        (GitLinkRelation::Candidate, None)
    );
    assert!(
        at(&plain).links.is_empty(),
        "another project's run is not a link"
    );

    // Reading the first page again is the refresh: it follows the moved ref.
    let refreshed = body(&get(Some("main"), None).unwrap());
    assert_eq!(refreshed.tip, Some(head(&repo, "main")));
    assert_eq!(refreshed.commits[0].subject, "later");
    assert!(get(Some("gone"), None).is_err());
    assert!(get(Some("main"), Some(format!("x:{merge}:1"))).is_err());
}
