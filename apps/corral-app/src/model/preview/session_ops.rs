//! `session.send` and `build.start` against the sample, answered as corrald answers them. A send
//! goes to the session's pi host under the sender's command id, which the transcript then serves
//! the message under (`<session>/input/<id>`); a message to a stopped chat starts its host, one
//! to a stopped build is refused until it is resumed. A start makes a new build session on a
//! configured project, as the daemon launches a headless host.

use corral_protocol::assistant::QUICK_THREAD;
use corral_protocol::build::BuildStart;
use corral_protocol::rows::{AssistantKind, AssistantState};
use corral_protocol::session_host::{InputDisposition, SessionSend, SessionSent};
use corral_types::Origin;
use serde_json::Value;

use super::{effects, pi, pi_rows, sessions};
use crate::model::daemon::Feed;

/// Sends owner input to a sample session; the reply is the acknowledgement.
pub(super) fn send(feed: &mut Feed, send: &SessionSend) -> Result<Value, String> {
    if send.text.trim().is_empty() {
        return Err("Preview message is empty".into());
    }
    if !send.attachments.is_empty() {
        return Err("Preview sends do not upload or deliver attachments; remove them first".into());
    }
    if send.to.is_some() {
        return Err("Preview child agents take no messages".into());
    }
    let id = if send.id == QUICK_THREAD {
        effects::chat_for(feed, QUICK_THREAD)?
    } else {
        send.id.clone()
    };
    match feed
        .assistants
        .get(id.as_str())
        .map(|row| (row.kind, row.state))
    {
        Some((AssistantKind::Build, AssistantState::Cold)) => {
            return Err("this build's host is not running; resume it first".into());
        }
        Some((_, AssistantState::Ended)) => {
            return Err("session has ended; start a new one".into());
        }
        // As corrald starts a stopped chat host before it submits the message.
        Some((AssistantKind::Quick | AssistantKind::Session, _)) => pi::revive(feed, &id),
        _ => {}
    }
    if !feed.hosts.contains_key(id.as_str()) {
        return Err("session has no Pi socket; it takes no messages".into());
    }
    let entry = pi::input_entry(&id, &send.cmd_id);
    let disposition = match pi::deliver(feed, &id, &send.text, Origin::Owner, Some(entry.clone())) {
        "delivered" => InputDisposition::Started,
        "steered" | "queued" => InputDisposition::Queued,
        _ => return Err("session has ended; relaunch it before sending".into()),
    };
    let turn = feed.hosts.get(id.as_str()).map(|host| host.turn);
    serde_json::to_value(SessionSent {
        session: id,
        entry,
        disposition,
        turn,
    })
    .map_err(|error| error.to_string())
}

/// `walk`, `auth-refactor`: the name as the host slugs it.
fn slug(name: &str) -> String {
    let mut slug = String::new();
    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').chars().take(40).collect()
}

/// The next build id in the host's scheme (`B-` and base 36).
fn next_build(feed: &Feed) -> String {
    let mut n = feed.assistants.len() as u64 + 1;
    loop {
        let mut digits = Vec::new();
        let mut rest = n;
        while rest > 0 {
            let digit = (rest % 36) as u32;
            digits.push(char::from_digit(digit, 36).unwrap_or('0'));
            rest /= 36;
        }
        let id = format!("B-p{}", digits.iter().rev().collect::<String>());
        if !feed.assistants.contains_key(id.as_str()) {
            return id;
        }
        n += 1;
    }
}

/// Starts a sample build session; the reply is its row.
pub(super) fn start(feed: &mut Feed, start: &BuildStart) -> Result<Value, String> {
    let project = start
        .project
        .clone()
        .ok_or("Preview builds need a project; choose one")?;
    let path = feed
        .projects
        .get(project.as_str())
        .ok_or_else(|| format!("Preview project {project} does not exist"))?
        .effective
        .as_ref()
        .map(|settings| settings.path.display().to_string());
    let id = next_build(feed);
    let name = start
        .name
        .as_deref()
        .map(slug)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("build-{}", id.trim_start_matches("B-")));
    let taken = feed.assistants.values().any(|row| {
        row.build.as_ref().is_some_and(|build| {
            build.name == name && build.project.as_deref() == Some(project.as_str())
        })
    });
    if taken {
        return Err(format!(
            "A build named {name} already exists in {project}; choose another name"
        ));
    }
    sessions::build(feed, &id, &name, Some(&project), false);
    let row = feed
        .assistants
        .get_mut(id.as_str())
        .ok_or("Preview build was not recorded")?;
    if start.model.is_some() {
        row.model = start.model.clone();
    }
    if start.effort.is_some() {
        row.effort = start.effort.clone();
    }
    if let Some(build) = row.build.as_mut().filter(|_| start.here) {
        build.here = true;
        build.branch = None;
        if let Some(path) = path {
            build.worktree = path;
        }
    }
    pi_rows::settle(feed, &id);
    let row = feed
        .assistants
        .get(id.as_str())
        .ok_or("Preview build was not recorded")?;
    serde_json::to_value(row).map_err(|error| error.to_string())
}
