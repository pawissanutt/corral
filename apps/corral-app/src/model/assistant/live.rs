//! Daemon-mode content for Ask, the thread head and facts, and Agents' assistant blocks, built
//! from `daemon::Feed` into the same records the fixture fills. Unknown facts stay unknown.

use std::collections::BTreeMap;

use corral_protocol::assistant::{ContextRef, EntryCard, MESSAGE_TEXT_MAX, PromptChoice};
use corral_protocol::board::CostBasis;
use corral_protocol::detail::{DetailBody, DetailRef};
use corral_protocol::rows::{
    AssistantKind, AssistantLocation, AssistantRow, AssistantState, ModelChoice, SettingsSnapshot,
    WaitReason,
};

use super::surfaces::drawer_buttons;
use super::views::{LIFETIME_NOTE, button, context_chip, model_footer, targets};
use super::{
    Action, Agents, COMPOSER_KEYS, Chip, Composer, Drawer, Fact, FiledLine, ModelList, ModelOption,
    QUICK, QuickThread, SessionRow, Staged, State, ThreadHeader, tray,
};
use crate::model::daemon::{Feed, Link, PmLevel};
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::id::Id;
use crate::model::live::Live;
use crate::model::nav::{AskTarget, Form};
use crate::model::text::span;

/// The served settings snapshot, when settings are served.
pub fn settings(feed: &Feed) -> Option<&SettingsSnapshot> {
    feed.settings.values().next()
}

/// The configured `[assistant.quick] models`, in cycling order.
pub fn models(feed: &Feed) -> &[ModelChoice] {
    settings(feed).map_or(&[], |settings| settings.assistant_models.as_slice())
}

/// The assistant row of a thread scope: the quick row for "quick", else the session's.
pub fn row<'a>(feed: &'a Feed, scope: &str) -> Option<&'a AssistantRow> {
    if scope == QUICK {
        current_quick(feed)
    } else {
        feed.assistants.get(scope)
    }
}

/// The quick thread's current generation's row, when corrald has sent it (see `quick_id`).
pub fn current_quick(feed: &Feed) -> Option<&AssistantRow> {
    let id = quick_id(feed)?;
    feed.assistants
        .get(id.as_str())
        .filter(|row| row.kind == AssistantKind::Quick)
}

/// The session an entry or streamed turn belongs to: the `A-n` before its first '/'.
pub fn entry_session(id: &str) -> Option<&str> {
    let session = id.split('/').next()?;
    session_number(session).map(|_| session)
}

fn session_number(id: &str) -> Option<u64> {
    id.strip_prefix("A-")?.parse().ok()
}

/// The quick thread's current session id. Rolling or a restart leaves earlier quick
/// generations (an ended A-3 beside a live A-6) in the rows; the rows' current one is the
/// newest that has not ended, else the newest. The session rows and the thread's entries
/// arrive separately, so whichever names the newer generation wins: an A-8 answer streaming
/// before A-8's row is never labelled A-7, and a new A-8 row is not hidden by A-7's entries.
pub fn quick_id(feed: &Feed) -> Option<String> {
    let quick = || {
        feed.assistants
            .values()
            .filter(|row| row.kind == AssistantKind::Quick)
    };
    let number = |row: &&AssistantRow| session_number(&row.id).unwrap_or(0);
    let from_rows = quick()
        .filter(|row| row.state != AssistantState::Ended)
        .max_by_key(number)
        .or_else(|| quick().max_by_key(number))
        .map(|row| row.id.as_str());
    let from_thread = feed.threads.get(QUICK).and_then(|thread| {
        thread
            .entries
            .keys()
            .map(|id| id.as_str())
            .chain(thread.live.as_ref().map(|live| live.entry.as_str()))
            .filter_map(entry_session)
            .max_by_key(|session| session_number(session).unwrap_or(0))
    });
    [from_rows, from_thread]
        .into_iter()
        .flatten()
        .max_by_key(|session| session_number(session).unwrap_or(0))
        .map(str::to_owned)
}

