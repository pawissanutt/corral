//! Project grants: every unfinished task of a granted project, including tasks filed after
//! confirmation, bounded by an expiry and an optional cap on new tasks.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use super::{
    OperatingApproval, OperatingScope, OperatingSelection, OperatingTaskConstraints, ScopedTask,
    digest, file_identity, task_constraints,
};
use crate::backend::{HeadQuery, PageReq, TaskHeader};
use crate::work::{self, Counter, Subject};
use crate::{Config, Status, Store, Task, TaskHead};

pub const DEFAULT_GRANT_DAYS: u32 = 7;

/// The repository a project grant was confirmed against; a moved or replaced repository
/// stops covering its tasks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopedProject {
    pub repository: PathBuf,
    pub repository_identity: String,
}

/// When a grant with an expiry stops; `None` for grants without one.
pub(super) fn expiry(scope: &OperatingScope, approval: &OperatingApproval) -> Option<Timestamp> {
    let days = scope.expires_after_days?;
    approval
        .confirmed_at
        .checked_add(SignedDuration::from_hours(i64::from(days) * 24))
        .ok()
}

fn covered_now(scope: &OperatingScope, task: &Task) -> bool {
    task.project
        .as_ref()
        .is_some_and(|project| scope.projects.contains_key(project))
        && !scope.tasks.contains_key(&task.id)
        && !scope.exclusions.contains(&task.id)
        && !matches!(task.status, Status::Done | Status::Dropped)
}

impl Store {
    pub(crate) fn project_identity(
        &self,
        config: &Config,
        project: &str,
    ) -> Result<(PathBuf, String)> {
        let configured = config
            .projects
            .get(project)
            .context("unknown task project")?;
        // Every member of a grant asks again; within one read-only pass the repository is
        // resolved once per configured path.
        let key = format!("project_identity\0{project}\0{:?}", configured.path);
        let identity = self.pass_derived(&key, || {
            self.resolve_project_identity(project, &configured.path)
        })?;
        Ok(identity.as_ref().clone())
    }

    fn resolve_project_identity(
        &self,
        project: &str,
        path: &impl AsRef<std::path::Path>,
    ) -> Result<(PathBuf, String)> {
        crate::work::record(crate::work::Work::ProjectIdentity, 1);
        let repository = self
            .root()
            .join(path)
            .canonicalize()
            .with_context(|| format!("resolving project {project}"))?;
        let git = repository.join(".git");
        let git_identity = match fs::symlink_metadata(&git) {
            Ok(_) => {
                let metadata = fs::metadata(&git).context("reading repository marker metadata")?;
                let pointer = if metadata.is_file() {
                    let mut bytes = Vec::new();
                    fs::File::open(&git)
                        .context("opening repository pointer")?
                        .take(4097)
                        .read_to_end(&mut bytes)
                        .context("reading repository pointer")?;
                    ensure!(bytes.len() <= 4096, "repository pointer exceeds 4096 bytes");
                    Some(digest(&bytes)?)
                } else {
                    ensure!(
                        metadata.is_dir(),
                        "repository marker is not a file or directory"
                    );
                    None
                };
                Some((
                    git.canonicalize().context("resolving repository marker")?,
                    file_identity(&git)?,
                    pointer,
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error).context("identifying repository marker"),
        };
        let identity = digest(&(file_identity(&repository)?, git_identity))?;
        Ok((repository, identity))
    }

    pub(super) fn preview_projects(
        &self,
        config: &Config,
        selection: &OperatingSelection,
        scope: &mut OperatingScope,
        constraints: &mut BTreeMap<String, OperatingTaskConstraints>,
    ) -> Result<()> {
        if selection.projects.is_empty() {
            return Ok(());
        }
        let days = selection.expires_after_days.unwrap_or(DEFAULT_GRANT_DAYS);
        ensure!(
            (1..=365).contains(&days),
            "a project grant must expire within 1 to 365 days"
        );
        for project in &selection.projects {
            let (repository, repository_identity) = self.project_identity(config, project)?;
            scope.projects.insert(
                project.clone(),
                ScopedProject {
                    repository,
                    repository_identity,
                },
            );
        }
        scope.expires_after_days = Some(days);
        scope.max_new_tasks = selection.max_new_tasks;
        for task in self.list()? {
            if covered_now(scope, &task) {
                constraints.insert(task.id.clone(), task_constraints(config, &task));
            }
        }
        Ok(())
    }

    /// Live members of project grants. Tasks filed after confirmation count toward the cap even
    /// once finished, so the cap is a budget of new work, not a concurrency limit.
    pub(super) fn project_members(
        &self,
        config: &Config,
        reasons: &mut Vec<String>,
    ) -> BTreeMap<String, ScopedTask> {
        let scope = &config.automation.scope;
        let mut members = BTreeMap::new();
        if scope.projects.is_empty() {
            return members;
        }
        let since = match self.operating_state() {
            Ok(state) => state.approval.map(|approval| approval.confirmed_at),
            Err(error) => {
                reasons.push(format!("project grant state unavailable: {error:#}"));
                return members;
            }
        };
        let mut tasks = match self.project_member_heads(scope, since) {
            Ok(tasks) => tasks,
            Err(error) => {
                reasons.push(format!("project grant tasks unavailable: {error:#}"));
                return members;
            }
        };
        // Oldest first, so a cap covers the earliest new work and never reshuffles.
        tasks.sort_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));
        let mut moved = BTreeSet::new();
        for task in tasks {
            let Some(granted) = task
                .project
                .as_ref()
                .and_then(|project| scope.projects.get(project))
            else {
                continue;
            };
            if scope.tasks.contains_key(&task.id) || scope.exclusions.contains(&task.id) {
                continue;
            }
            match self.operating_member_head(config, &task) {
                Ok(member)
                    if member.repository == granted.repository
                        && member.repository_identity == granted.repository_identity =>
                {
                    members.insert(task.id.clone(), member);
                }
                Ok(member) => {
                    moved.insert(member.project);
                }
                Err(error) => reasons.push(format!("{}: {error:#}", task.id)),
            }
        }
        for project in moved {
            reasons.push(format!(
                "project {project} repository changed since the grant"
            ));
        }
        members
    }

