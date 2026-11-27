//! The chat rows corrald serves (`quick` and `A-n`), the profiles a chat can answer with, and
//! where an owner's profile change or End stands. Unknown facts stay unknown.

use corral_protocol::rows::{AssistantKind, AssistantLocation, AssistantRow, AssistantState};

use super::{QUICK, State};
use crate::model::daemon::Feed;

/// The harness the chat host runs as (`[harnesses.pi]`); its profiles are the assistant's models.
const ASSISTANT_HARNESS: &str = "pi";

/// A configured profile the assistant can answer with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    pub name: String,
    /// The profile's model, or its harness default when it names none.
    pub model: String,
    pub effort: Option<String>,
}

/// The configured profiles of the chat host's harness, in `corral.toml`'s order.
pub fn models(feed: &Feed) -> Vec<ModelChoice> {
    let Some(profiles) = feed
        .settings
        .as_ref()
        .and_then(|settings| settings.config.get("profiles"))
        .and_then(serde_json::Value::as_object)
    else {
        return Vec::new();
    };
    let text = |profile: &serde_json::Value, key: &str| {
        profile
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    profiles
        .iter()
        .filter(|(_, profile)| text(profile, "harness").as_deref() == Some(ASSISTANT_HARNESS))
        .map(|(name, profile)| ModelChoice {
            name: name.clone(),
            model: text(profile, "model").unwrap_or_else(|| "harness default".into()),
            effort: text(profile, "effort"),
        })
        .collect()
}

/// The profile a new chat starts on: the one corrald reports it resolved
/// (`SettingsSnapshot.ask_chat_profile`, which also covers the saved Ask/Chat startup default),
/// as long as the configuration still lists it as a chat profile. A daemon that reports none
/// is read as before: the default profile when it runs on the chat host's harness, else the
/// first such profile.
pub fn configured_profile(feed: &Feed) -> Option<String> {
    let models = models(feed);
    let settings = feed.settings.as_ref()?;
    if let Some(name) = settings.ask_chat_profile.as_deref()
        && models.iter().any(|model| model.name == name)
    {
        return Some(name.to_owned());
    }
    let default = settings
        .config
        .get("defaults")
        .and_then(|defaults| defaults.get("profile"))
        .and_then(serde_json::Value::as_str);
    default
        .and_then(|name| models.iter().find(|model| model.name == name))
        .or_else(|| models.first())
        .map(|model| model.name.clone())
}

fn configured_text(feed: &Feed, key: &str) -> Option<String> {
    let name = configured_profile(feed)?;
    let value = feed.settings.as_ref()?.config["profiles"][&name][key].as_str()?;
    Some(value.to_owned())
}

/// The model a new chat answers with. None when that profile names no model (its harness
/// decides) or the settings are not served. Only a chat that does not exist yet reads this; a
/// running or resumed chat reports its own.
pub fn configured_model(feed: &Feed) -> Option<String> {
    configured_text(feed, "model")
}

/// The effort a new chat starts with, from the same profile. None when it names none.
pub fn configured_effort(feed: &Feed) -> Option<String> {
    configured_text(feed, "effort")
}

/// The chat row of a scope: the current quick chat's for `quick`, else the session's.
pub fn row<'a>(feed: &'a Feed, scope: &str) -> Option<&'a AssistantRow> {
    if scope == QUICK {
        feed.assistants.get(quick_id(feed)?.as_str())
    } else {
        feed.assistants.get(scope)
    }
}

fn session_number(id: &str) -> Option<u64> {
    id.strip_prefix("A-")?.parse().ok()
}

/// The chat `quick` resolves to, as corrald resolves it: the newest quick chat that has not
/// ended. None once every quick chat ended: the next send to `quick` starts a new one, and the
/// ended ones stay readable as their own `A-n`.
pub fn quick_id(feed: &Feed) -> Option<String> {
    feed.assistants
        .values()
        .filter(|row| row.kind == AssistantKind::Quick && row.state != AssistantState::Ended)
        .max_by_key(|row| session_number(&row.id).unwrap_or(0))
        .map(|row| row.id.clone())
}

/// Whether the app picks this chat's profile: `quick`, and a chat that is not ended and whose
/// host runs in the daemon or does not run at all (the daemon keeps the pick and applies it when
/// the host next starts). A Pi pane uses its own picker; an unknown location and an ended chat
/// take no pick.
pub fn pickable(feed: &Feed, scope: &str) -> bool {
    scope == QUICK
        || (!ended(feed, scope)
            && row(feed, scope).is_some_and(|row| {
                matches!(
                    row.live_in,
                    AssistantLocation::Daemon | AssistantLocation::None
                )
            }))
}

/// Where a requested profile change stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickState {
    /// Sent; the row does not report it yet.
    Pending,
    /// corrald refused it.
    Failed(String),
}

/// The profile change still awaiting the chat's row, or its refusal.
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

/// The profile that answers the chat's next message: a pending pick, else the row's.
pub fn profile(state: &State, feed: &Feed, scope: &str) -> Option<String> {
    if let Some((profile, PickState::Pending)) = pick_state(state, feed, scope) {
        return Some(profile);
    }
    row(feed, scope).and_then(|row| row.profile.clone())
}

/// Whether corrald reports the chat ended; an ended chat takes no more messages.
pub fn ended(feed: &Feed, scope: &str) -> bool {
    scope != QUICK && row(feed, scope).is_some_and(|row| row.state == AssistantState::Ended)
}

/// Where an End sent from this device stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndState {
    /// Sent; the row does not report it ended yet.
    Pending,
    /// corrald refused it; the chat runs on.
    Failed(String),
}

/// The End of a chat still awaiting its row, or its refusal.
pub fn end_state(state: &State, feed: &Feed, scope: &str) -> Option<EndState> {
    let session = end_target(feed, scope)?;
    let end = state.live.ends.get(&session)?;
    if ended(feed, &session) {
        return None;
    }
    Some(match &end.failed {
        Some(why) => EndState::Failed(why.clone()),
        None => EndState::Pending,
    })
}

/// The `A-n` an End of `scope` closes: the current quick chat for `quick`, else the scope.
pub fn end_target(feed: &Feed, scope: &str) -> Option<String> {
    if scope == QUICK {
        quick_id(feed)
    } else {
        Some(scope.to_owned())
    }
}