/// "Quick thread · A-n" with the current generation, or "Quick thread" before one is known.
pub fn quick_title(feed: &Feed) -> String {
    match quick_id(feed) {
        Some(id) => format!("Quick thread · {id}"),
        None => "Quick thread".into(),
    }
}

/// Whether the app picks this thread's model: the quick thread and daemon-hosted sessions do;
/// a Pi pane uses its own picker.
pub fn pickable(feed: &Feed, scope: &str) -> bool {
    scope == QUICK || row(feed, scope).is_some_and(|row| row.live_in == AssistantLocation::Daemon)
}

/// Where a requested profile change stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickState {
    /// Sent; the row does not report it yet.
    Pending,
    /// corrald refused it.
    Failed(String),
}

/// The profile change still awaiting the thread's row, or its refusal.
pub fn pick_state(state: &State, feed: &Feed, scope: &str) -> Option<(String, PickState)> {
    let pick = state.live.picks.get(scope)?;
    if row(feed, scope).and_then(|row| row.profile.as_deref()) == Some(pick.profile.as_str()) {
        return None;
    }
    let outcome = match &pick.failed {
        Some(why) => PickState::Failed(why.clone()),
        None => PickState::Pending,
    };
    Some((pick.profile.clone(), outcome))
}

/// The profile that answers the thread's next message: a pending pick, the row's, or the
/// configured default.
pub fn profile(state: &State, feed: &Feed, scope: &str) -> Option<String> {
    if let Some((profile, PickState::Pending)) = pick_state(state, feed, scope) {
        return Some(profile);
    }
    row(feed, scope)
        .and_then(|row| row.profile.clone())
        .or_else(|| settings(feed).and_then(|settings| settings.assistant_profile.clone()))
}

/// Whether corrald reports the session ended; an ended session takes no more messages.
pub fn ended(feed: &Feed, scope: &str) -> bool {
    scope != QUICK && row(feed, scope).is_some_and(|row| row.state == AssistantState::Ended)
}

/// Where an End sent from this device stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndState {
    /// Sent; the row does not report it ended yet.
    Pending,
    /// corrald refused it; the session runs on.
    Failed(String),
}

/// The End of a named session still awaiting its row, or its refusal.
pub fn end_state(state: &State, feed: &Feed, scope: &str) -> Option<EndState> {
    let end = state.live.ends.get(scope)?;
    if ended(feed, scope) {
        return None;
    }
    Some(match &end.failed {
        Some(why) => EndState::Failed(why.clone()),
        None => EndState::Pending,
    })
}

/// The task's owner revision from its served detail; task verbs carry it as their precondition.
pub fn task_revision(feed: &Feed, task: &str) -> Option<String> {
    let cache = feed.detail(&DetailRef::Task {
        id: task.to_owned(),
    })?;
    if cache.stale {
        return None;
    }
    match cache.body()? {
        DetailBody::Task(detail) => Some(detail.owner_revision.clone()),
        _ => None,
    }
}

/// The filed Idea's current title: the board's when it is on the board, else the filing card's.
pub fn idea_title(feed: &Feed, task: &str) -> Option<String> {
    if let Some(row) = feed.board.task(task) {
        return Some(row.title.clone());
    }
    feed.threads
        .values()
        .flat_map(|thread| thread.entries.values())
        .find_map(|entry| match &entry.card {
            Some(EntryCard::Idea {
                task: id, title, ..
            }) if id == task => Some(title.clone()),
            _ => None,
        })
}

/// A prompt choice as its button and receipt name it.
pub const fn choice_label(choice: PromptChoice) -> &'static str {
    match choice {
        PromptChoice::Once => "Allow once",
        PromptChoice::Hour => "Allow for 1 hour",
        PromptChoice::Session => "Allow for this session",
        PromptChoice::Deny => "Deny",
        PromptChoice::Confirm => "Confirm on this device",
        PromptChoice::Decline => "Decline",
        PromptChoice::Other => "Answer",
    }
}

