//! Settings › Configuration: `corral.toml` as corrald parsed it (`SettingsSnapshot.config`),
//! read-only, in the order an owner tunes it: the automation switches and slots first, then
//! defaults, projects, profiles, routing, harnesses, artifacts and the runner. A key corrald did
//! not send reads "not reported", never a default this app guesses.

use serde_json::{Map, Value};

use super::live::Fact;

/// One named entry of a list group: a project, profile, routing rule or harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub facts: Vec<Fact>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Facts(Vec<Fact>),
    /// Entries in key order; `empty` says what none configured means.
    Items {
        items: Vec<Item>,
        empty: &'static str,
    },
}

/// One titled part of the configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub title: &'static str,
    pub slug: &'static str,
    pub note: Option<&'static str>,
    pub body: Body,
}

/// Where the configuration can be changed while this page only reads it.
pub const READ_ONLY: &str = "This is corral.toml as corrald last read it. Change it in .corral/corral.toml on the daemon host; corrald checks it before using it.";

const NOT_REPORTED: &str = "not reported";

pub fn groups(config: &Map<String, Value>) -> Vec<Group> {
    let defaults = |key: &str| at(config, &["defaults", key]);
    vec![
        Group {
            title: "Automation",
            slug: "automation",
            note: Some(
                "Workspace switches. A project's own setting overrides them; Running and Blocked tasks hold slots, Review does not.",
            ),
            body: Body::Facts(vec![
                ("Auto dispatch", shown(defaults("auto_dispatch"), "off")),
                ("Auto land", shown(defaults("auto_land"), "off")),
                (
                    "Running slots",
                    shown(at(config, &["limits", "running"]), "unlimited"),
                ),
                ("Review rounds", shown(defaults("max_rounds"), "not set")),
            ]),
        },
        Group {
            title: "Defaults",
            slug: "defaults",
            note: None,
            body: Body::Facts(vec![
                ("Profile", shown(defaults("profile"), "not set")),
                (
                    "Base branch",
                    shown(defaults("base"), "the repository's default branch"),
                ),
                ("Branch prefix", shown(defaults("branch_prefix"), "none")),
            ]),
        },
        Group {
            title: "Projects",
            slug: "projects",
            note: None,
            body: Body::Items {
                items: entries(config, "projects", |name, project| Item {
                    name: name.to_owned(),
                    facts: project_facts(config, project),
                }),
                empty: "No projects are configured, so corral manages no repository.",
            },
        },
        Group {
            title: "Profiles",
            slug: "profiles",
            note: None,
            body: Body::Items {
                items: entries(config, "profiles", |name, profile| Item {
                    name: name.to_owned(),
                    facts: vec![
                        ("Harness", shown(profile.get("harness"), "not set")),
                        (
                            "Model",
                            shown(profile.get("model"), "the harness's default"),
                        ),
                        (
                            "Effort",
                            shown(profile.get("effort"), "the harness's default"),
                        ),
                        ("Arguments", words(profile.get("args"))),
                    ],
                }),
                empty: "No profiles are configured.",
            },
        },
        Group {
            title: "Routing",
            slug: "routing",
            note: Some(
                "The first rule that matches a task picks its profile; a task's own profile wins.",
            ),
            body: Body::Items {
                items: routing(config),
                empty: "No routing rules, so tasks use their own or the default profile.",
            },
        },
        Group {
            title: "Harnesses",
            slug: "harnesses",
            note: None,
            body: Body::Items {
                items: entries(config, "harnesses", |name, harness| Item {
                    name: name.to_owned(),
                    facts: vec![
                        ("Command", shown(harness.get("command"), "not set")),
                        ("Model flag", shown(harness.get("model_flag"), "none")),
                        ("Effort flag", shown(harness.get("effort_flag"), "none")),
                    ],
                }),
                empty: "No harnesses are configured beyond corral's built-in ones.",
            },
        },
        Group {
            title: "Artifacts",
            slug: "artifacts",
            note: None,
            body: Body::Facts(vec![
                ("Globs", lines(at(config, &["artifacts", "globs"]), "none")),
                (
                    "Largest file",
                    bytes(at(config, &["artifacts", "max_bytes"])),
                ),
            ]),
        },
        Group {
            title: "Runner",
            slug: "runner",
            note: None,
            body: Body::Facts(vec![
                ("Mode", shown(at(config, &["runner", "mode"]), "not set")),
                (
                    "herdr session",
                    shown(at(config, &["runner", "herdr_session"]), "the default"),
                ),
                (
                    "Quiet after",
                    seconds(at(config, &["sync", "quiet_after_secs"])),
                ),
                ("Sync every", seconds(at(config, &["sync", "watch_secs"]))),
            ]),
        },
    ]
}

