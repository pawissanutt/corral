//! A project's commit graph, a page at a time, with the records that name each commit.
//! Read-only: no ref, index, object or working tree changes.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_git::GraphCommit;
use rusqlite::params;

use crate::store::Store;
use crate::{Engine, Run};

/// Links kept per commit; the rest are counted.
pub const LINKS_MAX: usize = 16;
/// Event rows read per commit, through the commit index. Past it the commit's links may be
/// incomplete; the read never grows with unrelated history.
const EVENTS_PER_COMMIT: usize = 256;

/// Only the events that can link a commit, keyed by the commit they name. Its WHERE is the
/// lookup's own, so a seek visits no other event and reads them in record order.
pub(crate) const INDEX: &str = "CREATE INDEX IF NOT EXISTS events_commit_links
     ON events(json_extract(detail,'$.sha'))
     WHERE kind IN ('landed','done') AND task IS NOT NULL AND run IS NOT NULL";

/// `landed` and `done` events naming one commit, in record order, through
/// `events_commit_links`.
pub(crate) const EVENTS_NAMING: &str = "SELECT task, run, kind, json_extract(detail,'$.project')
     FROM events INDEXED BY events_commit_links
     WHERE json_extract(detail,'$.sha') = ?1 AND kind IN ('landed','done')
       AND task IS NOT NULL AND run IS NOT NULL
     ORDER BY seq LIMIT ?2";

/// One page of a project's history from a pinned tip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHistory {
    /// `refs/heads/<branch>` or `HEAD`.
    pub reference: String,
    /// The commit the walk starts from; `None` when the ref names no commit.
    pub tip: Option<String>,
    pub commits: Vec<HistoryCommit>,
    /// Whether commits remain past this page.
    pub more: bool,
    /// The ref listing hit its bound: any commit may lack some of its labels.
    pub refs_incomplete: bool,
    /// Some commit is named by more records than one read takes, or the store lacks the commit
    /// index: links may be missing, and `links_omitted` is then a lower bound.
    pub links_incomplete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryCommit {
    pub commit: GraphCommit,
    pub refs: Vec<String>,
    /// Ref labels past `corral_git::LABELS_MAX`.
    pub refs_omitted: u64,
    /// At most `LINKS_MAX` records that tie exactly this commit to this project.
    pub links: Vec<HistoryLink>,
    /// Such records past `LINKS_MAX`.
    pub links_omitted: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryLink {
    pub run: Run,
    pub title: String,
    pub basis: LinkBasis,
}

/// Which record names the commit. Either names this project (a `landed` event from before
/// landings recorded one only when no other configured project shares the repository) and
/// comes from a run whose recorded worktree is in this project's repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkBasis {
    /// A `landed` event: the run's landing fast-forwarded its target to this commit.
    Landed,
    /// A `done` event: the worker submitted this commit for review. It stays even when a later
    /// landing rebased the run to another head.
    Submitted,
}

impl Engine {
    /// Read `limit` commits of `project`'s history after `skip`, from `tip` when continuing a
    /// walk, else from the current commit of `branch` (the checkout's `HEAD` when absent).
    /// Links come only from records that name this project and the exact commit; a task's
    /// current project is not evidence, and nothing is inferred from subjects, authors or
    /// dates.
    pub fn project_git_history(
        &self,
        project: &str,
        branch: Option<&str>,
        tip: Option<&str>,
        skip: usize,
        limit: usize,
    ) -> Result<GitHistory> {
        let settings = self.config.effective_project(project)?;
        let repo = self.root.join(settings.path);
        let (reference, current) = corral_git::resolve_tip(&repo, branch)?;
        let tip = match tip {
            Some(pinned) => Some(pinned.to_owned()),
            None => current,
        };
        let Some(start) = tip.as_deref() else {
            return Ok(GitHistory {
                reference,
                tip,
                commits: Vec::new(),
                more: false,
                refs_incomplete: false,
                links_incomplete: false,
            });
        };
        let mut page = corral_git::log_page(&repo, start, skip, limit + 1)?;
        let more = page.len() > limit;
        page.truncate(limit);
        let shas: BTreeSet<String> = page.iter().map(|commit| commit.sha.clone()).collect();
        let mut refs = corral_git::ref_labels(&repo, &shas)?;
        let (mut links, links_incomplete) = self.links(project, &repo, &shas)?;
        let commits = page
            .into_iter()
            .map(|commit| {
                let (links, links_omitted) = links.remove(&commit.sha).unwrap_or_default();
                HistoryCommit {
                    refs: refs.labels.remove(&commit.sha).unwrap_or_default(),
                    refs_omitted: refs.omitted.remove(&commit.sha).unwrap_or_default(),
                    links,
                    links_omitted,
                    commit,
                }
            })
            .collect();
        Ok(GitHistory {
            reference,
            tip,
            commits,
            more,
            refs_incomplete: refs.incomplete,
            links_incomplete,
        })
    }

