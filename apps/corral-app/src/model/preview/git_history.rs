//! The sample's commit history, served as corrald serves `git_history`: a small graph per
//! project with a branch that diverged and was merged back by hand, a backport merge and a root,
//! in pages of five so Load older is walked. Links come from the sample's own run records: its
//! landed worker runs land the mainline commits, and a submitted, unlanded worker run names the
//! merged-by-hand branch's tip as its candidate. Every other commit has none.

use corral_protocol::detail::*;
use corral_types::{Outcome, Role, Status};

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
        links: Vec::new(),
    }
}

/// Worker runs of `project`'s tasks with `outcome`, newest first.
fn runs<'a>(
    all: &'a [TaskDetail],
    project: &str,
    outcome: Outcome,
) -> Vec<(&'a TaskDetail, &'a corral_types::Run)> {
    let mut found: Vec<_> = all
        .iter()
        .filter(|record| record.task.project.as_deref() == Some(project))
        .flat_map(|record| record.runs.iter().map(move |run| (record, run)))
        .filter(|(_, run)| run.role == Role::Worker && run.outcome == Some(outcome))
        .collect();
    found.sort_by_key(|(_, run)| std::cmp::Reverse(run.ended));
    found
}

fn link(record: &TaskDetail, run: &corral_types::Run, relation: GitLinkRelation) -> GitCommitLink {
    GitCommitLink {
        task: record.task.id.clone(),
        title: record.task.title.clone(),
        run: run.id.clone(),
        relation,
        session: run
            .session
            .is_some()
            .then(|| format!("{}_{}", record.task.id, run.id)),
    }
}

/// The newest submitted worker run whose task has not landed: a candidate.
fn candidate<'a>(
    all: &'a [TaskDetail],
    project: &str,
) -> Option<(&'a TaskDetail, &'a corral_types::Run)> {
    runs(all, project, Outcome::Done)
        .into_iter()
        .find(|(record, _)| !matches!(record.task.status, Status::Done | Status::Dropped))
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
    let landed = runs(all, project, Outcome::Landed);
    for (key, (record, run)) in ["land-1", "land-2", "land-3"].iter().zip(&landed) {
        let sha = sha(project, key);
        if let Some(commit) = commits.iter_mut().find(|commit| commit.sha == sha) {
            commit
                .links
                .push(link(record, run, GitLinkRelation::Landed));
        }
    }
    // The owner merged this branch by hand: Corral only ever saw it as a candidate.
    if let Some((record, run)) = candidate(all, project) {
        let sha = sha(project, "side-2");
        if let Some(commit) = commits.iter_mut().find(|commit| commit.sha == sha) {
            commit
                .links
                .push(link(record, run, GitLinkRelation::Candidate));
        }
    }
    commits
}

/// A local branch: `main` is HEAD's history; any other has one commit of its own on the
/// mainline before the hand merge, the candidate's when a run worked on it.
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
    let mut tip = GitCommit {
        sha: sha(project, &format!("branch/{name}")),
        parents: vec![base],
        subject: format!("Work in progress on {name}"),
        author: "Jun Park".into(),
        authored: when.clone(),
        committed: when,
        refs: vec![name.into()],
        links: Vec::new(),
    };
    let worked = all
        .iter()
        .filter(|record| record.task.project.as_deref() == Some(project))
        .filter(|record| !matches!(record.task.status, Status::Done | Status::Dropped))
        .flat_map(|record| record.runs.iter().map(move |run| (record, run)))
        .find(|(_, run)| {
            run.role == Role::Worker && run.branch == name && run.outcome == Some(Outcome::Done)
        });
    if let Some((record, run)) = worked {
        tip.links
            .push(link(record, run, GitLinkRelation::Candidate));
    }
    std::iter::once(tip).chain(rest).collect()
}
