use super::{content, put, sessions, stamp};
use crate::model::daemon::*;
use corral_protocol::{
    cmd::{assistant::AssistantCommand, plan::PlanCommand, queue::QueueCommand},
    detail::*,
    rows::*,
};

pub(super) fn queue(feed: &mut Feed, command: &QueueCommand) -> Result<(), String> {
    match command {
        QueueCommand::Add { id, .. } | QueueCommand::Remove { id } => {
            let mut row = feed
                .board
                .task(id)
                .cloned()
                .ok_or("Preview queue task does not exist")?;
            if row.held_reason.is_some() || row.group == Group::Parked {
                return Err("Release the preview hold or park first".into());
            }
            let add = matches!(command, QueueCommand::Add { .. });
            row.now_code = if add { "ready" } else { "backlog" }.into();
            row.group = Group::Queued;
            row.now = if add {
                "queued for dispatch · preview"
            } else {
                "removed from dispatch queue · preview"
            }
            .into();
            row.rev += 1;
            content::save_task(feed, row);
            task_note(
                feed,
                id,
                if add {
                    "Added to the local dispatch queue"
                } else {
                    "Removed from the local dispatch queue"
                },
            );
        }
        QueueCommand::Pause {} | QueueCommand::Resume {} => {
            let enabled = matches!(command, QueueCommand::Resume {});
            grant(feed, "queue", enabled, "workspace");
        }
        QueueCommand::Admit { project, off } => grant(feed, "queue.admit", !off, project),
        _ => return Err("Unknown preview queue command".into()),
    }
    Ok(())
}

pub(super) fn focus(feed: &mut Feed, id: Option<&str>, all: bool) -> Result<(), String> {
    if let Some(id) = id {
        let mut row = feed
            .board
            .task(id)
            .cloned()
            .ok_or("Preview focus task does not exist")?;
        if row.now_code == "done" || row.now_code == "cancelled" {
            return Err("A terminal preview task cannot be focused".into());
        }
        row.held_reason = None;
        if row.group == Group::Held {
            row.group = Group::Queued;
        }
        row.now = "focused · preview".into();
        row.rev += 1;
        content::save_task(feed, row);
        sessions::sync_worker(feed, id);
        task_note(
            feed,
            id,
            if all {
                "Owner focus · all projects"
            } else {
                "Owner focus · this project"
            },
        );
    }
    grant(feed, "focus", id.is_some(), id.unwrap_or("workspace"));
    Ok(())
}

pub(super) fn revert(feed: &mut Feed, id: &str) -> Result<(), String> {
    let mut row = feed
        .board
        .task(id)
        .cloned()
        .ok_or("Preview landed task does not exist")?;
    if row.now_code != "done" {
        return Err("Only a landed preview task can be reverted".into());
    }
    row.now_code = "backlog".into();
    row.group = Group::Held;
    row.held_reason = Some("Preview revert · dispatch held until owner release".into());
    row.now = "reverted locally · backlog and held".into();
    row.rail = content::rail(Group::Queued);
    row.rev += 1;
    content::save_task(feed, row);
    sessions::sync_worker(feed, id);
    task_note(
        feed,
        id,
        "Preview · simulated reverting the landed range; no Git repository was changed",
    );
    Ok(())
}

/// `review.discard`: the task is dropped and its run worktree removed; artifacts stay.
pub(super) fn discard(feed: &mut Feed, id: &str) -> Result<(), String> {
    let mut row = feed
        .board
        .task(id)
        .cloned()
        .ok_or("Preview task does not exist")?;
    if matches!(row.now_code.as_str(), "done" | "dropped") {
        return Err("A landed or dropped preview task cannot be discarded".into());
    }
    row.now_code = "dropped".into();
    row.group = Group::Other;
    row.held_reason = None;
    row.now = "dropped locally · run worktree removed".into();
    row.rail = content::rail(Group::Other);
    row.rev += 1;
    content::save_task(feed, row);
    sessions::sync_worker(feed, id);
    task_note(
        feed,
        id,
        "Preview · simulated dropping the task and removing its run worktree; artifacts kept",
    );
    Ok(())
}

pub(super) fn membership(feed: &mut Feed, command: &PlanCommand) -> Result<(), String> {
    let (tracker, member, destination) = match command {
        PlanCommand::AddMember {
            tracker, member, ..
        } => (tracker, member, None),
        PlanCommand::MoveMember {
            tracker,
            member,
            to,
            ..
        } => (tracker, member, Some(to)),
        _ => return Err("Unknown preview plan membership operation".into()),
    };
    let source_ref = DetailRef::Plan {
        id: tracker.clone(),
    };
    let Some(DetailBody::Plan(mut source)) = feed
        .detail(&source_ref)
        .and_then(|cache| cache.body())
        .cloned()
    else {
        return Err("Preview source plan does not exist".into());
    };
    if source.role != Some(PlanRole::Tracker) {
        return Err("Members can only be published to preview trackers".into());
    }
    let mut task = feed
        .board
        .task(member)
        .cloned()
        .ok_or("Preview member task does not exist")?;
    if let Some(destination) = destination {
        if destination == tracker {
            return Err("Choose a different preview destination tracker".into());
        }
        let member_index = source
            .members
            .iter()
            .position(|row| &row.id == member)
            .ok_or("Task is not a member of the preview source tracker")?;
        let destination_ref = DetailRef::Plan {
            id: destination.clone(),
        };
        let Some(DetailBody::Plan(mut target)) = feed
            .detail(&destination_ref)
            .and_then(|cache| cache.body())
            .cloned()
        else {
            return Err("Preview destination tracker does not exist".into());
        };
        if target.role != Some(PlanRole::Tracker) {
            return Err("Preview destination is not a tracker".into());
        }
        if target.members.iter().any(|row| &row.id == member) {
            return Err("Preview destination already contains this member".into());
        }
        let mut row = source.members.remove(member_index);
        row.layer = Some(0);
        target.members.push(row);
        source
            .edges
            .retain(|edge| &edge.from != member && &edge.to != member);
        task.plan = Some(destination.clone());
        put(feed, destination_ref, DetailBody::Plan(target));
    } else {
        if source.members.iter().any(|row| &row.id == member) {
            return Err("Preview tracker already contains this member".into());
        }
        if task.plan.as_deref().is_some_and(|plan| plan != tracker) {
            return Err("Use Move member to transfer a task from another preview tracker".into());
        }
        source.members.push(PlanMember {
            id: member.clone(),
            title: task.title.clone(),
            layer: Some(0),
            state: task.now_code.clone(),
            rail: content::stages(&task),
        });
        task.plan = Some(tracker.clone());
    }
    put(feed, source_ref, DetailBody::Plan(source));
    let task_ref = DetailRef::Task { id: member.clone() };
    if let Some(DetailBody::Task(mut detail)) = feed
        .detail(&task_ref)
        .and_then(|cache| cache.body())
        .cloned()
    {
        detail.plan_context = None;
        put(feed, task_ref, DetailBody::Task(detail));
    }
    task.rev += 1;
    content::save_task(feed, task);
    for id in std::iter::once(tracker).chain(destination) {
        if let Some(row) = feed.plans.get_mut(id.as_str()) {
            row.rev += 1;
        }
    }
    Ok(())
}

