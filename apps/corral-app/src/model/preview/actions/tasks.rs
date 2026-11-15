use super::*;

pub(super) fn task(feed: &mut Feed, command: &TaskCmd) -> Result<(), String> {
    match command {
        TaskCmd::Focus { id, all } => return super::control::focus(feed, id.as_deref(), *all),
        TaskCmd::Revert { id } => return super::control::revert(feed, id),
        _ => {}
    }
    if let TaskCmd::New {
        title,
        body,
        project,
        status,
        ..
    } = command
    {
        let next = feed
            .board
            .tasks()
            .iter()
            .filter_map(|task| {
                task.id
                    .strip_prefix("T-")
                    .and_then(|number| number.parse::<u64>().ok())
            })
            .max()
            .unwrap_or(903)
            + 1;
        let id = format!("T-{next}");
        let mut row = content::task(feed, &id, &title.join(" "), None, Group::Queued, status);
        row.goal = Some(body.clone());
        row.project = project.clone();
        row.now_code = status.to_lowercase();
        content::save_task(feed, row);
        return Ok(());
    }
    let id = match command {
        TaskCmd::Hold { id, .. }
        | TaskCmd::Release { id }
        | TaskCmd::Move { id, .. }
        | TaskCmd::Edit { id, .. }
        | TaskCmd::Dispatch { id, .. }
        | TaskCmd::Retry { id, .. }
        | TaskCmd::Reply { id, .. }
        | TaskCmd::Comment { id, .. }
        | TaskCmd::Idea { id, .. }
        | TaskCmd::Unstick { id }
        | TaskCmd::Route { id, .. } => id,
        _ => return unsupported(command.verb()),
    };
    let Some(mut row) = feed.board.task(id).cloned() else {
        return Err(format!("Preview task {id} does not exist"));
    };
    match command {
        TaskCmd::Hold { reason, .. } => {
            row.group = Group::Held;
            row.held_reason = reason.clone();
            row.now = "held by owner · preview".into();
        }
        TaskCmd::Release { .. } | TaskCmd::Unstick { .. } => {
            row.group = Group::Queued;
            row.held_reason = None;
            row.now = "ready".into();
            row.now_code = "ready".into();
        }
        TaskCmd::Move { status, .. } => {
            row.now_code = status.to_lowercase();
            row.now = status.clone();
            row.group = match row.now_code.as_str() {
                "done" | "cancelled" => Group::Other,
                "running" => Group::Producing,
                "held" => Group::Held,
                _ => Group::Queued,
            };
            row.rail = content::rail(row.group);
        }
        TaskCmd::Edit { title, body, .. } => {
            if let Some(title) = title {
                row.title = title.clone();
            }
            if let Some(body) = body {
                row.goal = Some(body.clone());
            }
        }
        TaskCmd::Reply { text, .. } => return sessions::append(feed, id, text),
        TaskCmd::Comment { text, send, .. } => {
            if *send
                && let Some(session) = row
                    .agent
                    .as_ref()
                    .and_then(|agent| agent.session.as_deref())
                && feed
                    .detail(&DetailRef::Session { id: session.into() })
                    .and_then(|cache| cache.body())
                    .is_some_and(|body| matches!(body, DetailBody::Session(page) if page.replyable))
            {
                sessions::append(feed, session, text)?;
            }
            let target = DetailRef::Task { id: id.clone() };
            if let Some(DetailBody::Task(mut detail)) =
                feed.detail(&target).and_then(|cache| cache.body()).cloned()
            {
                detail.log_tail.push(format!("Preview owner: {text}"));
                put(feed, target, DetailBody::Task(detail));
            }
        }
        TaskCmd::Dispatch { profile, .. } | TaskCmd::Retry { profile, .. } => {
            row.group = Group::Producing;
            row.now = "working · preview dispatch".into();
            row.now_code = "running".into();
            row.rail = content::rail(row.group);
            row.agent = Some(Agent {
                session: Some(id.clone()),
                role: "worker".into(),
                profile: profile.clone(),
                runtime: Some("preview".into()),
                machine: Some("preview-workstation".into()),
            });
        }
        TaskCmd::Route {
            profile,
            model,
            effort,
            set_reviewer,
            clear_reviewer,
            ..
        } => {
            if *clear_reviewer {
                super::control::task_note(feed, id, "Reviewer route cleared");
            }
            if let Some(reviewer) = set_reviewer {
                super::control::task_note(feed, id, &format!("Reviewer route: {reviewer}"));
            }
            if let Some(agent) = &mut row.agent
                && let Some(profile) = profile
            {
                agent.profile = Some(profile.clone());
            }
            let target = DetailRef::Task { id: id.clone() };
            if let Some(DetailBody::Task(mut detail)) =
                feed.detail(&target).and_then(|cache| cache.body()).cloned()
            {
                if let Some(run) = &mut detail.run {
                    if let Some(model) = model {
                        run.model = Some(model.clone());
                    }
                    if let Some(effort) = effort {
                        run.effort = Some(effort.clone());
                    }
                }
                put(feed, target, DetailBody::Task(detail));
            }
        }
        TaskCmd::Idea {
            action, body, into, ..
        } => {
            match action.as_str() {
                "discard" => row.now_code = "cancelled".into(),
                "merge" => {
                    let into = into.as_deref().ok_or("Preview merge needs a destination")?;
                    if into == id {
                        return Err("A preview idea cannot merge into itself".into());
                    }
                    let mut destination = feed
                        .board
                        .task(into)
                        .cloned()
                        .ok_or("Preview merge destination does not exist")?;
                    let words = row.goal.as_deref().unwrap_or_default();
                    destination.goal = Some(format!(
                        "{}\n\n{words}",
                        destination.goal.as_deref().unwrap_or_default()
                    ));
                    destination.rev += 1;
                    content::save_task(feed, destination);
                    row.now_code = "cancelled".into();
                }
                _ => return unsupported(action),
            }
            let target = DetailRef::Filings { before: None };
            if let Some(DetailBody::Filings(mut page)) =
                feed.detail(&target).and_then(|cache| cache.body()).cloned()
            {
                for filed in &mut page.rows {
                    if filed.task == *id {
                        filed.decision = Some(FiledDecision {
                            action: action.clone(),
                            into: into.clone(),
                        });
                    }
                }
                put(feed, target, DetailBody::Filings(page));
            }
            row.group = Group::Other;
            row.now = row.now_code.clone();
            if let Some(body) = body {
                row.goal = Some(body.clone());
            }
        }
        _ => return unsupported(command.verb()),
    }
    row.rev += 1;
    content::save_task(feed, row);
    sessions::sync_worker(feed, id);
    Ok(())
}
pub(super) fn message(
    feed: &mut Feed,
    command: &cmd::message::MessageCommand,
) -> Result<(), String> {
    use corral_protocol::assistant::{EntryCard, EntryKind, MessageState};
    let entry_id = match command {
        cmd::message::MessageCommand::FileAsIdea { entry }
        | cmd::message::MessageCommand::Withdraw { entry } => entry,
        _ => return unsupported(command.verb()),
    };
    let Some((scope, mut entry)) = feed.threads.iter().find_map(|(scope, thread)| {
        thread
            .entries
            .get(entry_id.as_str())
            .map(|entry| (scope.clone(), entry.clone()))
    }) else {
        return Err("Preview message does not exist".into());
    };
    if matches!(command, cmd::message::MessageCommand::Withdraw { .. }) {
        return Err("Preview message already delivered; it was not withdrawn".into());
    }
    if matches!(entry.state, Some(MessageState::Saved { .. })) {
        return Ok(());
    }
    let words = entry
        .text
        .as_deref()
        .ok_or("Preview message has no owner text")?;
    if !matches!(entry.kind, EntryKind::Message) {
        return Err("Only owner messages can be filed as preview ideas".into());
    }
    let next = feed
        .board
        .tasks()
        .iter()
        .filter_map(|task| {
            task.id
                .strip_prefix("T-")
                .and_then(|number| number.parse::<u64>().ok())
        })
        .max()
        .unwrap_or(903)
        + 1;
    let task_id = format!("T-{next}");
    let mut row = content::task(feed, &task_id, words, None, Group::Queued, "idea");
    row.goal = Some(words.into());
    row.now_code = "idea".into();
    content::save_task(feed, row);
    entry.kind = EntryKind::Idea;
    entry.card = Some(EntryCard::Idea {
        task: task_id.clone(),
        title: words.into(),
        labels: vec!["preview".into()],
        criteria: vec![],
        duplicates: vec![],
        words: words.into(),
    });
    entry.state = Some(MessageState::Saved {
        task: task_id.clone(),
    });
    for thread in feed.threads.values_mut() {
        if thread.entries.contains_key(entry_id.as_str()) {
            thread
                .entries
                .insert(entry_id.clone().into(), entry.clone());
        }
    }
    let target = DetailRef::Filings { before: None };
    if let Some(DetailBody::Filings(mut page)) =
        feed.detail(&target).and_then(|cache| cache.body()).cloned()
    {
        page.total += 1;
        page.rows.insert(
            0,
            FiledIdea {
                id: entry_id.clone(),
                seq: page.total,
                task: task_id,
                session: if scope == "quick" {
                    "A-7".into()
                } else {
                    scope.to_string()
                },
                quick: scope == "quick" || scope == "A-7",
                device: Some("Preview desktop".into()),
                at: stamp(feed, feed.now),
                title: entry.text.clone().unwrap_or_default(),
                status: "idea".into(),
                decision: None,
            },
        );
        put(feed, target, DetailBody::Filings(page));
    }
    Ok(())
}