/// The phone root list's line for Configuration.
pub fn subtitle(config: &Map<String, Value>) -> String {
    let projects = config
        .get("projects")
        .and_then(Value::as_object)
        .map_or(0, Map::len);
    let dispatch = shown(at(config, &["defaults", "auto_dispatch"]), "off");
    let land = shown(at(config, &["defaults", "auto_land"]), "off");
    let noun = if projects == 1 { "project" } else { "projects" };
    format!("{projects} {noun} · auto dispatch {dispatch} · auto land {land}")
}

fn project_facts(config: &Map<String, Value>, project: &Map<String, Value>) -> Vec<Fact> {
    let get = |key: &str| project.get(key);
    // A project key left out (or null) takes the workspace default.
    let inherit = |key: &str, unset: &str| match get(key).filter(|value| !value.is_null()) {
        Some(value) => shown(Some(value), unset),
        None => format!(
            "workspace · {}",
            shown(at(config, &["defaults", key]), unset)
        ),
    };
    vec![
        ("Path", shown(get("path"), "not set")),
        ("Gate", lines(get("gate"), "none")),
        ("Reviewer", shown(get("reviewer"), "not set")),
        ("Profile", inherit("profile", "not set")),
        (
            "Base branch",
            inherit("base", "the repository's default branch"),
        ),
        ("Auto dispatch", inherit("auto_dispatch", "off")),
        ("Auto land", inherit("auto_land", "off")),
        (
            "Running slots",
            shown(get("running"), "the workspace limit"),
        ),
        ("Review rounds", inherit("max_rounds", "not set")),
        ("Branch prefix", inherit("branch_prefix", "none")),
        // Names only: values can be credentials, and this page is read on shared screens.
        ("Environment", keys(get("env"))),
    ]
}

fn routing(config: &Map<String, Value>) -> Vec<Item> {
    let Some(rules) = config.get("routing").and_then(Value::as_array) else {
        return Vec::new();
    };
    rules
        .iter()
        .enumerate()
        .filter_map(|(at, rule)| {
            let rule = rule.as_object()?;
            Some(Item {
                name: format!(
                    "Rule {} → {}",
                    at + 1,
                    shown(rule.get("profile"), "no profile")
                ),
                facts: vec![
                    ("Kinds", shown(rule.get("kind"), "any")),
                    ("Sizes", shown(rule.get("size"), "any")),
                    ("Title matches", shown(rule.get("title_regex"), "any")),
                    ("Project", shown(rule.get("project"), "any")),
                ],
            })
        })
        .collect()
}

/// The entries of the table at `key`, in key order.
fn entries(
    config: &Map<String, Value>,
    key: &str,
    item: impl Fn(&str, &Map<String, Value>) -> Item,
) -> Vec<Item> {
    config
        .get(key)
        .and_then(Value::as_object)
        .map(|table| {
            table
                .iter()
                .filter_map(|(name, value)| Some(item(name, value.as_object()?)))
                .collect()
        })
        .unwrap_or_default()
}

fn at<'a>(config: &'a Map<String, Value>, path: &[&str]) -> Option<&'a Value> {
    let (first, rest) = path.split_first()?;
    rest.iter()
        .try_fold(config.get(*first)?, |value, key| value.get(key))
}

