//! Factual integration context. Semantic advice belongs to the attributed reviewer, not Git.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use corral_core::{Artifact, ArtifactKind, ReviewPolicy, Run, RunOutcome, Status, Task};
use serde::{Deserialize, Serialize};

use crate::{Ctx, verification};

#[cfg(test)]
mod tests;

#[derive(Debug)]
pub(crate) struct StalePacket;

impl std::fmt::Display for StalePacket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("stale_retry: integration packet task, run, head or base changed")
    }
}

impl std::error::Error for StalePacket {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RepairKey {
    pub task: String,
    pub run: String,
    pub head: String,
    pub base: String,
    pub reason: String,
}

impl RepairKey {
    pub fn new(task: &Task, run: &Run, reason: &str) -> Result<Self> {
        Ok(Self {
            task: task.id.clone(),
            run: run.id.clone(),
            head: corral_git::rev_sha(&run.worktree, "HEAD")?,
            base: corral_git::rev_sha(&run.worktree, &run.base)?,
            reason: reason.to_string(),
        })
    }

    pub fn digest(&self) -> Result<String> {
        Ok(verification::digest(&serde_json::to_vec(self)?))
    }

    /// Called inside the task write lock as well as before any external delivery.
    pub fn check(&self, task: &Task) -> Result<()> {
        let Some(run) = task
            .current_run()
            .filter(|r| task.id == self.task && r.id == self.run)
        else {
            return Err(StalePacket.into());
        };
        if task.status != Status::Review
            || run.outcome != Some(RunOutcome::Done)
            || corral_git::rev_sha(&run.worktree, "HEAD")? != self.head
            || corral_git::rev_sha(&run.worktree, &run.base)? != self.base
        {
            return Err(StalePacket.into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LandedTask {
    pub id: String,
    pub title: Option<String>,
    pub run: Option<String>,
    pub approved_sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LandedCommit {
    pub sha: String,
    pub subject: String,
    /// Empty means unattributed: subjects and branch names are never attribution evidence.
    pub tasks: Vec<LandedTask>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileSize {
    pub path: String,
    pub candidate_sloc: Option<usize>,
    pub main_sloc: Option<usize>,
    pub rust_ceiling: Option<usize>,
}

impl FileSize {
    pub fn at(repo: &std::path::Path, key: &RepairKey, path: String) -> Result<Self> {
        let count = |sha: &str| -> Result<Option<usize>> {
            Ok(corral_git::read_blob(repo, sha, &path)?.map(|text| {
                text.lines()
                    .filter(|line| {
                        let line = line.trim();
                        !line.is_empty() && !line.starts_with("//")
                    })
                    .count()
            }))
        };
        Ok(Self {
            candidate_sloc: count(&key.head)?,
            main_sloc: count(&key.base)?,
            rust_ceiling: path.ends_with(".rs").then_some(800),
            path,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReviewerNote {
    pub reviewer: String,
    pub sha: String,
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct IntegrationPacket {
    pub key: RepairKey,
    pub dispatch_base: String,
    pub merge_base: String,
    /// Historical drift since dispatch, including commits the worker has already integrated.
    pub dispatch_history: Vec<LandedCommit>,
    /// Everything reachable from main but not the candidate, regardless of dispatch history.
    pub missing_from_candidate: Vec<LandedCommit>,
    pub conflict_paths: Vec<String>,
    pub candidate_changed_paths: Vec<String>,
    pub main_changed_paths: Vec<String>,
    pub overlaps: Vec<String>,
    pub file_sizes: Vec<FileSize>,
    pub sloc_method: String,
    pub gate_commands: Vec<String>,
    pub git_details: Option<String>,
    pub reviewer_note: Option<ReviewerNote>,
}

impl IntegrationPacket {
    pub fn build(
        ctx: &Ctx,
        task: &Task,
        run: &Run,
        policy: &ReviewPolicy,
        key: RepairKey,
        mut conflict_paths: Vec<String>,
        git_details: Option<String>,
    ) -> Result<Self> {
        let repo = &run.worktree;
        let merge_base = corral_git::merge_base(repo, &key.base, &key.head)?;
        let approvals = approvals(ctx, task)?;
        let commits = |older: &str| -> Result<Vec<LandedCommit>> {
            corral_git::commits_between(repo, older, &key.base)?
                .into_iter()
                .map(|c| {
                    let tasks = approvals.get(&c.sha).cloned().unwrap_or_default();
                    Ok(LandedCommit {
                        sha: c.sha,
                        subject: c.subject,
                        tasks,
                    })
                })
                .collect()
        };
        let dispatch_history = commits(&run.base_sha)?;
        let missing_from_candidate = commits(&key.head)?;
        let candidate_changed_paths =
            corral_git::changed_paths_between(repo, &merge_base, &key.head)?;
        let main_changed_paths = corral_git::changed_paths_between(repo, &merge_base, &key.base)?;
        let candidate_paths: BTreeSet<_> = candidate_changed_paths.iter().cloned().collect();
        let main_paths: BTreeSet<_> = main_changed_paths.iter().cloned().collect();
        let overlaps = candidate_paths.intersection(&main_paths).cloned().collect();
        conflict_paths.sort();
        conflict_paths.dedup();
        let paths: BTreeSet<_> = candidate_paths
            .union(&main_paths)
            .cloned()
            .chain(conflict_paths.iter().cloned())
            .collect();
        let mut file_sizes = Vec::new();
        for path in paths {
            file_sizes.push(FileSize::at(repo, &key, path)?);
        }
        let reviewer_note = run
            .review
            .as_ref()
            .filter(|r| r.sha == key.head)
            .and_then(|r| {
                r.notes.as_ref().map(|notes| ReviewerNote {
                    reviewer: r.reviewer.clone(),
                    sha: r.sha.clone(),
                    notes: notes.clone(),
                })
            });
        Ok(Self {
            key,
            dispatch_base: run.base_sha.clone(),
            merge_base,
            dispatch_history,
            missing_from_candidate,
            conflict_paths,
            candidate_changed_paths,
            main_changed_paths,
            overlaps,
            file_sizes,
            sloc_method: "Nonblank lines excluding whole-line // comments; null means absent or binary. Rust ceiling: 800.".into(),
            gate_commands: policy.gate.clone(),
            git_details,
            reviewer_note,
        })
    }

    pub fn compact(&self) -> String {
        let landed: BTreeSet<_> = self
            .missing_from_candidate
            .iter()
            .flat_map(|c| c.tasks.iter().map(|t| &t.id))
            .collect();
        let unattributed = self
            .missing_from_candidate
            .iter()
            .filter(|c| c.tasks.is_empty())
            .count();
        format!(
            "{} conflict paths; {} overlapping paths; {} missing commits ({} recorded tasks, {} unattributed). Full identities, dispatch history, sizes and gate commands are in the packet",
            self.conflict_paths.len(),
            self.overlaps.len(),
            self.missing_from_candidate.len(),
            landed.len(),
            unattributed
        )
    }

    pub fn attach(&self, ctx: &Ctx) -> Result<Artifact> {
        let digest = self.key.digest()?;
        let file = ctx
            .store
            .state_dir()
            .join("integration")
            .join(format!("{digest}.json"));
        std::fs::create_dir_all(
            file.parent()
                .context("integration artifact has no parent")?,
        )
        .context("creating integration packet directory")?;
        // No timestamp or abbreviated/truncated data: identical inputs produce identical artifacts.
        std::fs::write(&file, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("writing {}", file.display()))?;
        crate::artifacts::add_artifact_for_run(
            ctx,
            &self.key.task,
            &self.key.run,
            &file,
            Some(ArtifactKind::Report),
            Some("Integration repair packet"),
        )
    }
}

fn approvals(ctx: &Ctx, candidate: &Task) -> Result<BTreeMap<String, Vec<LandedTask>>> {
    let tasks: BTreeMap<_, _> = ctx
        .store
        .list()?
        .into_iter()
        .map(|t| (t.id.clone(), t))
        .collect();
    let mut approvals: BTreeMap<String, Vec<LandedTask>> = BTreeMap::new();
    for event in ctx.store.events()? {
        if event.kind != "approved" || event.detail["strategy"].as_str() == Some("keep_branch") {
            continue;
        }
        let (Some(id), Some(sha)) = (event.task, event.detail["sha"].as_str()) else {
            continue;
        };
        let task = tasks.get(&id);
        if task.is_some_and(|t| t.project != candidate.project) {
            continue;
        }
        let landed = LandedTask {
            id,
            title: task.map(|t| t.title.clone()),
            run: event.detail["run"].as_str().map(str::to_string),
            approved_sha: sha.to_string(),
        };
        let rows = approvals.entry(sha.to_string()).or_default();
        if !rows.contains(&landed) {
            rows.push(landed);
        }
    }
    for rows in approvals.values_mut() {
        rows.sort_by(|a, b| (&a.id, &a.run).cmp(&(&b.id, &b.run)));
    }
    Ok(approvals)
}
