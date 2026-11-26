use std::{borrow::Cow, path::PathBuf};

use anyhow::{Context, Result};
use corral_types::{TASK_SIZES, normalize_tags};
use jiff::Timestamp;
use serde_json::json;

use crate::{
    Applied, ArtifactKind, Engine, Next, Op, Run, Runner,
    config::EffectiveProject,
    error::refuse,
    events::{EventKind, NewEvent},
    model::{Actor, Blocked, Outcome, Status, Task},
    store::Store,
};

pub use corral_types::{NewTask, TaskEdit};

impl Engine {
    pub fn new_task(&self, new: NewTask) -> Result<Task> {
        self.store
            .transaction(|store| self.insert_new_task(store, new, &[]))
    }

    /// Publish the task and its reference attachments together, so a ready task
    /// cannot be dispatched before the artifacts in its body are available.
    pub fn file_task(&self, new: NewTask, paths: &[PathBuf]) -> Result<(Applied, Vec<String>)> {
        self.store.transaction(|store| {
            let task = self.insert_new_task(store, new, &[])?;
            let references = self.attach_references_in(store, &task.id, paths)?;
            let mut applied = crate::applied_task(task);
            applied.seq = store.latest_seq()?;
            Ok((applied, references))
        })
    }

    fn attach_references_in(
        &self,
        store: &Store,
        id: &str,
        paths: &[PathBuf],
    ) -> Result<Vec<String>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let run = self.artifact_run(store, id)?;
        paths
            .iter()
            .map(|path| {
                self.add_artifact_file_in(
                    store,
                    self.artifact_target(id, run.as_deref()),
                    path,
                    ArtifactKind::Reference,
                    None,
                    Some(self.config.artifacts.max_bytes),
                )
                .map(|artifact| artifact.reference())
            })
            .collect()
    }

    pub(crate) fn message_with_attachments<'a>(
        &self,
        store: &Store,
        id: &str,
        text: &'a str,
        paths: &[PathBuf],
    ) -> Result<Cow<'a, str>> {
        let references = self.attach_references_in(store, id, paths)?;
        if references.is_empty() {
            return Ok(Cow::Borrowed(text));
        }
        let mut message = text.to_owned();
        if !text.is_empty() {
            message.push_str("\n\n");
        }
        message.push_str(&references.join("\n"));
        Ok(Cow::Owned(message))
    }

    /// The caller holds the creating transaction, including any dedup lookup.
    pub(crate) fn insert_new_task(
        &self,
        store: &Store,
        new: NewTask,
        lens_keys: &[String],
    ) -> Result<Task> {
        if !planning(new.status) {
            return Err(refuse(
                "new tasks must start in idea, backlog, or ready",
                vec![Next::run(Op::New {
                    task: NewTask {
                        status: Status::Backlog,
                        ..new
                    },
                })],
            ));
        }
        validate_size(new.size.as_deref())?;
        let tags = normalize_tags(new.tags).map_err(|reason| refuse(reason, vec![]))?;
        let now = Timestamp::now();
        let task = Task {
            id: store.next_task_id()?,
            title: new.title,
            body: new.body,
            status: new.status,
            project: new.project,
            priority: new.priority,
            kind: new.kind,
            size: new.size,
            difficulty: new.difficulty,
            tags,
            profile: new.profile,
            base: new.base,
            parent: new.parent,
            depends_on: new.depends_on,
            blocked: None,
            attention: None,
            round: 0,
            created: now,
            updated: now,
        };
        store.insert_task(&task)?;
        let mut detail = json!({"status":task.status,"title":task.title});
        if !lens_keys.is_empty() {
            detail["lens_task_keys"] = json!(lens_keys);
        }
        self.task_event(store, &task.id, EventKind::Created, detail)?;
        Ok(task)
    }

    /// Retarget an untouched lens draft without creating a second open task.
    /// The caller holds the writer transaction across both lookup and retargeting.
    pub(crate) fn retarget_lens_draft(
        &self,
        store: &Store,
        mut task: Task,
        repair: NewTask,
        lens_keys: &[String],
    ) -> Result<Task> {
        if !planning(task.status)
            || task.created != task.updated
            || !store.list_runs(&task.id)?.is_empty()
            || !matches!(
                store.event_page(Some(&task.id), None, None, true, 2)?.as_slice(),
                [event] if event.kind == EventKind::Created
                    && event.detail["lens_task_keys"].as_array().is_some_and(|keys| {
                        matches!(keys.as_slice(), [key] if key.as_str().is_some_and(|key| key.starts_with("draft:")))
                    })
            )
        {
            // An existing task satisfies deduplication, but owner edits and runs
            // must never be overwritten by an automatic retarget.
            return Ok(task);
        }
        task.title = repair.title;
        task.body = repair.body;
        self.touch(&mut task);
        store.update_task(&task)?;
        self.task_event(
            store,
            &task.id,
            EventKind::Edited,
            json!({"title":task.title, "body":task.body, "lens_task_keys":lens_keys}),
        )?;
        Ok(task)
    }

    pub fn edit_task(&self, id: &str, mut edit: TaskEdit) -> Result<Task> {
        self.store.transaction(|store| {
            let mut task = store.require_task(id)?;
            if let Some(size) = &edit.size
                && size != &task.size
            {
                validate_size(size.as_deref())?;
            }
            edit.tags = edit
                .tags
                .map(normalize_tags)
                .transpose()
                .map_err(|reason| refuse(reason, vec![]))?;
            let detail = serde_json::to_value(&edit)?;
            if let Some(value) = edit.title {
                task.title = value;
            }
            if let Some(value) = edit.body {
                task.body = value;
            }
            if let Some(value) = edit.project {
                let from = std::mem::replace(&mut task.project, value);
                if task.status.holds_slot() && task.project != from {
                    let project = self.project_for(&task)?;
                    self.require_project_slot(store, task.project.as_deref(), project.as_ref())?;
                }
            }
            if let Some(value) = edit.priority {
                task.priority = value;
            }
            if let Some(value) = edit.kind {
                task.kind = value;
            }
            if let Some(value) = edit.size {
                task.size = value;
            }
            if let Some(value) = edit.difficulty {
                task.difficulty = value;
            }
            if let Some(value) = edit.tags {
                task.tags = value;
            }
            if let Some(value) = edit.profile {
                task.profile = value;
            }
            if let Some(value) = edit.base {
                task.base = value;
            }
            if let Some(value) = edit.parent {
                task.parent = value;
            }
            if let Some(value) = edit.depends_on {
                task.depends_on = value;
            }
            self.touch(&mut task);
            store.update_task(&task)?;
            self.task_event(store, id, EventKind::Edited, detail)?;
            Ok(task)
        })
    }

    pub fn comment(&self, id: &str, text: &str) -> Result<Task> {
        self.comment_with_attachments(id, text, &[])
    }

    pub fn comment_with_attachments(
        &self,
        id: &str,
        text: &str,
        paths: &[PathBuf],
    ) -> Result<Task> {
        self.store.transaction(|store| {
            let mut task = store.require_task(id)?;
            let text = self.message_with_attachments(store, id, text, paths)?;
            if !task.body.is_empty() {
                task.body.push_str("\n\n");
            }
            task.body.push_str(&text);
            self.touch(&mut task);
            store.update_task(&task)?;
            self.task_event(store, id, EventKind::Commented, json!({"text":text}))?;
            Ok(task)
        })
    }

    pub fn move_task(&self, id: &str, status: Status, force: bool) -> Result<Task> {
        let move_op = || Op::Move {
            id: id.to_owned(),
            status,
        };
        self.check_force(force, move_op)?;
        let task = self.store.transaction(|store| {
            let mut task = store.require_task(id)?;
            let allowed = (planning(task.status) && planning(status))
                || (matches!(task.status, Status::Done | Status::Dropped)
                    && status == Status::Backlog);
            if !allowed && !force {
                return Err(refuse(
                    format!(
                        "cannot move {} to {} without force",
                        task.status.as_str(),
                        status.as_str()
                    ),
                    vec![Next::force(move_op())],
                ));
            }
            let from = task.status;
            self.set_status(store, &mut task, status)?;
            self.task_event(
                store,
                id,
                EventKind::Moved,
                json!({"from":from,"to":status}),
            )?;
            if force {
                let skipped: Vec<&str> = if allowed {
                    Vec::new()
                } else {
                    vec!["status rule"]
                };
                self.task_event(
                    store,
                    id,
                    EventKind::Forced,
                    json!({"operation":"move","from":from,"to":status,"skipped":skipped}),
                )?;
            }
            Ok(task)
        })?;
        self.clean_terminal_pipeline_lock(&task)?;
        Ok(task)
    }

    pub fn drop_task(&self, id: &str) -> Result<Task> {
        let task = self.store.transaction(|store| {
            let mut task = store.require_task(id)?;
            let from = task.status;
            self.set_status(store, &mut task, Status::Dropped)?;
            self.task_event(store, id, EventKind::Dropped, json!({"from":from}))?;
            Ok(task)
        })?;
        self.clean_terminal_pipeline_lock(&task)?;
        Ok(task)
    }

    pub fn reopen(&self, id: &str) -> Result<Task> {
        self.store.transaction(|store| {
            let mut task = store.require_task(id)?;
            let from = task.status;
            self.set_status(store, &mut task, Status::Backlog)?;
            self.task_event(store, id, EventKind::Reopened, json!({"from":from}))?;
            Ok(task)
        })
    }

    // Call inside the write transaction, only when acquiring a slot (not when
    // resuming a task that already holds one, even if the limit was lowered).
    pub(crate) fn require_dispatch_slot(
        &self,
        store: &Store,
        project_name: Option<&str>,
        project: Option<&EffectiveProject>,
    ) -> Result<()> {
        if store.occupied_slots(None)? >= self.config.limits.running {
            return Err(refuse("no free task slot (limits.running)", vec![]));
        }
        self.require_project_slot(store, project_name, project)
    }

    pub(crate) fn require_project_slot(
        &self,
        store: &Store,
        project_name: Option<&str>,
        project: Option<&EffectiveProject>,
    ) -> Result<()> {
        if let Some(limit) = project.and_then(|p| p.running)
            && store.occupied_slots(project_name)? >= limit
        {
            return Err(refuse("no free task slot (project.running)", vec![]));
        }
        Ok(())
    }

    fn set_status(&self, store: &Store, task: &mut Task, status: Status) -> Result<()> {
        if status.holds_slot() && !task.status.holds_slot() {
            let project = self.project_for(task)?;
            self.require_dispatch_slot(store, task.project.as_deref(), project.as_ref())?;
        }
        if !status.holds_slot()
            && (task.status.holds_slot() || matches!(status, Status::Done | Status::Dropped))
        {
            let reason = if matches!(status, Status::Done | Status::Dropped) {
                "terminal task move"
            } else {
                "task returned to planning"
            };
            for mut run in store.live_runs_for_task(&task.id)? {
                self.end_run(store, &mut run, Outcome::Stopped, true)?;
                store.append_event(&NewEvent {
                    task: Some(task.id.clone()),
                    run: Some(run.id),
                    kind: EventKind::Stopped,
                    actor: self.actor,
                    detail: json!({"reason":reason}),
                })?;
            }
        }
        task.status = status;
        task.blocked = if status == Status::Blocked {
            Some(Blocked {
                reason: "moved to blocked".to_owned(),
                detail: None,
                next: vec![Next::force(Op::Move {
                    id: task.id.clone(),
                    status: Status::Backlog,
                })],
            })
        } else {
            None
        };
        self.touch(task);
        store.update_task(task)
    }

    fn touch(&self, task: &mut Task) {
        task.updated = Timestamp::now();
        if self.actor == Actor::Owner {
            task.round = 0;
        }
    }

    fn task_event(
        &self,
        store: &Store,
        id: &str,
        kind: EventKind,
        detail: serde_json::Value,
    ) -> Result<()> {
        self.event(store, Some(id), None, kind, self.actor, detail)
    }

    pub(crate) fn event(
        &self,
        store: &Store,
        task: Option<&str>,
        run: Option<&str>,
        kind: EventKind,
        actor: Actor,
        detail: serde_json::Value,
    ) -> Result<()> {
        store.append_event(&NewEvent {
            task: task.map(str::to_owned),
            run: run.map(str::to_owned),
            kind,
            actor,
            detail,
        })?;
        Ok(())
    }

    pub(crate) fn check_force(&self, force: bool, op: impl FnOnce() -> Op) -> Result<()> {
        if force && self.actor == Actor::Auto {
            return Err(refuse("auto never forces", vec![Next::force(op())]));
        }
        Ok(())
    }

    pub(crate) fn end_run(
        &self,
        store: &Store,
        run: &mut Run,
        outcome: Outcome,
        stop: bool,
    ) -> Result<()> {
        if stop && let Some(session) = &run.session {
            self.session_runner()
                .stop(session)
                .with_context(|| format!("stop session {}:{}", run.task, run.id))?;
        }
        run.ended = Some(Timestamp::now());
        run.outcome = Some(outcome);
        run.quiet_since = None;
        store.update_run(run)
    }

    pub(crate) fn block_in(
        &self,
        store: &Store,
        task: &mut Task,
        run: Option<&str>,
        blocked: Blocked,
        actor: Actor,
    ) -> Result<()> {
        if task.blocked.as_ref() == Some(&blocked) {
            return Ok(());
        }
        self.event(
            store,
            Some(&task.id),
            run,
            EventKind::Blocked,
            actor,
            serde_json::to_value(&blocked).context("encode blocked event")?,
        )?;
        set_lifecycle(task, Status::Blocked);
        task.blocked = Some(blocked);
        store.update_task(task)
    }
}

