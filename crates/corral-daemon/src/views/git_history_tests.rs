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

/// A run as dispatch records it: in `<canonical repo>/.worktrees/<task>`.
fn run(
    repo: &std::path::Path,
    task: &str,
    role: Role,
    outcome: Option<Outcome>,
    head: &str,
    session: bool,
) -> Run {
    Run {
        id: "R1".into(),
        task: task.into(),
        role,
        profile: "worker".into(),
        harness: String::new(),
        model: None,
        effort: None,
        worktree: repo.canonicalize().unwrap().join(".worktrees").join(task),
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
    // A fork shares every commit; its records must stay its own.
    let fork = fixture.shared.root().join("fork");
    git(fixture.shared.root(), &["clone", "-q", "repo", "fork"]);
    std::fs::write(
        fixture.shared.root().join(".corral-next/corral.toml"),
        "[runner]\nmode = \"process\"\n[projects.demo]\npath = \"repo\"\n[projects.fork]\npath = \"fork\"\n",
    )
    .unwrap();
    fixture.shared.publish();

    let landed = fixture.named_task("Landed work", Some("demo"));
    let candidate = fixture.named_task("Candidate work", Some("demo"));
    let elsewhere = fixture.named_task("Fork work", Some("fork"));
    let engine = fixture.shared.engine().unwrap();
    let store = &engine.store;
    store
        .insert_run(&run(
            &repo,
            &landed,
            Role::Worker,
            Some(Outcome::Landed),
            &merge,
            true,
        ))
        .unwrap();
    store
        .insert_run(&run(&repo, &candidate, Role::Worker, None, &side, false))
        .unwrap();
    store
        .insert_run(&run(
            &fork,
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

    // More runs name one commit than a page lists: they are counted, not dropped silently.
    let crowded = head(&repo, "main~5");
    for n in 0..18 {
        let task = fixture.named_task(&format!("Crowd {n}"), Some("demo"));
        store
            .insert_run(&run(&repo, &task, Role::Worker, None, &crowded, false))
            .unwrap();
    }

    let get_in = |project: &str, branch: Option<&str>, page: Option<String>| {
        detail(
            &fixture.shared,
            DetailGet {
                target: DetailRef::GitHistory {
                    project: project.into(),
                    branch: branch.map(str::to_owned),
                },
                page,
                etag: None,
            },
        )
    };
    let get = |branch: Option<&str>, page: Option<String>| get_in("demo", branch, page);
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
    let [link] = &at(&submitted).links[..] else {
        panic!("the earlier submission stays a candidate link");
    };
    assert_eq!(
        (link.relation, link.task.as_str()),
        (GitLinkRelation::Candidate, landed.as_str())
    );
    let crowded = at(&crowded);
    assert_eq!(crowded.links.len(), 16);
    assert_eq!(crowded.links_omitted, 2);
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

    // Moving the landed task to the fork does not move its landing: the run's recorded
    // worktree is in `repo`, so `demo` keeps the link and the fork gains none.
    let mut moved = store.require_task(&landed).unwrap();
    moved.project = Some("fork".into());
    store.update_task(&moved).unwrap();
    let links_at = |project: &str, sha: &str| {
        body(&get_in(project, Some("main"), None).unwrap())
            .commits
            .into_iter()
            .find(|c| c.sha == sha)
            .unwrap()
            .links
    };
    assert_eq!(
        links_at("demo", &merge)[0].relation,
        GitLinkRelation::Landed
    );
    assert!(links_at("fork", &merge).is_empty());
    assert!(links_at("fork", &submitted).is_empty());
    // Retargeted before it submitted: the event says `fork`, but the run worked in `repo`.
    let relabelled = head(&repo, "main~4");
    store
        .transaction(|store| {
            store.append_event(&corral_engine::events::NewEvent {
                task: Some(landed.clone()),
                run: Some("R1".into()),
                kind: corral_engine::EventKind::Done,
                actor: corral_engine::model::Actor::Owner,
                detail: serde_json::json!({"sha": relabelled, "project": "fork"}),
            })
        })
        .unwrap();
    assert!(links_at("fork", &relabelled).is_empty());
    assert!(links_at("demo", &plain).is_empty());
    assert_eq!(links_at("fork", &plain)[0].task, elsewhere);
}
