use super::*;

pub(super) fn question(feed: &mut Feed, command: &Question) -> Result<(), String> {
    let (key, rev) = match command {
        Question::Answer { key, rev, .. }
        | Question::Followup { key, rev, .. }
        | Question::Cancel { key, rev, .. }
        | Question::Deadline { key, rev, .. }
        | Question::Escalate { key, rev, .. } => (key, *rev),
        _ => return unsupported(command.verb()),
    };
    let target = DetailRef::Question { key: key.clone() };
    let Some(DetailBody::Question(mut detail)) =
        feed.detail(&target).and_then(|cache| cache.body()).cloned()
    else {
        return Err(format!("Preview question {key} does not exist"));
    };
    if detail.rev != rev || detail.state != InboxState::Open {
        return Err("conflict: preview question changed or already answered".into());
    }
    let at = stamp(feed, feed.now);
    match command {
        Question::Answer { text, option, .. } => {
            if text.trim().is_empty() && option.is_none() {
                return Err("Preview answer is empty".into());
            }
            if let Some(option) = option
                && !detail.options.iter().enumerate().any(|(index, choice)| {
                    choice.label == *option || (index + 1).to_string() == *option
                })
            {
                return Err("Preview answer option does not exist".into());
            }
            detail.answer = Some(QuestionAnswer {
                by: "owner".into(),
                via: "preview:local-only".into(),
                device: Some("preview-device".into()),
                at: at.clone(),
                text: text.clone(),
                option: option.clone(),
            });
            detail.state = InboxState::Answered {
                by: "owner".into(),
                via: "preview:local-only".into(),
                at,
            };
        }
        Question::Followup { text, .. } => detail.thread.push(Followup {
            by: "owner".into(),
            via: "preview:local-only".into(),
            at,
            text: text.clone(),
        }),
        Question::Cancel { .. } => detail.state = InboxState::Cancelled,
        Question::Deadline {
            at, instruction, ..
        } => {
            detail.needed_by = Some(at.clone());
            detail.deadline = Some(QuestionDeadline {
                at: at.clone(),
                instruction: instruction.clone(),
            });
        }
        Question::Escalate { note, .. } => detail.history.push(QuestionHistory {
            by: "owner".into(),
            via: Some("preview".into()),
            at,
            text: note.clone().unwrap_or_else(|| "Escalated to owner".into()),
            source_ref: None,
        }),
        _ => return unsupported(command.verb()),
    }
    let settled = detail.state != InboxState::Open;
    if let Some(row) = feed.inbox.get_mut(key.as_str()) {
        row.state = detail.state.clone();
        row.rev += 1;
        row.answerable = !settled;
        row.needed_by = detail.needed_by.clone();
    }
    if settled {
        if let Some(task_id) = &detail.task
            && let Some(mut row) = feed.board.task(task_id).cloned()
        {
            row.attention.retain(|entry| entry != key);
            row.questions = row.questions.saturating_sub(1);
            if row.questions == 0 && row.group == Group::NeedsYou && row.attention.is_empty() {
                row.group = Group::Producing;
                row.now = "working · restore policy decided".into();
            }
            row.rev += 1;
            content::save_task(feed, row);
        }
        let targets: Vec<_> = feed
            .details
            .values()
            .filter_map(|cache| match cache.body() {
                Some(DetailBody::Session(page)) => Some((cache.target.clone(), page.clone())),
                _ => None,
            })
            .collect();
        for (target, mut page) in targets {
            for message in &mut page.messages {
                for item in &mut message.items {
                    if let SessionItem::QuestionCard {
                        key: card_key,
                        answer,
                        ..
                    } = item
                        && card_key == key
                    {
                        *answer = detail.answer.clone();
                    }
                }
            }
            put(feed, target, DetailBody::Session(page));
        }
        if let Some(session) = &detail.from.session
            && let Some(agent) = feed.agents.get_mut(session.as_str())
        {
            agent.status = AgentStatus::Working;
            agent.now = "restore policy decided · preview".into();
            agent.rev += 1;
            agent.controls = sessions::controls(agent.status);
        }
    }
    put(feed, target, DetailBody::Question(detail));
    Ok(())
}
pub(super) fn attention(feed: &mut Feed, command: &Attention) -> Result<(), String> {
    let id = match command {
        Attention::Park { id, .. } | Attention::Unpark { id, .. } => id,
        _ => return unsupported(command.verb()),
    };
    let mut row = feed
        .board
        .task(id)
        .cloned()
        .ok_or("Preview task not found")?;
    let target = DetailRef::Task { id: id.clone() };
    let Some(DetailBody::Task(mut detail)) =
        feed.detail(&target).and_then(|cache| cache.body()).cloned()
    else {
        return Err("Preview task detail not found".into());
    };
    match command {
        Attention::Park { reason, .. } => {
            row.group = Group::Parked;
            row.now = "parked · preview".into();
            detail.park_revision = Some(detail.park_revision.unwrap_or(0) + 1);
            detail.park = Some(TaskHold {
                reason: Some(reason.clone()),
                since: Some(stamp(feed, feed.now)),
                by: Some("owner".into()),
            });
        }
        Attention::Unpark {
            expected_revision, ..
        } => {
            if detail.park_revision != Some(*expected_revision) {
                return Err("conflict: preview park revision changed".into());
            }
            row.group = Group::Queued;
            row.now = "ready".into();
            detail.park = None;
            detail.park_revision = None;
        }
        _ => return unsupported(command.verb()),
    }
    put(feed, target, DetailBody::Task(detail));
    row.rev += 1;
    content::save_task(feed, row);
    Ok(())
}