pub(crate) fn set_lifecycle(task: &mut Task, status: Status) {
    task.status = status;
    task.blocked = None;
    task.attention = None;
    task.updated = Timestamp::now();
}

pub(crate) fn next_reply(id: &str) -> Next {
    Next::run(Op::Reply {
        id: id.to_owned(),
        text: String::new(),
    })
}

pub(crate) fn next_retry(id: &str) -> Next {
    Next::run(Op::Retry {
        id: id.to_owned(),
        profile: None,
        fresh: false,
        note: None,
    })
}

pub(crate) fn next_review(id: &str) -> Next {
    Next::run(Op::Review {
        id: id.to_owned(),
        profile: None,
    })
}

pub(crate) fn next_changes(id: &str) -> Next {
    Next::run(Op::Changes {
        id: id.to_owned(),
        text: String::new(),
    })
}

pub(crate) fn next_force_approve(id: &str) -> Next {
    Next::force(Op::Approve {
        id: id.to_owned(),
        onto: None,
    })
}

fn validate_size(size: Option<&str>) -> Result<()> {
    if let Some(size) = size
        && !TASK_SIZES.contains(&size)
    {
        return Err(refuse(
            format!("unknown size {size:?} ({})", TASK_SIZES.join(", ")),
            vec![],
        ));
    }
    Ok(())
}

fn planning(status: Status) -> bool {
    matches!(status, Status::Idea | Status::Backlog | Status::Ready)
}

#[cfg(test)]
#[path = "tasks_tests.rs"]
mod tests;
