use super::*;

pub(super) fn session(feed: &mut Feed, command: &Session) -> Result<(), String> {
    let id = match command {
        Session::Cancel { id, .. }
        | Session::Pause { id, .. }
        | Session::Resume { id, .. }
        | Session::Stop { id, .. }
        | Session::Relaunch { id, .. }
        | Session::Send { id, .. }
        | Session::Keys { id, .. } => id.as_str(),
        Session::PmOpen { .. } => "PM",
        _ => return unsupported(command.verb()),
    };
    let row = feed
        .agents
        .get(id)
        .ok_or_else(|| format!("Preview session {id} does not exist"))?;
    let control = match command {
        Session::Cancel {
            generation, turn, ..
        } => {
            if generation.is_some_and(|generation| Some(generation) != row.generation)
                || turn.is_some_and(|turn| Some(turn) != row.turn)
            {
                return Err("conflict: preview session generation or turn changed".into());
            }
            &row.controls.cancel
        }
        Session::Pause { generation, .. } | Session::Stop { generation, .. } => {
            if generation.is_some_and(|generation| Some(generation) != row.generation) {
                return Err("conflict: preview session generation changed".into());
            }
            if matches!(command, Session::Pause { .. }) {
                &row.controls.pause
            } else {
                &row.controls.stop
            }
        }
        Session::Resume { .. } => &row.controls.resume,
        Session::Relaunch { .. } | Session::PmOpen { .. } => &row.controls.relaunch,
        Session::Send { .. } => &row.controls.send,
        Session::Keys { .. } => &row.controls.keys,
        _ => return unsupported(command.verb()),
    };
    if !control.available {
        return Err(format!(
            "Preview session refused: {}",
            control.refusal.as_deref().unwrap_or("unavailable")
        ));
    }
    if let Session::Send { text, .. } = command {
        sessions::append(feed, id, text)?;
    }
    let status = match command {
        Session::Pause { .. } => AgentStatus::Paused,
        Session::Stop { .. } => AgentStatus::Ended,
        Session::Send { .. } => AgentStatus::Working,
        _ => AgentStatus::Idle,
    };
    let task = if let Some(row) = feed.agents.get_mut(id) {
        row.status = status;
        row.controls = sessions::controls(status);
        row.turn = None;
        row.rev += 1;
        row.now = format!("Preview · {status:?}");
        if matches!(command, Session::Relaunch { .. } | Session::PmOpen { .. }) {
            row.generation = Some(row.generation.unwrap_or(0) + 1);
        }
        row.task.clone()
    } else {
        None
    };
    let target = DetailRef::Session { id: id.into() };
    if let Some(DetailBody::Session(mut page)) =
        feed.detail(&target).and_then(|cache| cache.body()).cloned()
    {
        page.replyable = status != AgentStatus::Ended;
        page.reply_refusal = (!page.replyable).then(|| "Preview session ended".into());
        put(feed, target, DetailBody::Session(page));
    }
    super::control::session_task(
        feed,
        id,
        task.as_deref(),
        status,
        matches!(command, Session::Relaunch { .. } | Session::PmOpen { .. }),
    );
    Ok(())
}
