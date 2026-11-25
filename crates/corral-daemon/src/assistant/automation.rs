//! Rule-owned chats. The scheduler's chat ID is a hint, never authority.
use super::*;
use corral_engine::runner::pi::{Disposition, SubmitReply};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct Binding {
    pub(super) rule: String,
    instructions: String,
    launch: Value,
    #[serde(default)]
    stopped: bool,
}

/// An intentionally dropped firing (not queued and not a delivery refusal).
#[derive(Debug)]
pub(crate) struct Skip(&'static str);

impl std::fmt::Display for Skip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for Skip {}

#[derive(Debug)]
pub(crate) struct Unknown;

impl std::fmt::Display for Unknown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("no definitive acknowledgement for the submitted automation input")
    }
}

impl std::error::Error for Unknown {}

fn saved<'a>(config: &'a Config, rule: &str) -> Result<(&'a str, &'a str)> {
    let rule = config
        .automation
        .rules
        .get(rule)
        .filter(|rule| rule.enabled)
        .ok_or_else(|| refuse("automation rule is missing or disabled", vec![]))?;
    let agent = config
        .automation
        .agents
        .get(&rule.agent)
        .ok_or_else(|| refuse("automation agent no longer exists", vec![]))?;
    Ok((&agent.profile, &agent.instructions))
}

fn launch_settings(config: &Config, name: &str) -> Result<Value> {
    serde_json::to_value(profile(config, name)?).context("serialize automation launch settings")
}

fn matches(
    chat: &Chat,
    binding: &Binding,
    profile: &str,
    instructions: &str,
    launch: &Value,
) -> bool {
    chat.meta.profile == profile
        && binding.instructions == instructions
        && binding.launch == *launch
}

pub(super) fn launch_policy<'a>(
    chat: &'a Chat,
    config: &Config,
) -> Result<Option<(&'a str, &'a str)>> {
    let Some(binding) = &chat.meta.automation else {
        return Ok(None);
    };
    let (profile, instructions) = saved(config, &binding.rule)?;
    if !matches(
        chat,
        binding,
        profile,
        instructions,
        &launch_settings(config, profile)?,
    ) {
        return Err(refuse(
            "automation policy changed; the rule needs a fresh chat",
            vec![],
        ));
    }
    Ok(Some((&binding.rule, &binding.instructions)))
}

pub(super) fn require_resumed(chat: &Chat) -> Result<()> {
    if chat
        .meta
        .automation
        .as_ref()
        .is_some_and(|binding| binding.stopped)
    {
        return Err(refuse(
            "automation chat is stopped; resume it first",
            vec![],
        ));
    }
    Ok(())
}

pub(super) fn set_stopped(shared: &Shared, chat: &mut Chat, stopped: bool) -> Result<()> {
    if let Some(binding) = chat
        .meta
        .automation
        .as_mut()
        .filter(|binding| binding.stopped != stopped)
    {
        binding.stopped = stopped;
        chat.save()?;
        shared.publish();
    }
    Ok(())
}

fn belongs(chat: &Chat, rule: &str) -> bool {
    !chat.meta.quick
        && chat.meta.task.is_none()
        && chat
            .meta
            .automation
            .as_ref()
            .is_some_and(|binding| binding.rule == rule)
}

fn latest(shared: &Shared, rule: &str) -> Result<Option<Chat>> {
    Ok(records(shared)?
        .into_iter()
        .rev()
        .find(|chat| belongs(chat, rule)))
}

/// Discover the most recent allocation even if startup or submission failed.
pub(crate) fn allocated_chat(shared: &Shared, rule: &str) -> Result<Option<String>> {
    Ok(latest(shared, rule)?.map(|chat| chat.id()))
}

/// Check the actual socket, including a launch that has not saved a host record yet.
fn ready(shared: &Shared, chat: &Chat) -> Result<Option<SessionRef>> {
    let host = sessions::host_record(&chat.dir)?;
    let session = pi::chat_session(
        &chat.dir,
        host.as_ref().map_or(0, |host| host.pid),
        socket_root(shared).as_deref(),
    )?;
    let client = Client::for_session(&session)?;
    let timeout = Duration::from_millis(250);
    let hello = match client.hello_with_timeout(timeout) {
        Ok(hello) => hello,
        Err(error)
            if host.is_none()
                && error.downcast_ref::<std::io::Error>().is_some_and(|error| {
                    matches!(
                        error.kind(),
                        ErrorKind::NotFound | ErrorKind::ConnectionRefused
                    )
                }) =>
        {
            return Ok(None);
        }
        Err(_) => return Err(Skip("automation chat unavailable").into()),
    };
    ensure!(
        hello.session_id == chat.id(),
        "Pi socket answers for a different chat session"
    );
    let state: HostState = client
        .request_with_timeout("get_state", json!({}), timeout)
        .map_err(|_| Skip("automation chat unavailable"))?;
    if state.paused {
        return Err(Skip("automation chat paused").into());
    }
    if !state.owner_questions.is_empty() {
        return Err(Skip("automation chat waiting for owner").into());
    }
    if state.turn.open {
        return Err(Skip("automation chat busy").into());
    }
    Ok(Some(pi::chat_session(
        &chat.dir,
        hello.pid,
        socket_root(shared).as_deref(),
    )?))
}

