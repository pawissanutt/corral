//! A project's commit graph, a page at a time, with the records that name each commit.
//! Read-only: no ref, index, object or working tree changes.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result};
use corral_git::GraphCommit;
use rusqlite::params;

use crate::store::{Store, encode};
use crate::{Engine, Run};

/// Links kept per commit; the rest are counted.
pub const LINKS_MAX: usize = 16;

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryCommit {
    pub commit: GraphCommit,
    pub refs: Vec<String>,
    /// Ref labels past `corral_git::LABELS_MAX`.
    pub refs_omitted: u64,
    /// At most `LINKS_MAX` records that name exactly this commit in this project.
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

/// Which record names the commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkBasis {
    /// The run's `head_sha`, for a run whose recorded worktree is in this project's repository:
    /// its landed tip, its latest submission or, for a reviewer, the candidate it was bound to.
    Head,
    /// A `done` event that recorded this project and commit: the worker submitted it for
    /// review. It stays even when a later landing rebased the run to another head.
    Submitted,
}

impl Engine {
    /// Read `limit` commits of `project`'s history after `skip`, from `tip` when continuing a
    /// walk, else from the current commit of `branch` (the checkout's `HEAD` when absent).
    /// Links come only from records that tie this project's repository to the exact commit;
    /// a task's current project is not evidence, and nothing is inferred from subjects,
    /// authors or dates.
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
            });
        };
        let mut page = corral_git::log_page(&repo, start, skip, limit + 1)?;
        let more = page.len() > limit;
        page.truncate(limit);
        let shas: BTreeSet<String> = page.iter().map(|commit| commit.sha.clone()).collect();
        let mut refs = corral_git::ref_labels(&repo, &shas)?;
        let mut links = self.links(project, &repo, &shas)?;
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
        })
    }

    fn links(
        &self,
        project: &str,
        repo: &Path,
        shas: &BTreeSet<String>,
    ) -> Result<BTreeMap<String, (Vec<HistoryLink>, u64)>> {
        let wanted: Vec<&str> = shas.iter().map(String::as_str).collect();
        // Dispatch records each run's worktree as `<canonical repo>/.worktrees/<task>`, so this
        // ties a run to the repository it ran in, whatever its task's project is now. A
        // repository that no longer resolves, or a legacy run elsewhere, links nothing.
        let worktrees = repo.canonicalize().ok().and_then(|repo| {
            repo.join(".worktrees")
                .to_str()
                .map(|dir| format!("{dir}/"))
        });
        let heads = self.store.runs_naming(&wanted, worktrees.as_deref())?;
        let submissions = self
            .store
            .submissions_naming(project, &wanted, worktrees.as_deref())?;
        let ids: BTreeSet<&str> = heads
            .iter()
            .map(|(run, _)| run.task.as_str())
            .chain(submissions.iter().map(|row| row.task.as_str()))
            .collect();
        let titles: BTreeMap<String, String> = self
            .store
            .tasks_by_ids(&ids.into_iter().collect::<Vec<_>>())?
            .into_iter()
            .map(|task| (task.id, task.title))
            .collect();
        let mut links: BTreeMap<String, (Vec<HistoryLink>, u64)> = BTreeMap::new();
        let mut totals: BTreeMap<String, u64> = BTreeMap::new();
        for (run, total) in heads {
            let Some(sha) = run.head_sha.clone() else {
                continue;
            };
            *totals.entry(sha.clone()).or_default() = total;
            links.entry(sha).or_default().0.push(HistoryLink {
                title: titles.get(&run.task).cloned().unwrap_or_default(),
                run,
                basis: LinkBasis::Head,
            });
        }
        for row in submissions {
            *totals.entry(row.sha.clone()).or_default() += row.total;
            let entry = &mut links.entry(row.sha).or_default().0;
            if entry.len() >= LINKS_MAX {
                continue;
            }
            let Some(run) = self.store.get_run(&row.task, &row.run)? else {
                continue;
            };
            entry.push(HistoryLink {
                title: titles.get(&row.task).cloned().unwrap_or_default(),
                run,
                basis: LinkBasis::Submitted,
            });
        }
        for (sha, (list, omitted)) in &mut links {
            list.truncate(LINKS_MAX);
            *omitted = totals.get(sha).copied().unwrap_or_default() - list.len() as u64;
        }
        links.retain(|_, (list, omitted)| !list.is_empty() || *omitted > 0);
        Ok(links)
    }
}