/// A served timestamp as the local clock, with the date when it is not today. Text that is not
/// a timestamp is shown as served.
pub fn clock(at: &str, today: &str) -> String {
    let Ok(stamp) = at.parse::<jiff::Timestamp>() else {
        return at.to_owned();
    };
    let local = stamp.to_zoned(jiff::tz::TimeZone::system());
    let time = format!("{:02}:{:02}", local.hour(), local.minute());
    if local.date().to_string() == today {
        time
    } else {
        format!("{} {} {time}", local.strftime("%b"), local.day())
    }
}

/// A wait reason's short words ("host down", "provider quota") from its code.
pub fn wait_label(wait: &WaitReason) -> String {
    match wait.code.as_str() {
        "" => wait.text.clone(),
        code => code.replace('_', " "),
    }
}

/// A wait reason's full explanation, with when it may lift.
pub fn wait_explanation(wait: &WaitReason, today: &str) -> String {
    match &wait.retry_at {
        Some(at) => format!("{} Retrying at {}.", wait.text, clock(at, today)),
        None => wait.text.clone(),
    }
}

/// Dollars with the basis when not provider-reported.
pub fn cost(usd: Option<f64>, basis: Option<CostBasis>) -> String {
    match (usd, basis) {
        (None, _) => "cost unknown".into(),
        (Some(usd), Some(CostBasis::Reported) | None) => format!("${usd:.2}"),
        (Some(usd), Some(_)) => format!("${usd:.2} (estimated)"),
    }
}

/// Whether the loaded thread has a prompt or question still waiting for the owner.
pub fn needs_you(feed: &Feed, scope: &str) -> bool {
    feed.threads.get(scope).is_some_and(|thread| {
        thread.entries.values().any(|entry| match &entry.card {
            Some(EntryCard::Prompt { choices, .. }) => !choices.is_empty() && entry.state.is_none(),
            Some(EntryCard::Question { answer, .. }) => answer.is_none() && entry.state.is_none(),
            _ => false,
        })
    })
}

/// The thread's state chip. With corrald unreachable the row is only what was last received:
/// the chip says so, in no state colour, rather than claiming the host is live now.
pub fn live_chip(feed: &Feed, scope: &str) -> Chip {
    let chip = served_chip(feed, scope);
    if feed.link == Link::Up {
        return chip;
    }
    match &feed.synced {
        Some(synced) if row(feed, scope).is_some() => Chip::new(
            format!("last known {} · {synced}", chip.label),
            Hue::Neutral,
        ),
        _ => Chip::new("state unknown", Hue::Neutral),
    }
}

/// The thread's state chip from its row, its stream and its open prompts, as served.
fn served_chip(feed: &Feed, scope: &str) -> Chip {
    let streaming = feed
        .threads
        .get(scope)
        .and_then(|thread| thread.live.as_ref())
        .is_some_and(|live| !live.done);
    let Some(row) = row(feed, scope) else {
        let loading = feed
            .topics
            .get("assistant")
            .is_none_or(|topic| topic.loading);
        return Chip::new(if loading { "loading" } else { "unknown" }, Hue::Neutral);
    };
    // An ended session is over: a failure its row still records, a stale stream or an open
    // prompt is history, not a current wait.
    if row.state == AssistantState::Ended {
        return Chip::new("ended", Hue::Neutral);
    }
    if streaming {
        return Chip::new("answering", Hue::Corral);
    }
    if let Some(wait) = &row.wait {
        return Chip::new(
            format!("waiting for assistant · {}", wait_label(wait)),
            Hue::Held,
        );
    }
    if needs_you(feed, scope) {
        return Chip::new("waiting on you", Hue::You);
    }
    let (label, hue) = match row.state {
        AssistantState::Cold => ("not started · starts on your message", Hue::Neutral),
        AssistantState::Starting => ("starting", Hue::Corral),
        AssistantState::Ready => ("live", Hue::Worker),
        AssistantState::Working => ("working", Hue::Corral),
        AssistantState::Waiting => ("waiting for assistant", Hue::Held),
        AssistantState::Stopping => ("stopping", Hue::Neutral),
        AssistantState::Ended => ("ended", Hue::Neutral),
        AssistantState::Failed => ("host failed", Hue::Fail),
        AssistantState::Other => ("unknown", Hue::Neutral),
    };
    Chip::new(label, hue)
}

