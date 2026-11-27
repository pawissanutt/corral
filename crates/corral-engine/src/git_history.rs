//! A project's commit graph, a page at a time, with the run records that name each commit.
//! Read-only: no ref, index or working tree changes.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use corral_git::GraphCommit;

use crate::{Engine, Run};

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryCommit {
    pub commit: GraphCommit,
    pub refs: Vec<String>,
    /// Runs of this project's tasks whose recorded head is exactly this commit.
    pub links: Vec<HistoryLink>,
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
    /// The run's current `head_sha`: its landed tip, its latest submission or, for a
    /// reviewer, the candidate its verdict was recorded on.
    Head,
    /// A `done` event: the worker submitted this commit for review. It stays even when a
    /// later landing rebased the run to another head.
    Submitted,
}

impl Engine {
    /// Read `limit` commits of `project`'s history after `skip`, from `tip` when continuing a
    /// walk, else from the current commit of `branch` (the checkout's `HEAD` when absent).
    /// Links come only from run records whose head is the exact commit and whose task belongs
    /// to `project`; nothing is inferred from subjects, authors or dates.
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
            });
        };
        let mut page = corral_git::log_page(&repo, start, skip, limit + 1)?;
        let more = page.len() > limit;
        page.truncate(limit);
        let shas: BTreeSet<String> = page.iter().map(|commit| commit.sha.clone()).collect();
        let mut refs = corral_git::ref_labels(&repo, &shas)?;
        let mut links = self.links(project, &shas)?;
        let commits = page
            .into_iter()
            .map(|commit| HistoryCommit {
                refs: refs.remove(&commit.sha).unwrap_or_default(),
                links: links.remove(&commit.sha).unwrap_or_default(),
                commit,
            })
            .collect();
        Ok(GitHistory {
            reference,
            tip,
            commits,
            more,
        })
    }

    fn links(
        &self,
        project: &str,
        shas: &BTreeSet<String>,
    ) -> Result<BTreeMap<String, Vec<HistoryLink>>> {
        let wanted: Vec<&str> = shas.iter().map(String::as_str).collect();
        let runs = self.store.runs_with_heads(&wanted)?;
        let submissions = self.store.submissions_of(project, &wanted)?;
        let ids: BTreeSet<&str> = runs
            .iter()
            .map(|run| run.task.as_str())
            .chain(submissions.iter().map(|(task, _, _)| task.as_str()))
            .collect();
        let tasks: BTreeMap<String, crate::Task> = self
            .store
            .tasks_by_ids(&ids.into_iter().collect::<Vec<_>>())?
            .into_iter()
            .map(|task| (task.id.clone(), task))
            .collect();
        let mut links: BTreeMap<String, Vec<HistoryLink>> = BTreeMap::new();
        for run in runs {
            let task = tasks.get(&run.task);
            // A run's head names a commit of the task's current project only.
            let (Some(task), Some(sha)) = (
                task.filter(|task| task.project.as_deref() == Some(project)),
                run.head_sha.clone(),
            ) else {
                continue;
            };
            links.entry(sha).or_default().push(HistoryLink {
                title: task.title.clone(),
                run,
                basis: LinkBasis::Head,
            });
        }
        // The submission recorded its project, so it holds even if the task moved since.
        for (task, run, sha) in submissions {
            let entry = links.entry(sha).or_default();
            if entry.iter().any(|l| l.run.task == task && l.run.id == run) {
                continue;
            }
            let (Some(found), Some(run)) = (tasks.get(&task), self.store.get_run(&task, &run)?)
            else {
                continue;
            };
            entry.push(HistoryLink {
                title: found.title.clone(),
                run,
                basis: LinkBasis::Submitted,
            });
        }
        links.retain(|_, list| !list.is_empty());
        Ok(links)
    }
}
