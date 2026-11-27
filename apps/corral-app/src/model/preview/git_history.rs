//! The sample's commit history, served as corrald serves `git_history`: a small graph per
//! project with a branch that diverged and was merged back by hand, a backport merge and a root,
//! in pages of five so Load older is walked. Links are fixed: each names one sample task's run
//! on one commit (`MAIN_LINKS`, `BRANCH_LINKS`), landed, candidate or reviewed, so they never
//! move between commits as the sample's tasks change. Every other commit has none.

use corral_protocol::detail::*;

use super::{records, stamp};
use crate::model::daemon::{DetailCache, Feed, detail_key};

/// Commits in one sample page, fewer than corrald's so the walk has pages to load.
const PAGE: usize = 5;
const HOUR: u64 = 3600;

/// A sample commit: its key, subject, author and parents' keys.
type Sample = (
    &'static str,
    &'static str,
    &'static str,
    &'static [&'static str],
);

/// The mainline, children before parents.
const MAIN: [Sample; 13] = [
    (
        "merge-side",
        "Merge branch 'pairing-marker'",
        "Mira Okafor",
        &["land-1", "side-2"],
    ),
    (
        "side-2",
        "Keep the paired marker until the credential is read",
        "Mira Okafor",
        &["side-1"],
    ),
    (
        "side-1",
        "Read the stored credential before reconnecting",
        "Mira Okafor",
        &["land-2"],
    ),
    (
        "land-1",
        "Fold the sidebar when the window is narrow",
        "Jun Park",
        &["land-2"],
    ),
    (
        "land-2",
        "Name the harnesses whose runs report no usage",
        "Jun Park",
        &["tag"],
    ),
    (
        "tag",
        "Pin the toolchain for the 0.9 release",
        "Mira Okafor",
        &["docs"],
    ),
    (
        "docs",
        "Describe the review rounds in the guide",
        "Ada Lindqvist",
        &["fix"],
    ),
    (
        "fix",
        "Keep the scope when the daemon restarts",
        "Jun Park",
        &["land-3"],
    ),
    (
        "land-3",
        "Show forced steps with what they skipped",
        "Ada Lindqvist",
        &["merge-rel"],
    ),
    (
        "merge-rel",
        "Merge branch 'release/0.8'",
        "Mira Okafor",
        &["early", "backport"],
    ),
    (
        "backport",
        "Backport the reconnect fix to 0.8",
        "Ada Lindqvist",
        &["import"],
    ),
    (
        "early",
        "Add the project scope switcher",
        "Jun Park",
        &["import"],
    ),
    ("import", "Initial import", "Mira Okafor", &[]),
];

/// Serves `target` into `feed.details`, every page up to the one `page` names.
pub(super) fn serve(
    feed: &mut Feed,
    target: &DetailRef,
    project: &str,
    branch: Option<&str>,
    page: Option<&str>,
) -> Result<(), String> {
    if !feed.projects.contains_key(project) {
        return Err(format!("not_found: project {project}"));
    }
    let all = records(feed);
    let (reference, commits) = match branch {
        None => ("HEAD".to_owned(), mainline(feed, &all, project, true)),
        Some(name) => {
            let known = super::details::sample_branches(feed, project).contains(&name.to_owned());
            if !known {
                return Err("not_found: branch not found".into());
            }
            (
                format!("refs/heads/{name}"),
                on_branch(feed, &all, project, name),
            )
        }
    };
    let count = match page {
        None => 1,
        Some(token) => {
            token
                .parse::<usize>()
                .map_err(|_| "invalid history cursor".to_string())?
                + 1
        }
    };
    let tip = commits.first().map(|commit| commit.sha.clone());
    let chunks: Vec<&[GitCommit]> = commits.chunks(PAGE).collect();
    let shown = count.min(chunks.len().max(1));
    let walk = |skip: usize, commits: Vec<GitCommit>| GitHistoryPage {
        reference: reference.clone(),
        tip: tip.clone(),
        skip: skip as u64,
        commits,
        refs_incomplete: false,
        links_incomplete: false,
    };
    let pages: Vec<DetailReply> = (0..shown)
        .map(|i| DetailReply {
            etag: format!("preview-history-{}-{i}", tip.as_deref().unwrap_or("none")),
            as_of: stamp(feed.now),
            seq: feed.changes_head,
            next: (i + 1 < chunks.len()).then(|| (i + 1).to_string()),
            body: Some(DetailBody::GitHistory(walk(
                i * PAGE,
                chunks
                    .get(i)
                    .map(|chunk| chunk.to_vec())
                    .unwrap_or_default(),
            ))),
        })
        .collect();
    let merged = walk(0, commits.into_iter().take(shown * PAGE).collect());
    feed.details.insert(
        detail_key(target),
        DetailCache {
            transcript_seams: Default::default(),
            target: target.clone(),
            pages,
            loading: false,
            stale: false,
            error: None,
            merged: Some(DetailBody::GitHistory(merged)),
        },
    );
    Ok(())
}

/// The walk again at the depth already read, after the sample changed.
pub(super) fn again(feed: &mut Feed, target: &DetailRef) -> Result<(), String> {
    let DetailRef::GitHistory { project, branch } = target else {
        return Ok(());
    };
    let count = feed
        .detail(target)
        .map_or(1, |cache| cache.pages.len().max(1));
    let token = (count > 1).then(|| (count - 1).to_string());
    serve(
        feed,
        target,
        &project.clone(),
        branch.clone().as_deref(),
        token.as_deref(),
    )
}

