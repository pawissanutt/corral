//! What an unset field of Settings › Configuration means: the value it inherits and from where.
//! A project key inherits the draft's `[defaults]`, else the engine's default for a project
//! (`project_defaults` in the settings snapshot); a workspace key falls back to the engine's
//! built-in configuration (`built_in_defaults`).

use serde_json::Value as Json;

use super::document::{Path, Seg};
use super::form::{Live, Shown, bytes};
use crate::model::daemon::Feed;

/// A served value in owner words; none for null or empty text.
pub(super) fn json_words(value: &Json) -> Option<String> {
    match value {
        Json::Null => None,
        Json::Bool(true) => Some("on".into()),
        Json::Bool(false) => Some("off".into()),
        Json::String(text) if text.is_empty() => None,
        Json::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

fn words(shown: &Shown) -> String {
    match shown {
        Shown::Flag(true) => "on".into(),
        Shown::Flag(false) => "off".into(),
        Shown::Text(text) => text.clone(),
        Shown::Lines(lines) => lines.join(", "),
        Shown::Pairs(pairs) => pairs
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// The engine's built-in value of a workspace key.
fn built_in(feed: &Feed, table: &str, key: &str) -> Option<String> {
    let defaults = feed.settings.as_ref()?.built_in_defaults.as_ref()?;
    json_words(defaults.get(table)?.get(key)?)
}

/// The engine's value of a key for a project that configures nothing.
fn project_default(feed: &Feed, key: &str) -> Option<String> {
    let defaults = feed.settings.as_ref()?.project_defaults.as_ref()?;
    json_words(defaults.get(key)?)
}

/// The workspace's running limit a project's cap sits under: the draft's, else the built-in one.
fn workspace_limit(live: &Live, feed: &Feed) -> Option<String> {
    let path = Path::of(&["limits", "running"]);
    live.draft
        .get(&path)
        .map(words)
        .or_else(|| built_in(feed, "limits", "running"))
}

/// What an unset field means: the value it inherits and from where.
pub fn unset(path: &Path, live: &Live, feed: &Feed) -> String {
    let keys: Vec<&str> = path
        .0
        .iter()
        .filter_map(|seg| match seg {
            Seg::Key(key) => Some(key.as_str()),
            Seg::Index(_) => None,
        })
        .collect();
    match keys.as_slice() {
        ["defaults", "base"] => "unset · each repository's default branch".into(),
        ["defaults", "profile"] => match built_in(feed, "defaults", "profile") {
            Some(profile) => format!("unset · corral's default, {profile}"),
            None => {
                "unset · no workspace profile; routing, the project or dispatch names one".into()
            }
        },
        [table @ ("defaults" | "limits" | "artifacts"), key] => match built_in(feed, table, key) {
            Some(value) if *key == "max_bytes" => {
                format!("unset · corral's default, {}", bytes(&value))
            }
            Some(value) => format!("unset · corral's default, {value}"),
            None => "unset · corral's default".into(),
        },
        ["projects", project, key] => project_unset(live, feed, project, key),
        ["extensions", rest @ ..] => super::extensions::unset(rest).unwrap_or("unset").into(),
        ["profiles", _, "model" | "effort"] => "unset · the harness's default".into(),
        ["profiles", _, "args"] => "No extra arguments.".into(),
        ["harnesses", _, "model_flag" | "effort_flag"] => {
            "unset · profiles of this harness cannot set it".into()
        }
        ["routing", "profile"] => "unset · a rule must name a profile".into(),
        ["routing", _] => "unset · any".into(),
        _ => "unset".into(),
    }
}

fn project_unset(live: &Live, feed: &Feed, project: &str, key: &str) -> String {
    let drafted = |key: &str| {
        live.draft
            .get(&Path::of(&["defaults", key]))
            .map(|shown| format!("inherits {} from Workspace", words(shown)))
    };
    match key {
        "base" => drafted("base").unwrap_or_else(|| {
            let found = project_default(feed, "base").or_else(|| {
                feed.projects
                    .get(project)
                    .and_then(|row| row.effective.as_ref()?.base.clone())
                    .filter(|_| {
                        live.saved
                            .get(&Path::entry("projects", project, "base"))
                            .is_none()
                            && live.saved.get(&Path::of(&["defaults", "base"])).is_none()
                    })
            });
            match found {
                Some(base) => format!("inherits the repository's default branch, {base}"),
                None => "inherits the repository's default branch".into(),
            }
        }),
        "profile" | "max_rounds" | "branch_prefix" | "auto_dispatch" | "auto_land" => drafted(key)
            .unwrap_or_else(|| match project_default(feed, key) {
                Some(value) => format!("inherits corral's default, {value}"),
                None => "inherits corral's default".into(),
            }),
        "running" => match workspace_limit(live, feed) {
            Some(limit) => format!("unset · only the workspace limit of {limit} applies"),
            None => "unset · only the workspace limit applies".into(),
        },
        "reviewer" => "unset · no review pass after done".into(),
        "gate" => "No gate lines: the gate has nothing to run.".into(),
        "env" => "No variables.".into(),
        "path" => "unset · corrald refuses a project without a path".into(),
        _ => "unset".into(),
    }
}
