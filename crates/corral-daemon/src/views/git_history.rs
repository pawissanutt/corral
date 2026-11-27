//! `git.history.v1`: a project's commit graph a page at a time, read straight from Git, with
//! links only where a run record names the exact commit.
use anyhow::{Context, Result, ensure};
use corral_engine::{
    Engine,
    git_history::{HistoryLink, LinkBasis},
};
use corral_protocol::detail::{
    DetailBody, DetailGet, DetailReply, GIT_HISTORY_PAGE, GitCommit, GitCommitLink, GitHistoryPage,
    GitLinkRelation,
};

use super::{NotFound, REPLY_MAX, details::reply_page, fingerprint, json_len};

/// Characters of a task title a link carries.
const TITLE_MAX: usize = 200;
/// Deepest walk a page token may ask for.
const SKIP_MAX: usize = 1_000_000;

pub(super) fn read(
    engine: &Engine,
    query: &DetailGet,
    project: &str,
    branch: Option<&str>,
) -> Result<DetailReply> {
    let selector = fingerprint(&query.target)?;
    let pinned = query
        .page
        .as_deref()
        .map(|token| parse_token(token, &selector))
        .transpose()?;
    let (tip, skip) = match pinned {
        Some((tip, skip)) => (Some(tip), skip),
        None => (None, 0),
    };
    // Observe Git outside the store snapshot: refs move independently of store events.
    let history = engine.project_git_history(project, branch, tip, skip, GIT_HISTORY_PAGE)?;
    if history.tip.is_none() && branch.is_some() {
        return Err(NotFound("branch not found").into());
    }
    let mut page = GitHistoryPage {
        reference: history.reference,
        tip: history.tip,
        skip: skip as u64,
        commits: history.commits.into_iter().map(commit).collect(),
        refs_incomplete: history.refs_incomplete,
        links_incomplete: history.links_incomplete,
    };
    let mut more = history.more;
    // Leave room for the envelope; drop the oldest commits of the page, never skip any.
    while json_len(&page)? > REPLY_MAX - 1024 {
        ensure!(
            page.commits.len() > 1,
            "too_large: one commit exceeds the reply budget"
        );
        page.commits.pop();
        more = true;
    }
    let next = match (&page.tip, more) {
        (Some(tip), true) if skip + page.commits.len() <= SKIP_MAX => {
            Some(format!("{selector}:{tip}:{}", skip + page.commits.len()))
        }
        _ => None,
    };
    let etag = fingerprint(&(&selector, &page))?;
    let unchanged = query.page.is_none() && query.etag.as_ref() == Some(&etag);
    reply_page(
        engine.store.latest_seq()?,
        etag,
        DetailBody::GitHistory(page),
        next,
        unchanged,
    )
}

fn parse_token<'a>(token: &'a str, selector: &str) -> Result<(&'a str, usize)> {
    ensure!(
        token.len() <= 160,
        "invalid: git history page token is too long"
    );
    let mut parts = token.split(':');
    let (Some(key), Some(tip), Some(skip), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        anyhow::bail!("invalid: malformed git history page token");
    };
    ensure!(
        key == selector,
        "invalid: detail page belongs to a different selection"
    );
    ensure!(
        matches!(tip.len(), 40 | 64) && tip.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid: malformed git history page tip"
    );
    let skip: usize = skip
        .parse()
        .context("invalid: malformed git history page offset")?;
    ensure!(
        skip != 0 && skip <= SKIP_MAX,
        "invalid: git history page offset out of range"
    );
    Ok((tip, skip))
}

fn commit(commit: corral_engine::git_history::HistoryCommit) -> GitCommit {
    let git = commit.commit;
    GitCommit {
        sha: git.sha,
        parents: git.parents,
        subject: git.subject,
        author: git.author,
        authored: git.authored,
        committed: git.committed,
        refs: commit.refs,
        refs_omitted: commit.refs_omitted,
        links: commit.links.into_iter().map(link).collect(),
        links_omitted: commit.links_omitted,
    }
}

fn link(link: HistoryLink) -> GitCommitLink {
    let run = link.run;
    let relation = match link.basis {
        LinkBasis::Landed => GitLinkRelation::Landed,
        LinkBasis::Submitted => GitLinkRelation::Candidate,
    };
    GitCommitLink {
        session: run
            .session
            .is_some()
            .then(|| format!("{}_{}", run.task, run.id)),
        title: link.title.chars().take(TITLE_MAX).collect(),
        task: run.task,
        run: run.id,
        relation,
    }
}
