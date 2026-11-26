//! A tiny local engine for the sample: each operation checks the status table and force rules of
//! the simple engine, refuses with the same typed next steps, or changes the sample records and
//! appends the events the engine would. Nothing leaves this process.

mod pipeline;

use corral_protocol::detail::TaskDetail;
use corral_protocol::op::{OP_SUBMIT, OpReply};
use corral_types::{
    Actor, Blocked, EventKind, NewTask, Next, Op, Origin, Outcome, Refusal, Role, Status,
    TASK_SIZES, Task, TaskEdit, normalize_tags,
};
use serde_json::{Value, json};

use super::rollup::{closed, last_worker, rebuild};
use super::sample::{finish, run};
use super::system_lenses::{draft_task, repair_task};
use super::{ago, event, number, record, records, save, workspace};
use crate::model::act::Act;
use crate::model::daemon::{ClientError, Feed, Operation};
use pipeline::{approve, done, run_gate, send_back, start_review, verdict};

enum Fail {
    Refused(Refusal),
    Missing(String),
    Unsupported(String),
}

type Applied = Result<Option<String>, Fail>;

fn refuse(reason: impl Into<String>, next: Vec<Next>) -> Fail {
    Fail::Refused(Refusal {
        reason: reason.into(),
        next,
        problems: None,
    })
}

/// Run `act` against the sample and record its reply under `act.key`, as the client records an
/// `op.submit` reply. Err only for an operation the sample cannot represent.
pub fn act(feed: &mut Feed, act: &Act) -> Result<(), String> {
    let outcome = apply(feed, &act.op, act.force);
    let mut operation = Operation {
        method: OP_SUBMIT.into(),
        loading: false,
        value: None,
        error: None,
        refusal: None,
    };
    let mut result = Ok(());
    match outcome {
        Ok(task) => {
            rebuild(feed);
            super::pi::sync(feed);
            operation.value = serde_json::to_value(OpReply::Done {
                seq: feed.changes_head,
                task,
            })
            .ok();
        }
        Err(Fail::Refused(refusal)) => {
            operation.error = Some(error("refused", &refusal.reason));
            operation.refusal = Some(refusal);
        }
        Err(Fail::Missing(message)) => operation.error = Some(error("not_found", &message)),
        Err(Fail::Unsupported(message)) => {
            operation.error = Some(error("preview_unsupported", &message));
            result = Err(message);
        }
    }
    feed.operations.insert(act.key.clone(), operation);
    result
}

fn error(code: &str, message: &str) -> ClientError {
    ClientError {
        code: code.into(),
        message: message.into(),
    }
}

fn apply(feed: &mut Feed, op: &Op, force: bool) -> Applied {
    let id = match op {
        Op::New { task } => return new(feed, task),
        Op::Sync => return Ok(None),
        Op::LensDraft {
            project,
            kind,
            about,
            ready,
        } => {
            let task = draft_task(project, kind, about, *ready)
                .map_err(|reason| refuse(reason, Vec::new()))?;
            return new(feed, &task);
        }
        Op::LensRepair { project, id, ready } => {
            let task =
                repair_task(project, id, *ready).map_err(|reason| refuse(reason, Vec::new()))?;
            return new(feed, &task);
        }
        Op::ArtifactAdd { .. } => {
            return Err(Fail::Unsupported(
                "Preview cannot read files on this device; nothing was attached".into(),
            ));
        }
        Op::UsageAdd { .. } => {
            return Err(Fail::Unsupported(
                "Preview usage comes from its sample runs; nothing was recorded".into(),
            ));
        }
        _ => crate::model::act::target(op).unwrap_or_default().to_owned(),
    };
    let mut rec = record(feed, &id).ok_or_else(|| Fail::Missing(format!("task {id} not found")))?;
    match op {
        Op::Edit { edit, .. } => self::edit(feed, rec, edit)?,
        Op::Comment { text, .. } => {
            rec.task.body.push_str("\n\n");
            rec.task.body.push_str(text);
            touch(feed, &mut rec);
            commit(feed, rec, EventKind::Commented, None, json!({"text": text}));
        }
        Op::Move { status, .. } => r#move(feed, rec, *status, force)?,
        Op::Dispatch { profile, base, .. } => dispatch(feed, rec, op, profile, base, force)?,
        Op::Reply { text, .. } => reply(feed, rec, text)?,
        Op::Stop { .. } => stop(feed, rec)?,
        Op::Retry {
            profile,
            fresh,
            note,
            ..
        } => retry(feed, rec, profile.as_deref(), *fresh, note)?,
        Op::Done { summary, .. } => done(feed, rec, summary)?,
        Op::Gate { .. } => run_gate(feed, rec)?,
        Op::Review { profile, .. } => start_review(feed, rec, profile.as_deref())?,
        Op::Verdict { approve, notes, .. } => verdict(feed, rec, *approve, notes.as_deref())?,
        Op::Changes { text, .. } => {
            if !matches!(rec.task.status, Status::Review | Status::Blocked) {
                return Err(refuse(
                    "changes requires a task in Review or Blocked",
                    vec![next_done(&id), next_retry(&id)],
                ));
            }
            rec.task.round = 0;
            send_back(feed, rec, text);
        }
        Op::Approve { onto, .. } => approve(feed, rec, onto.as_deref(), force)?,
        Op::Drop { .. } => {
            let from = rec.task.status;
            set_status(feed, &mut rec, Status::Dropped);
            commit(feed, rec, EventKind::Dropped, None, json!({"from": from}));
        }
        Op::Reopen { .. } => {
            let from = rec.task.status;
            set_status(feed, &mut rec, Status::Backlog);
            commit(feed, rec, EventKind::Reopened, None, json!({"from": from}));
        }
        Op::New { .. }
        | Op::LensDraft { .. }
        | Op::LensRepair { .. }
        | Op::Sync
        | Op::ArtifactAdd { .. }
        | Op::UsageAdd { .. } => {}
    }
    Ok(Some(id))
}