/// Where the session's host runs.
fn location(row: Option<&AssistantRow>) -> &'static str {
    match row.map(|row| row.live_in) {
        Some(AssistantLocation::Daemon) => "runs: daemon",
        Some(AssistantLocation::Pane) => "runs in a Pi pane",
        Some(AssistantLocation::None) => "no host",
        Some(AssistantLocation::Other) | None => "host unknown",
    }
}

fn pm_chip(feed: &Feed) -> Chip {
    let level = feed
        .health
        .as_ref()
        .map_or(PmLevel::Unknown, |health| health.pm);
    let (label, hue) = match level {
        PmLevel::Live => ("live", Hue::Ok),
        PmLevel::Relaunching => ("relaunching", Hue::Held),
        PmLevel::Absent => ("absent", Hue::Fail),
        PmLevel::Unknown => ("unknown", Hue::Neutral),
    };
    if feed.link == Link::Up {
        return Chip::new(label, hue);
    }
    match &feed.synced {
        Some(synced) => Chip::new(format!("{label} · as of {synced}"), Hue::Held),
        None => Chip::new("unknown", Hue::Neutral),
    }
}

/// The context chip's words: its kind and id, with the task's title when the board has it.
fn context_label(feed: &Feed, context: &ContextRef) -> String {
    match feed.board.task(&context.id) {
        Some(task) if context.kind == "task" => format!("{} {}", context.id, task.title),
        _ => context.id.clone(),
    }
}

/// The model list for a thread: the configured profiles, the current one checked.
fn model_list(feed: &Feed, scope: &str, current: Option<&str>) -> ModelList {
    ModelList {
        label: "Model",
        options: models(feed)
            .iter()
            .map(|model| ModelOption {
                name: model.name.clone(),
                model: match &model.effort {
                    Some(effort) => format!("{} · {effort}", model.model),
                    None => model.model.clone(),
                },
                note: model.note.clone().unwrap_or_default(),
                selected: Some(model.name.as_str()) == current,
                action: Action::PickProfile {
                    thread: scope.to_owned(),
                    profile: model.name.clone(),
                },
            })
            .collect(),
        footer: model_footer(),
        illustrative: None,
    }
}

/// What the live composer needs from the app besides the feed.
#[derive(Debug, Clone, Copy)]
pub struct Draft<'a> {
    /// Thread scope this composer sends to.
    pub scope: &'a str,
    /// Ask's target (the quick thread only).
    pub target: AskTarget,
    /// The app-owned draft.
    pub text: &'a str,
    /// Ask's bounded context.
    pub context: Option<&'a ContextRef>,
    /// This device may message the assistant (or has no session to say otherwise yet).
    pub can_send: bool,
    /// What is staged for the next message.
    pub staged: &'a [Staged],
}

