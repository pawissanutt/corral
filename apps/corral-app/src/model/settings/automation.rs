//! Automation entries in the shared configuration draft.
use corral_protocol::app::AUTOMATION_V1;
use corral_types::ConfigProblem;
use serde_json::Value as Json;
use toml_edit::{DocumentMut, Item, Table, Value};

use super::document::{self, Path};
use super::form::{Field, Kind};
use crate::model::daemon::Feed;

pub const UNAVAILABLE: &str =
    "Automation editing needs a connected daemon that supports automation.v1.";
pub const PREVIEW: &str = "Preview: changes only the local sample config. No host execution.";
pub const AUTHORITY: &str = "These agents have your normal PM authority. Firings are not queued: busy, rate-limited and missed firings are skipped.";
pub const AGENT_KEYS: [&str; 2] = ["profile", "instructions"];
pub const RULE_KEYS: [&str; 4] = ["agent", "event", "cron", "enabled"];
pub const WEEKDAYS: &str = "0 9 * * 1-5";

pub fn available(feed: &Feed, preview: bool) -> bool {
    preview || (feed.ready() && feed.capabilities.contains(AUTOMATION_V1))
}

pub fn save_blocked(available: bool, saved: Option<&str>, draft: &str) -> bool {
    !available && saved.is_some_and(|saved| changed(saved, draft))
}

fn changed(saved: &str, draft: &str) -> bool {
    if saved == draft {
        return false;
    }
    let subtree = |doc| document::json(&doc).remove("automation");
    match (document::parse(saved), document::parse(draft)) {
        (Ok(saved), Ok(draft)) => subtree(saved) != subtree(draft),
        _ => saved.contains("automation") || draft.contains("automation"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    AddAgent(String),
    AddRule(String, String),
    Remove(&'static str, String),
    Trigger(String, bool),
}

pub fn path(section: &str, id: &str, key: &str) -> Path {
    Path::of(&["automation", section, id, key])
}

pub fn names(doc: &DocumentMut, section: &str) -> Vec<String> {
    document::table(doc.as_table(), &Path::of(&["automation", section]).0)
        .into_iter()
        .flat_map(|table| table.iter())
        .filter(|(_, item)| item.is_table_like())
        .map(|(id, _)| id.to_owned())
        .collect()
}

pub fn valid_id(id: &str) -> bool {
    let mut bytes = id.bytes();
    id.len() <= 40
        && bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
}

pub fn apply(doc: &mut DocumentMut, change: &Change) {
    let (section, id) = match change {
        Change::AddAgent(id) => ("agents", id),
        Change::AddRule(id, _) | Change::Trigger(id, _) => ("rules", id),
        Change::Remove(section, id) => (*section, id),
    };
    let adding = matches!(change, Change::AddAgent(_) | Change::AddRule(..));
    let Some(parent) = document::table_mut(
        doc.as_table_mut(),
        &Path::of(&["automation", section]).0,
        adding,
    ) else {
        return;
    };
    match change {
        Change::Remove(..) => {
            parent.remove(id);
        }
        Change::Trigger(_, cron) if parent.contains_key(id) => {
            let (removed, key, value) = if *cron {
                ("event", "cron", WEEKDAYS)
            } else {
                ("cron", "event", "blocked")
            };
            document::remove(doc, &path(section, id, removed));
            if document::get(doc, &path(section, id, key)).is_none() {
                document::set(doc, &path(section, id, key), Value::from(value));
            }
        }
        _ if adding && valid_id(id) && !parent.contains_key(id) => {
            let mut entry = Table::new();
            match change {
                Change::AddAgent(_) => {
                    entry.insert("profile", toml_edit::value("pi"));
                    entry.insert("instructions", toml_edit::value(""));
                }
                Change::AddRule(_, agent) => {
                    entry.insert("agent", toml_edit::value(agent.as_str()));
                    entry.insert("event", toml_edit::value("blocked"));
                    entry.insert("enabled", toml_edit::value(false));
                }
                _ => return,
            }
            parent.insert(id, Item::Table(entry));
        }
        _ => {}
    }
}

pub fn profiles(doc: &DocumentMut) -> Vec<String> {
    let mut profiles = document::names(doc, "profiles");
    if !profiles.iter().any(|name| name == "pi") {
        profiles.insert(0, "pi".into());
    }
    profiles.retain(|name| {
        document::get(doc, &Path::entry("profiles", name, "harness"))
            .map_or(name == "pi", |item| item.as_str() == Some("pi"))
    });
    profiles
}

pub fn field(
    section: &str,
    id: &str,
    key: &str,
    label: &'static str,
    kind: Kind,
    note: Option<&'static str>,
) -> Field {
    Field {
        path: path(section, id, key),
        label,
        kind,
        note,
        offers: Vec::new(),
        caption: "",
    }
}

pub fn summary(doc: &DocumentMut, id: &str) -> String {
    let get = |key| document::get(doc, &path("rules", id, key));
    let enabled = get("enabled").and_then(Item::as_bool).unwrap_or(false);
    let agent = get("agent").and_then(Item::as_str).unwrap_or("(no agent)");
    let when = match get("cron").and_then(Item::as_str) {
        Some(WEEKDAYS) => "At 09:00 UTC on weekdays".into(),
        Some(cron) => format!("When {cron} matches the UTC clock"),
        None if get("event").and_then(Item::as_str) == Some("blocked") => {
            "When a task becomes blocked".into()
        }
        None => "No trigger configured".into(),
    };
    format!(
        "{when}, ask {agent}.{}",
        if enabled { "" } else { " Paused." }
    )
}

/// The live daemon validates the full schema, including cron ranges.
pub fn preview_problems(root: &Json) -> Vec<ConfigProblem> {
    let mut problems = Vec::new();
    let mut problem = |key: String, message: &str| {
        problems.push(ConfigProblem {
            key: Some(key),
            message: message.into(),
            line: None,
            column: None,
        })
    };
    let agents = root["automation"]["agents"].as_object();
    for (id, agent) in agents.into_iter().flatten() {
        let profile = agent["profile"].as_str().unwrap_or_default();
        if root["profiles"]
            .get(profile)
            .and_then(|p| p["harness"].as_str())
            != Some("pi")
        {
            problem(
                format!("automation.agents.{id}.profile"),
                "Choose a named Pi profile",
            );
        }
        if !agent["instructions"]
            .as_str()
            .is_some_and(|s| (1..=8192).contains(&s.trim().len()))
        {
            problem(
                format!("automation.agents.{id}.instructions"),
                "Enter 1–8192 bytes of instructions",
            );
        }
    }
    for (id, rule) in root["automation"]["rules"]
        .as_object()
        .into_iter()
        .flatten()
    {
        if !rule["agent"]
            .as_str()
            .is_some_and(|name| agents.is_some_and(|a| a.contains_key(name)))
        {
            problem(
                format!("automation.rules.{id}.agent"),
                "Choose an existing agent or remove this rule",
            );
        }
        if !matches!(
            (rule["event"].as_str(), rule["cron"].as_str()),
            (Some("blocked"), None)
        ) && !(rule["event"].is_null()
            && rule["cron"]
                .as_str()
                .is_some_and(|cron| cron.split_whitespace().count() == 5))
        {
            problem(
                format!("automation.rules.{id}.cron"),
                "Choose Task blocked or enter a five-field UTC cron expression",
            );
        }
    }
    problems
}