    /// The links of each commit, and whether any commit's records were cut at
    /// `EVENTS_PER_COMMIT`.
    #[allow(clippy::type_complexity)]
    fn links(
        &self,
        project: &str,
        repo: &Path,
        shas: &BTreeSet<String>,
    ) -> Result<(BTreeMap<String, (Vec<HistoryLink>, u64)>, bool)> {
        // Dispatch records each run's worktree as `<canonical repo>/.worktrees/<task>`. A
        // repository that no longer resolves links nothing.
        let Ok(canonical) = repo.canonicalize() else {
            return Ok((BTreeMap::new(), false));
        };
        let worktrees = canonical.join(".worktrees");
        // An event that names no project is evidence only when this project alone uses the
        // repository; with another configured project there it is ambiguous.
        let sole = self.config.projects.iter().all(|(name, other)| {
            name == project
                || self
                    .root
                    .join(&other.path)
                    .canonicalize()
                    .is_ok_and(|path| path != canonical)
        });
        let mut links = BTreeMap::new();
        let mut incomplete = false;
        self.store.read_snapshot(|| {
            // A store opened without setup (read-only) may lack the index: show plain history
            // and say links are missing, rather than scan the log.
            if !self.store.has_commit_index()? {
                incomplete = true;
                return Ok(());
            }
            for sha in shas {
                let mut rows = self.store.events_naming(sha)?;
                if rows.len() > EVENTS_PER_COMMIT {
                    incomplete = true;
                    rows.truncate(EVENTS_PER_COMMIT);
                }
                let found = self.commit_links(project, sole, &worktrees, rows)?;
                if !found.0.is_empty() || found.1 > 0 {
                    links.insert(sha.clone(), found);
                }
            }
            Ok(())
        })?;
        Ok((links, incomplete))
    }

    /// One commit's links from its event rows: one per run, Landed over Submitted.
    fn commit_links(
        &self,
        project: &str,
        sole: bool,
        worktrees: &Path,
        rows: Vec<EventNaming>,
    ) -> Result<(Vec<HistoryLink>, u64)> {
        let mut runs: BTreeMap<(String, String), LinkBasis> = BTreeMap::new();
        for row in rows {
            let names_project = match row.project.as_deref() {
                Some(named) => named == project,
                None => sole && row.basis == LinkBasis::Landed,
            };
            if !names_project {
                continue;
            }
            let basis = runs.entry((row.task, row.run)).or_insert(row.basis);
            if row.basis == LinkBasis::Landed {
                *basis = LinkBasis::Landed;
            }
        }
        let mut links = Vec::new();
        let mut omitted = 0;
        for ((task, id), basis) in runs {
            let Some(run) = self.store.get_run(&task, &id)? else {
                continue;
            };
            let expected: PathBuf = worktrees.join(&task);
            if run.worktree != expected {
                continue;
            }
            if links.len() >= LINKS_MAX {
                omitted += 1;
                continue;
            }
            let title = self
                .store
                .get_task(&task)?
                .map(|task| task.title)
                .unwrap_or_default();
            links.push(HistoryLink { run, title, basis });
        }
        Ok((links, omitted))
    }
}

struct EventNaming {
    task: String,
    run: String,
    basis: LinkBasis,
    project: Option<String>,
}

impl Store {
    /// Whether `events_commit_links` exists (`Store::open` creates it).
    fn has_commit_index(&self) -> Result<bool> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master
                   WHERE type='index' AND name='events_commit_links')",
                [],
                |row| row.get(0),
            )
            .context("look up the commit index")
    }

    /// At most `EVENTS_PER_COMMIT` + 1 `landed` and `done` events naming `sha` (one past the
    /// cap says there are more), read through the commit index.
    fn events_naming(&self, sha: &str) -> Result<Vec<EventNaming>> {
        let mut statement = self
            .connection
            .prepare_cached(EVENTS_NAMING)
            .context("prepare events naming a commit")?;
        let rows = statement
            .query_map(params![sha, EVENTS_PER_COMMIT as i64 + 1], |row| {
                let kind: String = row.get(2)?;
                Ok(EventNaming {
                    task: row.get(0)?,
                    run: row.get(1)?,
                    basis: if kind == "landed" {
                        LinkBasis::Landed
                    } else {
                        LinkBasis::Submitted
                    },
                    project: row.get(3)?,
                })
            })
            .context("query events naming a commit")?;
        rows.map(|row| row.context("read event naming a commit"))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lookup seeks the partial commit index in record order; it never scans or sorts
    /// events, so neither unrelated history nor ineligible events naming the commit grow a
    /// page's read past its row cap.
    #[test]
    fn commit_lookup_seeks_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("store.db")).unwrap();
        let plan: Vec<String> = store
            .connection
            .prepare(&format!("EXPLAIN QUERY PLAN {EVENTS_NAMING}"))
            .unwrap()
            .query_map(params!["0".repeat(40), 1], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            plan.iter()
                .any(|step| step.contains("USING INDEX events_commit_links")),
            "{plan:?}"
        );
        assert!(
            !plan
                .iter()
                .any(|step| step.starts_with("SCAN") || step.contains("TEMP B-TREE")),
            "{plan:?}"
        );
    }
}