/// Ask's composer, or a named session's own, from the served thread and settings.
pub fn live_composer(state: &State, feed: &Feed, draft: Draft<'_>, env: Env) -> Composer {
    let Draft {
        scope,
        target,
        text,
        context,
        can_send,
        staged,
    } = draft;
    let named = scope != QUICK;
    let target = if named { AskTarget::Assistant } else { target };
    let row = row(feed, scope);
    let to_assistant = target == AskTarget::Assistant;
    let current = profile(state, feed, scope);
    let pick = pick_state(state, feed, scope);
    let pickable = to_assistant && pickable(feed, scope) && !models(feed).is_empty();
    let too_long = text.trim().len() > MESSAGE_TEXT_MAX;
    let tray = tray(feed, staged, to_assistant, can_send, text.trim().is_empty());
    let ended = ended(feed, scope);
    let end = end_state(state, feed, scope);
    let closing = ended || end == Some(EndState::Pending);
    let status = if ended {
        Some(vec![
            span("ended").hue(Hue::Neutral),
            span(
                " This session is closed for good and takes no more messages. Its transcript stays readable.",
            ),
        ])
    } else if let Some(EndState::Pending) = &end {
        Some(vec![
            span("ending").hue(Hue::Held),
            span(" corrald is stopping the host and closing this session."),
        ])
    } else if !can_send {
        Some(vec![
            span("may not message the assistant").hue(Hue::Fail),
            span(" This device was not granted assistant access; nothing is sent from here."),
        ])
    } else if too_long {
        Some(vec![
            span("too long").hue(Hue::Fail),
            span(format!(
                " {} of {MESSAGE_TEXT_MAX} bytes; shorten it to send.",
                text.trim().len()
            )),
        ])
    } else if let Some(line) = tray.status.clone() {
        Some(line)
    } else if feed.link != Link::Up {
        Some(vec![
            span("offline").hue(Hue::Held),
            span(" Messages queue on this device and send once when corrald is reachable."),
        ])
    } else if let Some(EndState::Failed(why)) = &end {
        Some(vec![
            span("not ended").hue(Hue::Fail),
            span(format!(" {why}. The session runs on.")),
        ])
    } else if let Some((name, PickState::Failed(why))) = &pick {
        Some(vec![
            span("model not changed").hue(Hue::Fail),
            span(format!(" {name}: {why}")),
        ])
    } else if !to_assistant {
        (feed.health.as_ref().map(|health| health.pm) == Some(PmLevel::Absent)).then(|| {
            vec![
                span("PM absent").hue(Hue::Held),
                span(" Messages wait in the PM inbox until it next takes input."),
            ]
        })
    } else {
        match row {
            // The quick thread's last generation ended: Send starts the next one, it does not
            // resume the ended session.
            Some(row) if row.state == AssistantState::Ended => Some(vec![
                span("ended").hue(Hue::Neutral),
                span(format!(
                    " {} ended. Your message starts a new quick thread; this one stays readable.",
                    row.id
                )),
            ]),
            // An ended generation's recorded failure is history, not a current wait.
            Some(AssistantRow {
                wait: Some(wait),
                state,
                ..
            }) if *state != AssistantState::Ended => Some(vec![
                span(format!("waiting for assistant · {}", wait_label(wait))).hue(Hue::Held),
                span(" Send still stores your message; it waits in the thread."),
            ]),
            Some(row) if row.state == AssistantState::Failed => Some(vec![
                span("host failed").hue(Hue::Fail),
                span(" Send still stores your message; it waits until the host runs again."),
            ]),
            Some(row) if row.state == AssistantState::Starting => Some(vec![
                span("starting").hue(Hue::Corral),
                span(" The host is starting; your message is stored meanwhile."),
            ]),
            _ => None,
        }
    };
    let default = settings(feed).and_then(|settings| settings.assistant_profile.as_deref());
    let next_message = match (&pick, current.as_deref()) {
        _ if !pickable => None,
        (Some((name, PickState::Pending)), _) => Some(vec![
            span(name.clone()),
            span(
                " Switching… it answers your next message once corrald confirms. The conversation is kept.",
            ),
        ]),
        (_, Some(name)) if Some(name) != default => {
            let model = models(feed)
                .iter()
                .find(|model| model.name == name)
                .map_or_else(|| name.to_owned(), |model| model.model.clone());
            Some(vec![
                span(name.to_owned()),
                span(format!(
                    " Your next message is answered by {model}. The conversation is kept."
                )),
            ])
        }
        _ => None,
    };
    let placeholder = if named {
        format!("Message {scope}…")
    } else if let Some(context) = context {
        format!("Ask about {} {}…", context.kind, context.id)
    } else if target == AskTarget::Pm {
        "Message the PM…".into()
    } else {
        "Ask a question, drop an idea, or give an instruction…".into()
    };
    let mut send = button("Send", Action::Send);
    send.primary = true;
    send.enabled = can_send && !closing && !too_long && !text.trim().is_empty() && !tray.holds_send;
    Composer {
        targets: if named {
            Vec::new()
        } else {
            targets(target, pm_chip(feed))
        },
        model_pill: pickable.then(|| button("Model for the next message", Action::ToggleModelMenu)),
        model_name: pickable.then(|| current.clone().unwrap_or_else(|| "default profile".into())),
        model_menu: (pickable && state.model_menu)
            .then(|| model_list(feed, scope, current.as_deref())),
        menu_inline: pickable && env.form == Form::Phone,
        profile_note: if named {
            match row.map(|row| row.live_in) {
                Some(AssistantLocation::Daemon) if pickable => {
                    Some("model for this session".into())
                }
                // Only a host that really runs in a Pi pane picks its model there.
                Some(AssistantLocation::Pane) => row
                    .and_then(|row| row.profile.clone())
                    .map(|profile| format!("{profile} · set in its pane")),
                // A pane-less host, live without a picker, ended or not running: the profile its
                // row reports, with nothing to change it from here; none when it reports none.
                _ => row
                    .and_then(|row| row.profile.clone())
                    .map(|profile| format!("profile {profile}")),
            }
        } else if !to_assistant {
            Some("the PM runs its own profile".into())
        } else if !pickable {
            current.map(|profile| format!("profile {profile}"))
        } else {
            None
        },
        next_message,
        chip: context
            .map(|context| context_chip(context.kind.clone(), context_label(feed, context))),
        text: text.into(),
        placeholder,
        status,
        attachments: tray.buttons,
        staged: tray.chips,
        attach_note: tray.note,
        send,
        key_hints: (!named && env.form != Form::Phone).then_some(COMPOSER_KEYS),
    }
}