/// A stable, made-up full SHA for a sample commit: FNV-1a mixed by SplitMix64's finaliser, so
/// keys that differ in one letter still read apart.
fn sha(project: &str, key: &str) -> String {
    let word = |seed: u64| {
        let mut x = format!("{project}/{key}")
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325 ^ seed, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
            });
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        x ^ (x >> 31)
    };
    format!("{:016x}{:016x}{:08x}", word(1), word(2), word(3) as u32)
}

fn commit(feed: &Feed, project: &str, at: usize, sample: &Sample) -> GitCommit {
    let (key, subject, author, parents) = *sample;
    // Children are newer: each commit a few hours after its parent.
    let when = stamp(feed.now.saturating_sub((at as u64 + 1) * 5 * HOUR));
    GitCommit {
        sha: sha(project, key),
        parents: parents.iter().map(|parent| sha(project, parent)).collect(),
        subject: subject.into(),
        author: author.into(),
        authored: when.clone(),
        committed: when,
        refs: match key {
            "tag" => vec!["tag: v0.9.0".into()],
            "merge-rel" => vec!["release/0.8".into()],
            _ => Vec::new(),
        },
        // One commit of each kind shows what corrald leaves out past its bounds.
        refs_omitted: if key == "tag" { 2 } else { 0 },
        links: Vec::new(),
        links_omitted: if key == "land-1" { 3 } else { 0 },
    }
}

/// A fixed link: the task, its run, what Corral did and whether the run's session is named.
type Linked = (&'static str, &'static str, GitLinkRelation, bool);

/// The links on a project's mainline by commit key: each names one sample task's run, so a link
/// stays on its commit whatever the sample's tasks do later. Every other commit has none.
const MAIN_LINKS: &[(&str, &str, Linked)] = &[
    (
        "corral",
        "land-1",
        ("T-2", "R1", GitLinkRelation::Landed, true),
    ),
    (
        "corral",
        "land-2",
        ("T-4", "R1", GitLinkRelation::Landed, true),
    ),
    (
        "corral",
        "land-3",
        ("T-5", "R1", GitLinkRelation::Landed, false),
    ),
    // The owner merged this branch by hand: Corral only ever saw it as a candidate.
    (
        "corral",
        "side-2",
        ("T-9", "R1", GitLinkRelation::Candidate, true),
    ),
    (
        "site",
        "land-1",
        ("T-6", "R1", GitLinkRelation::Landed, false),
    ),
];

/// The links on a local branch's own tip commit, by branch name.
const BRANCH_LINKS: &[(&str, &str, Linked)] = &[(
    "corral",
    "corral/T-11",
    ("T-11", "R1", GitLinkRelation::Candidate, true),
)];

/// The links `table` holds for `project` at `at`, titled by the sample task when it is known.
fn links(
    all: &[TaskDetail],
    table: &[(&str, &str, Linked)],
    project: &str,
    at: &str,
) -> Vec<GitCommitLink> {
    table
        .iter()
        .filter(|(owner, place, _)| *owner == project && *place == at)
        .map(|(_, _, (task, run, relation, session))| GitCommitLink {
            task: (*task).into(),
            title: all
                .iter()
                .find(|record| record.task.id == *task)
                .map(|record| record.task.title.clone())
                .unwrap_or_default(),
            run: (*run).into(),
            relation: *relation,
            session: session.then(|| format!("{task}_{run}")),
        })
        .collect()
}

/// HEAD's history; `head` marks its tip as HEAD and the base branch.
fn mainline(feed: &Feed, all: &[TaskDetail], project: &str, head: bool) -> Vec<GitCommit> {
    let mut commits: Vec<GitCommit> = MAIN
        .iter()
        .enumerate()
        .map(|(at, sample)| commit(feed, project, at, sample))
        .collect();
    if head && let Some(tip) = commits.first_mut() {
        tip.refs = vec!["HEAD".into(), "main".into()];
    }
    for (row, (key, ..)) in commits.iter_mut().zip(&MAIN) {
        row.links = links(all, MAIN_LINKS, project, key);
    }
    commits
}

/// A local branch: `main` is HEAD's history; any other has one commit of its own on the
/// mainline before the hand merge, linked as `BRANCH_LINKS` names.
fn on_branch(feed: &Feed, all: &[TaskDetail], project: &str, name: &str) -> Vec<GitCommit> {
    let mut mainline = mainline(feed, all, project, false);
    if name == "main" {
        if let Some(tip) = mainline.first_mut() {
            tip.refs = vec!["main".into()];
        }
        return mainline;
    }
    let base = sha(project, "land-1");
    let rest: Vec<GitCommit> = mainline
        .into_iter()
        .skip_while(|commit| commit.sha != base)
        .collect();
    let when = stamp(feed.now.saturating_sub(2 * HOUR));
    let tip = GitCommit {
        sha: sha(project, &format!("branch/{name}")),
        parents: vec![base],
        subject: format!("Work in progress on {name}"),
        author: "Jun Park".into(),
        authored: when.clone(),
        committed: when,
        refs: vec![name.into()],
        refs_omitted: 0,
        links: links(all, BRANCH_LINKS, project, name),
        links_omitted: 0,
    };
    std::iter::once(tip).chain(rest).collect()
}