    fn project_member_heads(
        &self,
        scope: &OperatingScope,
        since: Option<Timestamp>,
    ) -> Result<Vec<TaskHead>> {
        let admitted = match (scope.max_new_tasks, since) {
            (Some(cap), Some(since)) => Some(self.capped_project_tasks(scope, since, cap)?),
            _ => None,
        };
        let mut tasks = Vec::new();
        for project in scope.projects.keys() {
            tasks.extend(
                self.heads(&HeadQuery::Open {
                    project: Some(project.clone()),
                })?
                .into_iter()
                .filter(|task| {
                    !scope.tasks.contains_key(&task.id)
                        && !scope.exclusions.contains(&task.id)
                        && admitted.as_ref().is_none_or(|ids| {
                            since.is_none_or(|since| task.created <= since)
                                || ids.contains(&task.id)
                        })
                }),
            );
        }
        Ok(tasks)
    }

    /// Merge one lookahead per granted project. Terminal rows consume the same global budget
    /// as open rows; explicit members and exclusions consume none. Never refill after the cap.
    fn capped_project_tasks(
        &self,
        scope: &OperatingScope,
        since: Timestamp,
        cap: u32,
    ) -> Result<BTreeSet<String>> {
        let mut admitted = BTreeSet::new();
        if cap == 0 {
            return Ok(admitted);
        }
        let view = self.view()?;
        let mut streams = scope
            .projects
            .keys()
            .map(|project| {
                (
                    HeadQuery::ProjectCreatedAfter {
                        project: project.clone(),
                        after: since,
                    },
                    PageReq::first(1),
                )
            })
            .collect::<Vec<_>>();
        let mut pending = BTreeMap::new();
        let read = |query: &HeadQuery, page: &mut PageReq| -> Result<Option<TaskHeader>> {
            let found = view.heads(query, page.clone())?;
            page.after = found.next;
            let Some(doc) = found.items.into_iter().next() else {
                return Ok(None);
            };
            work::decoded(
                Counter::TaskHeadDecodes,
                u64::try_from(
                    serde_json::to_vec(&doc.doc)?.len() + serde_json::to_vec(&doc.summary)?.len(),
                )?,
                Subject::Plain,
            );
            Ok(Some(doc.header))
        };
        for (index, (query, page)) in streams.iter_mut().enumerate() {
            if let Some(task) = read(query, page)? {
                pending.insert((task.created, task.id), index);
            }
        }
        let mut charged = 0u32;
        while let Some(((_, id), index)) = pending.pop_first() {
            if !scope.tasks.contains_key(&id) && !scope.exclusions.contains(&id) {
                admitted.insert(id);
                charged += 1;
                if charged == cap {
                    break;
                }
            }
            let (query, page) = &mut streams[index];
            if page.after.is_some()
                && let Some(task) = read(query, page)?
            {
                pending.insert((task.created, task.id), index);
            }
        }
        Ok(admitted)
    }
}
