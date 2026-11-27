//! `git.history.v1`: a project's committed history on one ref, read-only, a bounded page at a
//! time, annotated only with links that Corral's own records prove.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Commits in one page.
pub const GIT_HISTORY_PAGE: usize = 50;

/// A page of commits reachable from one tip, children before parents (`git log --topo-order`).
///
/// The first page (no `DetailGet.page`) resolves the ref; every later page walks from the same
/// `tip`, so a walk is a stable snapshot even when the ref moves. Asking for the first page
/// again is the refresh: a moved ref changes its `tip` and etag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitHistoryPage {
    /// The fully qualified ref read (`refs/heads/main`), or `HEAD` for the project checkout's.
    pub reference: String,
    /// Full commit ID the walk starts from; absent when the ref has no commits yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tip: Option<String>,
    /// Commits already walked before this page.
    pub skip: u64,
    /// At most `GIT_HISTORY_PAGE` commits.
    pub commits: Vec<GitCommit>,
    /// The ref listing hit its bound, so any commit may lack some of its labels.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub refs_incomplete: bool,
    /// Some commit is named by more records than one read takes, so its links may be
    /// incomplete.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub links_incomplete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitCommit {
    /// Full commit ID.
    pub sha: String,
    /// Every parent's full commit ID in Git's order (first parent first). A parent need not
    /// be on this page: it is further down the walk, never a root.
    pub parents: Vec<String>,
    pub subject: String,
    pub author: String,
    /// Author date, RFC 3339.
    pub authored: String,
    /// Committer date, RFC 3339.
    pub committed: String,
    /// Local branches (`main`), tags (`tag: v1`) and `HEAD` that point at this commit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
    /// Ref labels at this commit beyond those listed (at most 16 are).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub refs_omitted: u64,
    /// What Corral recorded about exactly this commit in this project; empty when nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<GitCommitLink>,
    /// Such records beyond those listed (at most 16 are).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub links_omitted: u64,
}

fn is_zero(count: &u64) -> bool {
    *count == 0
}

/// One run record naming this exact commit. It proves what `relation` says, never that the
/// agent authored the commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitCommitLink {
    pub task: String,
    pub title: String,
    /// The run whose record names the commit (`R2`).
    pub run: String,
    pub relation: GitLinkRelation,
    /// The run's agent session (`T-12_R2`) when the run recorded one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitLinkRelation {
    /// The worker run landed: the target branch was fast-forwarded to this commit.
    Landed,
    /// The worker run submitted this commit for review (a recorded submission, or its current
    /// head when it has not landed). The run may have landed a different, rebased commit.
    Candidate,
    /// The reviewer run was bound to this candidate when a verdict on it was recorded; the
    /// verdict may have been the owner's. Not served: no record names its project.
    Reviewed,
    #[serde(other)]
    Other,
}