/// The engine's word on a size it is asked to set: one of the scale.
fn off_scale(size: Option<&str>) -> Result<(), Fail> {
    match size {
        Some(size) if !TASK_SIZES.contains(&size) => Err(refuse(
            format!("size {size:?} is not one of xs, s, m, l, xl"),
            Vec::new(),
        )),
        _ => Ok(()),
    }
}

fn new(feed: &mut Feed, task: &NewTask) -> Applied {
    off_scale(task.size.as_deref())?;
    let tags = normalize_tags(task.tags.clone()).map_err(|reason| refuse(reason, Vec::new()))?;
    if !matches!(task.status, Status::Idea | Status::Backlog | Status::Ready) {
        return Err(refuse(
            "a new task starts in Idea, Backlog or Ready",
            vec![Next::run(Op::New {
                task: NewTask {
                    status: Status::Backlog,
                    ..task.clone()
                },
            })],
        ));
    }
    let next = records(feed)
        .iter()
        .map(|record| number(&record.task.id))
        .filter(|number| *number != u64::MAX)
        .max()
        .unwrap_or(0)
        + 1;
    let id = format!("T-{next}");
    let now = ago(feed, 0);
    let rec = TaskDetail {
        task: Task {
            id: id.clone(),
            title: task.title.clone(),
            body: task.body.clone(),
            status: task.status,
            project: task.project.clone(),
            priority: task.priority,
            kind: task.kind.clone(),
            size: task.size.clone(),
            difficulty: task.difficulty,
            tags,
            profile: task.profile.clone(),
            base: task.base.clone(),
            parent: task.parent.clone(),
            depends_on: task.depends_on.clone(),
            blocked: None,
            round: 0,
            created: now,
            updated: now,
        },
        runs: Vec::new(),
        checks: Vec::new(),
        artifacts: Vec::new(),
        events: Vec::new(),
    };
    let detail = json!({"status": task.status, "title": task.title});
    commit(feed, rec, EventKind::Created, None, detail);
    Ok(Some(id))
}

/// Applies an edit as the engine does: a size is checked against the scale only when it
/// changes, so an older task keeps its own.
fn edit(feed: &mut Feed, mut rec: TaskDetail, edit: &TaskEdit) -> Result<(), Fail> {
    if let Some(project) = &edit.project
        && rec.task.status.holds_slot()
        && *project != rec.task.project
    {
        require_slot(feed, &records(feed), project.as_deref(), false)?;
    }
    if let Some(size) = &edit.size
        && *size != rec.task.size
    {
        off_scale(size.as_deref())?;
    }
    let tags = edit
        .tags
        .clone()
        .map(normalize_tags)
        .transpose()
        .map_err(|reason| refuse(reason, Vec::new()))?;
    let task = &mut rec.task;
    let mut fields = Vec::new();
    if let Some(title) = &edit.title {
        task.title = title.clone();
        fields.push("title");
    }
    if let Some(body) = &edit.body {
        task.body = body.clone();
        fields.push("body");
    }
    if let Some(priority) = edit.priority {
        task.priority = priority;
        fields.push("priority");
    }
    if let Some(depends_on) = &edit.depends_on {
        task.depends_on = depends_on.clone();
        fields.push("depends_on");
    }
    if let Some(difficulty) = edit.difficulty {
        task.difficulty = difficulty;
        fields.push("difficulty");
    }
    if let Some(tags) = tags {
        task.tags = tags;
        fields.push("tags");
    }
    for (name, field, value) in [
        ("project", &mut task.project, &edit.project),
        ("kind", &mut task.kind, &edit.kind),
        ("size", &mut task.size, &edit.size),
        ("profile", &mut task.profile, &edit.profile),
        ("base", &mut task.base, &edit.base),
        ("parent", &mut task.parent, &edit.parent),
    ] {
        if let Some(value) = value {
            *field = value.clone();
            fields.push(name);
        }
    }
    touch(feed, &mut rec);
    commit(
        feed,
        rec,
        EventKind::Edited,
        None,
        json!({"fields": fields}),
    );
    Ok(())
}