pub(super) fn permissions(feed: &mut Feed, command: &AssistantCommand) -> Result<(), String> {
    let AssistantCommand::PermissionsRevoke {
        id,
        grant: requested,
        all,
        ..
    } = command
    else {
        return Err("This preview assistant command is not offered".into());
    };
    let session = if id == "quick" { "A-7" } else { id };
    if !feed.assistants.contains_key(session) {
        return Err("Preview assistant session does not exist".into());
    }
    let key = format!(
        "assistant.permissions.{}",
        if *all {
            "all"
        } else {
            requested.as_deref().unwrap_or("all")
        }
    );
    grant(feed, &key, false, session);
    if let Some(row) = feed.assistants.get_mut(session) {
        row.rev += 1;
    }
    Ok(())
}

fn grant(feed: &mut Feed, key: &str, enabled: bool, scope: &str) {
    for settings in feed.settings.values_mut() {
        if let Some(grant) = settings.grants.iter_mut().find(|grant| grant.key == key) {
            grant.enabled = enabled;
            grant.scope = Some(scope.into());
        } else {
            settings.grants.push(GrantLine {
                key: key.into(),
                enabled,
                scope: Some(scope.into()),
                expires_at: None,
            });
        }
        settings.rev += 1;
        settings.config_hash = format!("preview-config-{}", settings.rev);
    }
}

pub(super) fn task_note(feed: &mut Feed, id: &str, text: &str) {
    let target = DetailRef::Task { id: id.into() };
    if let Some(DetailBody::Task(mut detail)) =
        feed.detail(&target).and_then(|cache| cache.body()).cloned()
    {
        detail
            .log_tail
            .push(format!("{} · Preview · {text}", stamp(feed, feed.now)));
        put(feed, target, DetailBody::Task(detail));
    }
}

pub(super) fn session_task(
    feed: &mut Feed,
    session: &str,
    task: Option<&str>,
    status: AgentStatus,
    relaunched: bool,
) {
    if session == "PM"
        && let Some(health) = &mut feed.health
    {
        health.pm = if status == AgentStatus::Ended {
            PmLevel::Absent
        } else {
            PmLevel::Live
        };
    }
    let Some(task) = task else { return };
    let Some(mut row) = feed.board.task(task).cloned() else {
        return;
    };
    let key = format!("preview:session-ended:{task}");
    if status == AgentStatus::Ended {
        row.group = Group::NeedsYou;
        row.now_code = "needs_me".into();
        row.now = "session ended · owner action needed".into();
        if !row.attention.contains(&key) {
            row.attention.push(key.clone());
        }
        feed.inbox.insert(
            key.clone().into(),
            InboxRow {
                id: key,
                kind: InboxKind::Stuck,
                task: Some(task.into()),
                plan: row.plan.clone(),
                project: row.project.clone(),
                title: format!("Preview · {task} session ended"),
                from: format!("preview session {session}"),
                asked_at: Some(stamp(feed, feed.now)),
                needed_by: None,
                blocking: true,
                state: InboxState::Open,
                question_rev: None,
                answerable: false,
                unanswerable_reason: Some("Relaunch the local preview session from Agents".into()),
                attention: Some(AttentionMove {
                    whose_move: AttentionActor::Owner,
                    reason: "Session ended".into(),
                    next_step: "Relaunch or dispatch the task".into(),
                    command: None,
                    destination: AttentionDestination::Now,
                }),
                rev: row.rev + 1,
            },
        );
    } else if relaunched {
        feed.inbox.remove(key.as_str());
        row.attention.retain(|attention| attention != &key);
        row.group = if row.questions > 0 {
            Group::NeedsYou
        } else {
            Group::Producing
        };
        row.now_code = "running".into();
        row.now = "session relaunched · preview".into();
    } else {
        return;
    }
    row.rev += 1;
    content::save_task(feed, row);
    let target = DetailRef::Task { id: task.into() };
    if let Some(DetailBody::Task(mut detail)) =
        feed.detail(&target).and_then(|cache| cache.body()).cloned()
    {
        if let Some(run) = &mut detail.run {
            run.ended_at = (status == AgentStatus::Ended).then(|| stamp(feed, feed.now));
        }
        put(feed, target, DetailBody::Task(detail));
    }
}