/// A value in owner words; `unset` for null, an empty string or an empty list.
fn shown(value: Option<&Value>, unset: &str) -> String {
    let Some(value) = value else {
        return NOT_REPORTED.into();
    };
    match value {
        Value::Null => unset.into(),
        Value::Bool(true) => "on".into(),
        Value::Bool(false) => "off".into(),
        Value::Number(number) => number.to_string(),
        Value::String(text) if text.is_empty() => unset.into(),
        Value::String(text) => text.clone(),
        Value::Array(items) if items.is_empty() => unset.into(),
        Value::Array(items) => items
            .iter()
            .map(|item| shown(Some(item), unset))
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(_) => value.to_string(),
    }
}

/// A list one entry per line, as a gate's commands or the artifact globs read.
fn lines(value: Option<&Value>, unset: &str) -> String {
    match value {
        Some(Value::Array(items)) if !items.is_empty() => items
            .iter()
            .map(|item| shown(Some(item), unset))
            .collect::<Vec<_>>()
            .join("\n"),
        other => shown(other, unset),
    }
}

/// Command arguments as the harness receives them, space-separated.
fn words(value: Option<&Value>) -> String {
    match value {
        Some(Value::Array(items)) if !items.is_empty() => items
            .iter()
            .map(|item| shown(Some(item), ""))
            .collect::<Vec<_>>()
            .join(" "),
        other => shown(other, "none"),
    }
}

fn keys(value: Option<&Value>) -> String {
    match value {
        Some(Value::Object(table)) if !table.is_empty() => {
            table.keys().cloned().collect::<Vec<_>>().join(", ")
        }
        Some(Value::Object(_) | Value::Null) => "none".into(),
        other => shown(other, "none"),
    }
}

fn seconds(value: Option<&Value>) -> String {
    match value.and_then(Value::as_u64) {
        Some(secs) => format!("{secs} s"),
        None => shown(value, "not set"),
    }
}

fn bytes(value: Option<&Value>) -> String {
    match value.and_then(Value::as_u64) {
        Some(bytes) if bytes >= 1_000_000 => format!("{} MB", bytes as f64 / 1_000_000.0),
        Some(bytes) if bytes >= 1_000 => format!("{} kB", bytes as f64 / 1_000.0),
        Some(bytes) => format!("{bytes} B"),
        None => shown(value, "not set"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(groups: &[Group], slug: &str) -> Body {
        groups
            .iter()
            .find(|group| group.slug == slug)
            .map(|group| group.body.clone())
            .unwrap_or_else(|| panic!("no {slug} group"))
    }

    #[test]
    fn a_project_shows_its_own_switch_or_the_workspace_one_and_missing_keys_stay_unknown() {
        let config = serde_json::json!({
            "defaults": {"auto_dispatch": true, "auto_land": false},
            "projects": {"app": {
                "path": "app", "gate": ["just check", "just lint"], "auto_land": true,
                "auto_dispatch": null, "env": {"TOKEN": "secret"}
            }}
        });
        let Value::Object(config) = config else {
            panic!("an object");
        };
        let groups = groups(&config);
        let Body::Items { items, .. } = facts(&groups, "projects") else {
            panic!("projects list their entries");
        };
        let fact = |label: &str| {
            items[0]
                .facts
                .iter()
                .find(|(key, _)| *key == label)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(fact("Auto dispatch").as_deref(), Some("workspace · on"));
        assert_eq!(fact("Auto land").as_deref(), Some("on"));
        assert_eq!(fact("Gate").as_deref(), Some("just check\njust lint"));
        assert_eq!(fact("Environment").as_deref(), Some("TOKEN"));
        let Body::Facts(automation) = facts(&groups, "automation") else {
            panic!("automation is facts");
        };
        assert!(automation.contains(&("Running slots", NOT_REPORTED.into())));
    }
}