/// Ask's head with the daemon: the quick thread's id and state.
pub fn live_drawer(feed: &Feed, env: Env) -> Drawer {
    let phone = env.form == Form::Phone;
    let title = if phone {
        "Ask".into()
    } else {
        quick_title(feed)
    };
    Drawer {
        label: "Ask",
        title,
        state: (!phone).then(|| live_chip(feed, QUICK)),
        subtitle: (!phone).then(|| "shared by your devices · runs in corrald".into()),
        buttons: drawer_buttons(phone, "Quick thread ›"),
        keyboard_requested: phone,
        close_label: "Close (Esc)",
    }
}

/// The Ideas filed from one thread's loaded entries, newest first, for its page's facts. One
/// row per filed task: an Idea card and the owner's "saved as written" state naming the same
/// task are one filing (the Idea card's title wins); different tasks stay separate rows.
pub fn live_filed(feed: &Feed, scope: &str) -> Vec<FiledLine> {
    let Some(thread) = feed.threads.get(scope) else {
        return Vec::new();
    };
    // Where the words came from, not who filed them.
    let source = match row(feed, scope) {
        Some(row) if scope == QUICK => format!("From {} · quick thread", row.id),
        Some(row) => format!("From {} · {}", row.id, location(Some(row))),
        None => format!("From {scope}"),
    };
    let mut filed: BTreeMap<String, (String, String, bool)> = BTreeMap::new();
    for entry in thread.entries.values() {
        let (task, title, idea) = match (&entry.card, &entry.state) {
            (Some(EntryCard::Idea { task, title, .. }), _) => (task, title.clone(), true),
            (_, Some(corral_protocol::assistant::MessageState::Saved { task })) => (
                task,
                entry
                    .text
                    .as_deref()
                    .unwrap_or("")
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_owned(),
                false,
            ),
            _ => continue,
        };
        let keep = filed
            .get(task)
            .is_none_or(|(_, _, had_idea)| idea && !had_idea);
        if keep {
            filed.insert(task.clone(), (entry.at.clone(), title, idea));
        }
    }
    let mut rows: Vec<(String, FiledLine)> = filed
        .into_iter()
        .map(|(task, (at, title, _))| {
            (
                at.clone(),
                FiledLine {
                    action: Action::OpenLive(Live::Task(Id::from(task.as_str()))),
                    id: task,
                    title,
                    at: clock(&at, &feed.date),
                    source: source.clone(),
                },
            )
        })
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    rows.into_iter().map(|(_, row)| row).collect()
}