struct Submission {
    task: String,
    run: String,
    sha: String,
    /// Submissions naming this commit in all.
    total: u64,
}

impl Store {
    /// Up to `LINKS_MAX` runs per commit whose `head_sha` is one of `shas` and whose recorded
    /// worktree is `<worktrees><task>`, each with how many such runs name its commit.
    fn runs_naming(&self, shas: &[&str], worktrees: Option<&str>) -> Result<Vec<(Run, u64)>> {
        if shas.is_empty() || worktrees.is_none() {
            return Ok(Vec::new());
        }
        let mut statement = self
            .connection
            .prepare_cached(
                "SELECT document, total FROM (
                   SELECT document,
                     ROW_NUMBER() OVER (PARTITION BY json_extract(document,'$.head_sha')
                       ORDER BY task, CAST(substr(id,2) AS INTEGER)) AS n,
                     COUNT(*) OVER (PARTITION BY json_extract(document,'$.head_sha')) AS total
                   FROM runs
                   WHERE json_extract(document,'$.head_sha') IN (SELECT value FROM json_each(?1))
                     AND json_extract(document,'$.worktree') = ?2 || task
                 ) WHERE n <= ?3",
            )
            .context("prepare runs naming commits")?;
        let rows = statement
            .query_map(
                params![encode(&shas)?, worktrees, LINKS_MAX as i64],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .context("query runs naming commits")?;
        rows.map(|row| {
            let (document, total) = row.context("read run naming a commit")?;
            let run: Run = serde_json::from_str(&document).context("decode stored run")?;
            Ok((run, u64::try_from(total).unwrap_or_default()))
        })
        .collect()
    }

    /// Up to `LINKS_MAX` distinct submissions per commit (`done` events) that recorded
    /// `project` and one of `shas`, leaving out those `runs_naming` already links by head.
    fn submissions_naming(
        &self,
        project: &str,
        shas: &[&str],
        worktrees: Option<&str>,
    ) -> Result<Vec<Submission>> {
        if shas.is_empty() {
            return Ok(Vec::new());
        }
        let mut statement = self
            .connection
            .prepare_cached(
                "SELECT task, run, sha, total FROM (
                   SELECT task, run, sha,
                     ROW_NUMBER() OVER (PARTITION BY sha ORDER BY task, run) AS n,
                     COUNT(*) OVER (PARTITION BY sha) AS total
                   FROM (SELECT DISTINCT e.task AS task, e.run AS run,
                           json_extract(e.detail,'$.sha') AS sha
                         FROM events e
                         WHERE e.kind='done' AND e.task IS NOT NULL AND e.run IS NOT NULL
                           AND json_extract(e.detail,'$.project')=?1
                           AND json_extract(e.detail,'$.sha') IN (SELECT value FROM json_each(?2))
                           AND NOT EXISTS (SELECT 1 FROM runs r
                             WHERE r.task=e.task AND r.id=e.run
                               AND json_extract(r.document,'$.head_sha')
                                 = json_extract(e.detail,'$.sha')
                               AND json_extract(r.document,'$.worktree') = ?3 || r.task))
                 ) WHERE n <= ?4",
            )
            .context("prepare submissions naming commits")?;
        let rows = statement
            .query_map(
                params![project, encode(&shas)?, worktrees, LINKS_MAX as i64],
                |row| {
                    Ok(Submission {
                        task: row.get(0)?,
                        run: row.get(1)?,
                        sha: row.get(2)?,
                        total: u64::try_from(row.get::<_, i64>(3)?).unwrap_or_default(),
                    })
                },
            )
            .context("query submissions naming commits")?;
        rows.map(|row| row.context("read submission naming a commit"))
            .collect()
    }
}