fn r#move(feed: &mut Feed, mut rec: TaskDetail, status: Status, force: bool) -> Result<(), Fail> {
    let from = rec.task.status;
    let planning = |status| matches!(status, Status::Idea | Status::Backlog | Status::Ready);
    let allowed =
        (planning(from) && planning(status)) || (closed(from) && status == Status::Backlog);
    if !allowed && !force {
        return Err(refuse(
            format!(
                "cannot move {} to {} without force",
                from.as_str(),
                status.as_str()
            ),
            vec![Next::force(Op::Move {
                id: rec.task.id.clone(),
                status,
            })],
        ));
    }
    if status.holds_slot() && !from.holds_slot() {
        require_slot(feed, &records(feed), rec.task.project.as_deref(), true)?;
    }
    set_status(feed, &mut rec, status);
    if status == Status::Blocked {
        rec.task.blocked = Some(Blocked {
            reason: "moved to Blocked by owner".into(),
            detail: None,
            next: vec![next_retry(&rec.task.id)],
        });
    }
    let id = rec.task.id.clone();
    commit(
        feed,
        rec,
        EventKind::Moved,
        None,
        json!({"from": from, "to": status}),
    );
    if force {
        let skipped: Vec<&str> = if allowed {
            Vec::new()
        } else {
            vec!["status rule"]
        };
        forced(
            feed,
            &id,
            None,
            json!({"operation": "move", "from": from, "to": status, "skipped": skipped}),
        );
    }
    Ok(())
}