/// The thread page's head and facts; an unknown thread shows only what is known.
pub fn live_thread_header(state: &State, feed: &Feed, scope: &str) -> ThreadHeader {
    let quick = scope == QUICK;
    let row = row(feed, scope);
    let quick_session = quick.then(|| quick_id(feed)).flatten();
    let id = quick_session.as_deref().unwrap_or(scope);
    let current = profile(state, feed, scope);
    let mut profile_value = vec![span(current.unwrap_or_else(|| "unknown".into()))];
    if pickable(feed, scope) && !models(feed).is_empty() {
        profile_value.push(span(" · switch in the composer").dim());
    }
    let chip = live_chip(feed, scope);
    let mut facts = vec![
        Fact {
            label: "Id",
            value: vec![span(id).mono()],
        },
        Fact {
            label: "State",
            value: vec![span(chip.label.clone())],
        },
        Fact {
            label: "Where",
            value: vec![span(location(row))],
        },
        Fact {
            label: "Profile",
            value: profile_value,
        },
        Fact {
            label: "Cost",
            value: vec![span(cost(
                row.and_then(|row| row.cost_usd),
                row.and_then(|row| row.cost_basis),
            ))],
        },
    ];
    if let Some(focus) = row.and_then(|row| row.focus.as_deref()) {
        facts.push(Fact {
            label: "Focus",
            value: vec![span(focus).mono().live(Live::Task(Id::from(focus)))],
        });
    }
    if let Some(pending) = row.map(|row| row.pending).filter(|pending| *pending > 0) {
        facts.push(Fact {
            label: "Waiting",
            value: vec![span(format!("{pending} message(s)"))],
        });
    }
    ThreadHeader {
        title: match row {
            _ if quick => quick_title(feed),
            Some(row) => format!("{} · {}", row.id, row.title),
            None => scope.to_owned(),
        },
        phone_title: if quick {
            quick_title(feed)
        } else {
            id.to_owned()
        },
        back_label: "Agents",
        state: Some(chip),
        subtitle: if quick {
            "runs: daemon · one per workspace · every device".into()
        } else {
            format!("assistant session · {}", location(row))
        },
        facts,
        lifetime_note: LIFETIME_NOTE,
        filed: live_filed(feed, scope),
        end: (!quick && row.is_some() && !ended(feed, scope)).then(|| {
            let mut end = button("End session", Action::EndSession(Some(scope.to_owned())));
            end.danger = true;
            end.enabled = end_state(state, feed, scope) != Some(EndState::Pending);
            end
        }),
    }
}

/// The newest owner message and answer of a loaded thread, for the quick-thread tile.
fn latest(feed: &Feed, scope: &str) -> (Option<(String, String)>, Option<String>) {
    let Some(thread) = feed.threads.get(scope) else {
        return (None, None);
    };
    let mut entries: Vec<_> = thread.entries.values().collect();
    entries.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
    use corral_protocol::assistant::EntryKind;
    let owner = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry.kind, EntryKind::Message | EntryKind::PmMessage))
        .and_then(|entry| Some((clock(&entry.at, &feed.date), entry.text.clone()?)));
    let answer = entries
        .iter()
        .rev()
        .find(|entry| entry.kind == EntryKind::Answer)
        .and_then(|entry| entry.text.as_deref())
        .and_then(|text| text.lines().find(|line| !line.trim().is_empty()))
        .map(str::to_owned);
    (owner, answer)
}