fn host_skip(reason: Option<&str>) -> Option<Skip> {
    match reason {
        Some("paused") => Some(Skip("automation chat paused")),
        Some("waiting") => Some(Skip("automation chat waiting for owner")),
        Some("busy") => Some(Skip("automation chat busy")),
        _ => None,
    }
}

fn unavailable(error: anyhow::Error) -> anyhow::Error {
    if builds::disconnected(&error) {
        Skip("automation chat unavailable").into()
    } else {
        error
    }
}

fn retire(shared: &Shared, chat: &mut Chat, active: Option<SessionRef>) -> Result<()> {
    if let Some(session) = &active {
        // Serialize the final idle check with the host's own admission machinery.
        #[derive(Deserialize)]
        struct Retirement {
            retired: bool,
            reason: Option<String>,
        }
        let result: Retirement = Client::for_session(session)?
            .request("retire_automation", json!({}))
            .map_err(unavailable)?;
        if !result.retired {
            if let Some(skip) = host_skip(result.reason.as_deref()) {
                return Err(skip.into());
            }
            return Err(refuse(
                result
                    .reason
                    .unwrap_or_else(|| "Pi refused automation retirement".into()),
                vec![],
            ));
        }
    }
    chat.meta.ended = true;
    chat.save()?;
    shared.publish();
    if let Some(session) = active {
        PiRunner::new(shared.chat_placement().as_ref()).stop(&session)?;
    }
    Ok(())
}

pub(crate) fn deliver(
    shared: &Shared,
    rule: &str,
    profile: &str,
    chat: Option<&str>,
    input_id: &str,
    text: &str,
) -> Result<String> {
    let _lock = shared
        .chat
        .mutation
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    // Automation is itself explicit opt-in; it need not also enable the quick thread.
    let config = load_config(shared)?;
    let (saved_profile, instructions) = saved(&config, rule)?;
    if saved_profile != profile {
        return Err(refuse(
            "automation profile changed since firing was scheduled",
            vec![],
        ));
    }
    let launch = launch_settings(&config, saved_profile)?;
    if let Some(id) = chat {
        ensure!(number(id).is_some(), "invalid automation chat id");
        if lookup(shared, id)?.is_some_and(|chat| !belongs(&chat, rule)) {
            return Err(refuse(
                "chat does not belong to this automation rule",
                vec![],
            ));
        }
    }
    let mut current = None;
    if let Some(mut chat) = latest(shared, rule)?.filter(|chat| !chat.meta.ended) {
        let binding = chat
            .meta
            .automation
            .as_ref()
            .context("missing automation binding")?;
        if binding.stopped {
            return Err(Skip("automation chat stopped by owner").into());
        }
        let active = ready(shared, &chat)?;
        if matches(&chat, binding, profile, instructions, &launch) {
            current = Some((chat, active));
        } else {
            retire(shared, &mut chat, active)?;
        }
    }
    let (chat, active) = match current {
        Some(current) => current,
        None => (
            create(
                shared,
                false,
                None,
                profile.into(),
                Some(Binding {
                    rule: rule.into(),
                    instructions: instructions.into(),
                    launch,
                    stopped: false,
                }),
            )?,
            None,
        ),
    };
    let session = match active {
        Some(session) => session,
        None => chat.start(shared, &config)?,
    };
    let reply: SubmitReply = Client::for_session(&session)?.request("submit_automation", json!({
        "origin": corral_types::Origin::Auto, "busy_policy": "steer", "input_id": input_id, "text": text,
    })).map_err(|error| if error.is::<pi::PiRefusal>() {error} else {error.context(Unknown)})?;
    if reply.disposition == Disposition::Rejected {
        if let Some(skip) = host_skip(reply.reason.as_deref()) {
            return Err(skip.into());
        }
        return Err(refuse(
            reply
                .reason
                .unwrap_or_else(|| "Pi rejected automation input".into()),
            vec![],
        ));
    }
    if reply.input.as_deref() != Some(input_id) || reply.disposition != Disposition::Started {
        return Err(Unknown.into());
    }
    shared.publish();
    Ok(chat.id())
}

#[cfg(test)]
#[path = "automation_tests.rs"]
mod tests;