fn dispatch(
    feed: &mut Feed,
    mut rec: TaskDetail,
    op: &Op,
    profile: &Option<String>,
    base: &Option<String>,
    force: bool,
) -> Result<(), Fail> {
    let id = rec.task.id.clone();
    if !matches!(rec.task.status, Status::Backlog | Status::Ready) {
        let ready = Op::Move {
            id: id.clone(),
            status: Status::Ready,
        };
        return Err(refuse(
            "dispatch requires a Backlog or Ready task, even with --force",
            vec![if rec.task.status == Status::Idea {
                Next::run(ready)
            } else {
                Next::force(ready)
            }],
        ));
    }
    let projects = config(feed, &["projects"]);
    let project = rec
        .task
        .project
        .clone()
        .filter(|project| projects.get(project).is_some());
    let Some(project) = project else {
        let next = projects
            .as_object()
            .map(|projects| {
                projects
                    .keys()
                    .map(|project| {
                        Next::run(Op::Edit {
                            id: id.clone(),
                            edit: TaskEdit {
                                project: Some(Some(project.clone())),
                                ..TaskEdit::default()
                            },
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        return Err(refuse(
            "dispatch requires a configured project; set the task's project or update .corral-next/corral.toml",
            next,
        ));
    };
    let all = records(feed);
    require_slot(feed, &all, Some(&project), true)?;
    let mut skipped = Vec::new();
    let mut next = Vec::new();
    for dependency in &rec.task.depends_on {
        let found = all.iter().find(|record| &record.task.id == dependency);
        if found.is_some_and(|record| record.task.status == Status::Done) {
            continue;
        }
        if skipped.is_empty() {
            skipped.push("dependencies");
        }
        if found.is_some() {
            next.push(Next::force(Op::Move {
                id: dependency.clone(),
                status: Status::Done,
            }));
        }
    }
    if !skipped.is_empty() && !force {
        next.insert(0, Next::force(op.clone()));
        return Err(refuse(
            format!("dispatch blocked by {}", skipped.join(", ")),
            next,
        ));
    }
    let profile = resolve_profile(
        feed,
        &rec,
        profile.as_deref(),
        &projects[&project],
        |profile| Next {
            op: Op::Dispatch {
                id: id.clone(),
                profile: Some(profile),
                base: base.clone(),
            },
            force,
        },
    )?;
    let run_id = start(feed, &mut rec, Role::Worker, &profile);
    let target = base
        .clone()
        .or_else(|| rec.task.base.clone())
        .or_else(|| projects[&project]["base"].as_str().map(str::to_owned));
    if let (Some(target), Some(run)) = (target, rec.runs.last_mut()) {
        run.base = target;
    }
    supersede(feed, &mut rec, &run_id);
    set_status(feed, &mut rec, Status::Running);
    rec.task.round = 0;
    let started = last_worker(&rec).map(|run| json!({"profile": run.profile, "base": run.base}));
    commit(
        feed,
        rec,
        EventKind::Dispatched,
        Some(&run_id),
        started.unwrap_or(Value::Null),
    );
    if force {
        forced(
            feed,
            &id,
            Some(&run_id),
            json!({"operation": "dispatch", "skipped": skipped}),
        );
    }
    Ok(())
}

fn reply(feed: &mut Feed, mut rec: TaskDetail, text: &str) -> Result<(), Fail> {
    let id = rec.task.id.clone();
    let live = last_worker(&rec)
        .filter(|run| run.ended.is_none())
        .map(|run| run.id.clone());
    let Some(run) = live.filter(|_| !closed(rec.task.status)) else {
        return Err(refuse(
            "reply requires a live worker",
            vec![next_retry(&id)],
        ));
    };
    if let Some(worker) = rec.runs.iter_mut().find(|worker| worker.id == run) {
        worker.quiet_since = None;
    }
    let resumed = rec.task.status == Status::Blocked;
    if resumed {
        set_status(feed, &mut rec, Status::Running);
    }
    commit(
        feed,
        rec,
        EventKind::Replied,
        Some(&run),
        json!({"text": text}),
    );
    if resumed {
        event(
            feed,
            Some(&id),
            Some(&run),
            EventKind::Resumed,
            Actor::Owner,
            json!({}),
        );
    }
    // The runner sends the reply as the owner's input: a paused pi session resumes.
    super::pi::deliver(feed, &format!("{id}_{run}"), text, Origin::Owner, None);
    Ok(())
}

fn stop(feed: &mut Feed, mut rec: TaskDetail) -> Result<(), Fail> {
    let id = rec.task.id.clone();
    let Some(run) = rec
        .runs
        .iter()
        .rev()
        .find(|run| run.ended.is_none())
        .map(|run| run.id.clone())
    else {
        let next = if last_worker(&rec).is_some() {
            next_retry(&id)
        } else {
            dispatch_next(&rec.task)
        };
        return Err(refuse("stop requires a live session", vec![next]));
    };
    end_live(feed, &mut rec, Outcome::Stopped);
    let block = matches!(rec.task.status, Status::Running | Status::Blocked);
    let blocked = Blocked {
        reason: "stopped by owner".into(),
        detail: None,
        next: vec![next_retry(&id), next_force_approve(&id)],
    };
    if block {
        set_status(feed, &mut rec, Status::Blocked);
        rec.task.blocked = Some(blocked.clone());
    }
    commit(
        feed,
        rec,
        EventKind::Stopped,
        Some(&run),
        json!({"reason": "stopped by owner"}),
    );
    if block {
        let detail = serde_json::to_value(&blocked).unwrap_or(Value::Null);
        event(
            feed,
            Some(&id),
            Some(&run),
            EventKind::Blocked,
            Actor::Owner,
            detail,
        );
    }
    Ok(())
}

fn retry(
    feed: &mut Feed,
    mut rec: TaskDetail,
    profile: Option<&str>,
    fresh: bool,
    note: &Option<String>,
) -> Result<(), Fail> {
    let Some(last) = last_worker(&rec).cloned() else {
        return Err(refuse(
            "task has not had a worker run",
            vec![dispatch_next(&rec.task)],
        ));
    };
    let profile = profile.map_or(last.profile, str::to_owned);
    if !rec.task.status.holds_slot() {
        require_slot(feed, &records(feed), rec.task.project.as_deref(), true)?;
    }
    let run_id = start(feed, &mut rec, Role::Worker, &profile);
    super::pi_script::follow(feed, &mut rec, &run_id, fresh);
    supersede(feed, &mut rec, &run_id);
    set_status(feed, &mut rec, Status::Running);
    rec.task.round = 0;
    let detail = json!({"profile": profile, "fresh": fresh, "note": note});
    commit(feed, rec, EventKind::Dispatched, Some(&run_id), detail);
    Ok(())
}

/// Start a run of `role`; returns its id.
fn start(feed: &Feed, rec: &mut TaskDetail, role: Role, profile: &str) -> String {
    let started = run(feed, rec, role, profile, 0);
    let id = started.id.clone();
    rec.runs.push(started);
    id
}

/// End the task's other live worker runs, as a new run supersedes them.
fn supersede(feed: &Feed, rec: &mut TaskDetail, keep: &str) {
    for run in rec
        .runs
        .iter_mut()
        .filter(|run| run.role == Role::Worker && run.ended.is_none() && run.id != keep)
    {
        finish(feed, run, 0, Outcome::Superseded);
    }
}

fn end_live(feed: &Feed, rec: &mut TaskDetail, outcome: Outcome) {
    for run in rec.runs.iter_mut().filter(|run| run.ended.is_none()) {
        finish(feed, run, 0, outcome);
    }
}

fn require_slot(
    feed: &Feed,
    records: &[TaskDetail],
    project: Option<&str>,
    check_global: bool,
) -> Result<(), Fail> {
    let slots = workspace::slots(feed, records);
    if check_global && slots.global.used >= slots.global.limit {
        return Err(refuse("no free task slot (limits.running)", Vec::new()));
    }
    if project
        .and_then(|project| slots.projects.get(project))
        .is_some_and(|count| count.used >= count.limit)
    {
        return Err(refuse("no free task slot (project.running)", Vec::new()));
    }
    Ok(())
}

/// Releasing a slot stops live sessions; only Blocked carries a block.
fn set_status(feed: &Feed, rec: &mut TaskDetail, status: Status) {
    if !status.holds_slot() && (rec.task.status.holds_slot() || closed(status)) {
        end_live(feed, rec, Outcome::Stopped);
    }
    rec.task.status = status;
    if status != Status::Blocked {
        rec.task.blocked = None;
    }
    touch(feed, rec);
}

fn touch(feed: &Feed, rec: &mut TaskDetail) {
    rec.task.updated = ago(feed, 0);
}

fn commit(feed: &mut Feed, rec: TaskDetail, kind: EventKind, run: Option<&str>, detail: Value) {
    let id = rec.task.id.clone();
    save(feed, rec);
    event(feed, Some(&id), run, kind, Actor::Owner, detail);
}

fn forced(feed: &mut Feed, id: &str, run: Option<&str>, detail: Value) {
    event(feed, Some(id), run, EventKind::Forced, Actor::Owner, detail);
}

fn resolve_profile(
    feed: &Feed,
    rec: &TaskDetail,
    requested: Option<&str>,
    project: &Value,
    choice: impl Fn(String) -> Next,
) -> Result<String, Fail> {
    let name = requested
        .map(str::to_owned)
        .or_else(|| rec.task.profile.clone())
        .or_else(|| project["profile"].as_str().map(str::to_owned))
        .or_else(|| {
            config(feed, &["defaults", "profile"])
                .as_str()
                .map(str::to_owned)
        });
    let known = profiles(feed);
    match name {
        Some(name) if known.contains(&name) => Ok(name),
        name => Err(refuse(
            match name {
                Some(name) => format!("unknown profile {name}"),
                None => "no worker profile resolves; set --profile".into(),
            },
            known.into_iter().map(choice).collect(),
        )),
    }
}

fn profiles(feed: &Feed) -> Vec<String> {
    config(feed, &["profiles"])
        .as_object()
        .map(|profiles| profiles.keys().cloned().collect())
        .unwrap_or_default()
}

/// A value of the parsed `corral.toml`, or null.
fn config(feed: &Feed, path: &[&str]) -> Value {
    let mut value = feed.settings.as_ref().map_or(Value::Null, |settings| {
        Value::Object(settings.config.clone())
    });
    for key in path {
        value = value.get(*key).cloned().unwrap_or(Value::Null);
    }
    value
}

fn next_retry(id: &str) -> Next {
    Next::run(Op::Retry {
        id: id.into(),
        profile: None,
        fresh: false,
        note: None,
    })
}

fn next_done(id: &str) -> Next {
    Next::run(Op::Done {
        id: id.into(),
        summary: String::new(),
    })
}

fn next_force_approve(id: &str) -> Next {
    Next::force(Op::Approve {
        id: id.into(),
        onto: None,
    })
}

fn dispatch_next(task: &Task) -> Next {
    let dispatch = Op::Dispatch {
        id: task.id.clone(),
        profile: None,
        base: None,
    };
    match task.status {
        Status::Backlog | Status::Ready => Next::run(dispatch),
        Status::Idea => Next::run(Op::Move {
            id: task.id.clone(),
            status: Status::Ready,
        }),
        _ => Next::force(Op::Move {
            id: task.id.clone(),
            status: Status::Ready,
        }),
    }
}