/// Agents' assistant blocks from the served rows.
pub fn live_agents(state: &State, feed: &Feed, can_send: bool) -> Agents {
    let quick = row(feed, QUICK);
    let chip = live_chip(feed, QUICK);
    let (owner, answer) = latest(feed, QUICK);
    // Earlier quick generations (an ended A-3 after the quick thread moved on to A-6) stay
    // openable as their own A-n threads; only the current generation is the pinned tile.
    let current = quick.map(|row| row.id.as_str());
    let mut sessions: Vec<&AssistantRow> = feed
        .assistants
        .values()
        .filter(|row| row.kind != AssistantKind::Quick || Some(row.id.as_str()) != current)
        .collect();
    sessions.sort_by_key(|row| {
        std::cmp::Reverse(
            row.id
                .strip_prefix("A-")
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(0),
        )
    });
    let rows: Vec<SessionRow> = sessions
        .iter()
        .map(|row| {
            let state = live_chip(feed, &row.id);
            let cost = cost(row.cost_usd, row.cost_basis);
            let focus = row.focus.as_deref().unwrap_or("−");
            SessionRow {
                id: row.id.clone(),
                title: row.title.clone(),
                desktop_facts: format!(
                    "{} · focus {focus} · {cost} · {} waiting",
                    location(Some(row)),
                    row.pending
                ),
                phone_facts: format!("{} · {}", state.label, location(Some(row))),
                state,
                cost,
                action: Action::OpenLive(Live::Thread(Id::from(row.id.as_str()))),
            }
        })
        .collect();
    let topic = feed.topics.get("assistant");
    let creating = state.live.new_session.is_some();
    let sessions_note = if creating {
        Some("Starting a new session in corrald…".into())
    } else if let Some(why) = &state.live.new_session_failed {
        Some(format!("New session not started: {why}"))
    } else if let Some(error) = topic.and_then(|topic| topic.error.as_ref()) {
        Some(format!("Sessions unavailable: {}", error.message))
    } else if topic.is_none_or(|topic| topic.loading) && feed.assistants.is_empty() {
        Some("Loading sessions…".into())
    } else if topic.is_some_and(|topic| topic.stale) {
        Some("Shown as last received; reconnecting.".into())
    } else if rows.is_empty() {
        Some("No named sessions. + New session starts one in corrald.".into())
    } else {
        None
    };
    let mut new_session = button("+ New session", Action::NewSession);
    new_session.enabled = can_send && !creating;
    let filed = super::global_filed(state, feed);
    Agents {
        quick_thread: QuickThread {
            id: quick_id(feed).unwrap_or_else(|| QUICK.to_owned()),
            title: quick_title(feed),
            row_title: "Quick thread",
            last_line: answer.unwrap_or_else(|| chip.label.clone()),
            state: chip,
            pinned: "assistant · pinned",
            last_question: owner.as_ref().map_or_else(String::new, |(_, text)| {
                let clipped: String = text.chars().take(96).collect();
                if text.chars().count() > 96 {
                    format!("{clipped}…")
                } else {
                    clipped
                }
            }),
            owner_at: owner.map_or_else(String::new, |(at, _)| format!("you · {at}")),
            model: profile(state, feed, QUICK).unwrap_or_else(|| "profile unknown".into()),
            location: location(quick).into(),
            scope: "shared by your devices",
            lifetime: quick.map_or_else(
                || "host unknown".into(),
                |row| cost(row.cost_usd, row.cost_basis),
            ),
            action: Action::OpenQuickThread,
        },
        sessions_title: "Assistant sessions",
        sessions_count: format!("{} · daemon and pane", rows.len()),
        sessions: rows,
        new_session,
        sessions_note,
        sessions_illustrative: None,
        filed_title: "Filed from chat",
        filed_total: filed.total,
        filed_note: filed.note,
        filed_pager: filed.pager,
        filed: filed.rows,
        filed_illustrative: None,
    }
}

/// Spans for a record id that opens it.
pub fn record_link(context: &ContextRef) -> Option<(String, Live)> {
    let id = Id::from(context.id.as_str());
    let live = match context.kind.as_str() {
        "task" => Live::Task(id),
        "plan" => Live::Plan(id),
        "question" => Live::Question(id),
        "session" => Live::Session(id),
        "thread" => Live::Thread(id),
        "review" => Live::Review(id),
        _ => return None,
    };
    Some((format!("{} {}", context.kind, context.id), live))
}
