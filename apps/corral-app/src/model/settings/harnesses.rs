//! The selectable harness catalog. Built-in keys always exist; a table in the draft replaces
//! the built-in definition of that name. Host observations only apply to the command reported
//! by the daemon, never to an unsaved replacement.

use std::collections::BTreeMap;

use corral_protocol::rows::{HarnessStatus, PiConfigStatus};
use serde_json::Value;
use toml_edit::DocumentMut;

use super::document::{self, Path};
use super::editor::{Editor, Part};
use super::form::built_in_table;
use super::live::{Standing, standing};
use crate::model::daemon::Feed;

pub const NOTE: &str =
    "Choose how Corral runs your agents. Built-ins need no entry in corral.toml.";
pub const HOST_NOTE: &str = "Detection checks for an executable on the daemon host. It does not check provider access or whether a session will launch.";
pub const CUSTOM_NOTE: &str =
    "Add a named command to the corral.toml draft, then assign it to a task profile.";
pub const TEMPLATE_NOTE: &str =
    "The built-in command needs no configuration. Create an override to edit its command or flags.";
pub const OVERRIDE_NOTE: &str = "This table replaces the built-in command and flags. Remove it to use the built-in definition again.";
pub const LOGIN_NOTE: &str = "Sign in from a terminal on the daemon host as the user running corrald. The app does not sign in or check credentials.";
pub const LOGIN_COMMAND: &str = "corral-pi login --provider openai-codex";
pub const CHILD_NOTE: &str = "These profiles configure Pi child agents in task workers and build sessions. They do not change the task worker’s model. Edit pi.toml on the daemon host.";
pub const PROFILES_NOTE: &str = "Task profiles select a harness for dispatch. This list reflects the corral.toml draft, including built-in profiles not overridden in the draft.";
pub const PI_PROFILE_NOTE: &str = "For a Pi task profile, set the harness key to pi. Use a provider/model selector to choose a model, and set effort in the task profile if needed.";
const PI_PATH: &str = ".corral-next/pi.toml";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltIn {
    pub key: &'static str,
    pub name: &'static str,
    pub mark: &'static str,
    pub about: &'static str,
    command: &'static str,
}

// These are the shipped engine defaults, used only if the daemon does not serve its own.
const BUILT_IN: [BuiltIn; 5] = [
    BuiltIn {
        key: "pi",
        name: "corral-pi",
        mark: "π",
        about: "Corral’s native Pi harness runs task sessions. Pi also powers chat and build sessions.",
        command: "corral-pi run {model_args} --prompt-file {prompt_file} --corral {corral}",
    },
    BuiltIn {
        key: "claude",
        name: "Claude",
        mark: "✳",
        about: "Runs tasks with the Claude CLI.",
        command: "claude {model_args} {prompt_ref}",
    },
    BuiltIn {
        key: "omo",
        name: "OMO",
        mark: "om",
        about: "Launches omo with a named session and the task profile’s arguments.",
        command: "omo -n {name} {model_args} {prompt_ref}",
    },
    BuiltIn {
        key: "omp",
        name: "OMP",
        mark: "op",
        about: "Launches omp with arguments from the task profile.",
        command: "omp {model_args} {prompt_ref}",
    },
    BuiltIn {
        key: "generic",
        name: "Generic",
        mark: ">_",
        about: "The default prints the assignment instead of starting an agent.",
        command: "cat {prompt_file}",
    },
];

pub fn builtin(key: &str) -> Option<&'static BuiltIn> {
    BUILT_IN.iter().find(|item| item.key == key)
}

pub fn keys() -> impl Iterator<Item = &'static str> {
    BUILT_IN.iter().map(|item| item.key)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub builtin: Option<&'static BuiltIn>,
    pub configured: bool,
}

impl Entry {
    pub fn title(&self) -> &str {
        self.builtin.map_or(self.key.as_str(), |item| item.name)
    }

    pub fn mark(&self) -> &'static str {
        self.builtin.map_or(">_", |item| item.mark)
    }

    pub fn kind(&self) -> &'static str {
        match (self.builtin, self.configured) {
            (Some(_), true) => "Built-in · overridden",
            (Some(_), false) => "Built-in harness",
            (None, _) => "Custom harness",
        }
    }

    pub fn description(&self) -> &'static str {
        self.builtin
            .map_or("Runs a command defined in corral.toml.", |item| item.about)
    }
}

pub fn entries(configured: &[String]) -> Vec<Entry> {
    BUILT_IN
        .iter()
        .map(|item| Entry {
            key: item.key.into(),
            builtin: Some(item),
            configured: configured.iter().any(|key| key == item.key),
        })
        .chain(
            configured
                .iter()
                .filter(|key| builtin(key).is_none())
                .map(|key| Entry {
                    key: key.clone(),
                    builtin: None,
                    configured: true,
                }),
        )
        .collect()
}

pub fn selected<'a>(items: &'a [Entry], chosen: Option<&str>) -> Option<&'a Entry> {
    chosen
        .and_then(|key| items.iter().find(|entry| entry.key == key))
        .or_else(|| items.first())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    pub command: String,
    pub model_flag: Option<String>,
    pub effort_flag: Option<String>,
}

impl Template {
    pub fn of(feed: &Feed, key: &str) -> Option<Self> {
        let item = builtin(key)?;
        if let Some(row) = built_in_table(feed, "harnesses").and_then(|table| table.get(key)) {
            return Some(Self {
                command: row.get("command")?.as_str()?.into(),
                model_flag: optional(row, "model_flag"),
                effort_flag: optional(row, "effort_flag"),
            });
        }
        Some(Self {
            command: item.command.into(),
            model_flag: (key != "generic").then(|| "--model".into()),
            effort_flag: (key != "generic").then(|| "--effort".into()),
        })
    }

    fn fields(&self) -> [(&'static str, Option<&str>); 3] {
        [
            ("command", Some(&self.command)),
            ("model_flag", self.model_flag.as_deref()),
            ("effort_flag", self.effort_flag.as_deref()),
        ]
    }

    /// The engine replaces the whole harness, so include both flags when starting an override.
    pub fn create_override(&self, doc: &mut DocumentMut, key: &str) -> bool {
        if builtin(key).is_none() || document::get(doc, &Path::of(&["harnesses", key])).is_some() {
            return false;
        }
        document::add_entry(doc, "harnesses", key);
        for (field, value) in self.fields() {
            if let Some(value) = value {
                document::set(
                    doc,
                    &Path::entry("harnesses", key, field),
                    toml_edit::Value::from(value),
                );
            }
        }
        true
    }

    pub fn example(&self, key: &str) -> String {
        let mut doc = DocumentMut::new();
        self.create_override(&mut doc, key);
        doc.to_string().trim_end().into()
    }
}

fn optional(row: &Value, key: &str) -> Option<String> {
    row.get(key)?.as_str().map(str::to_owned)
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Profile {
    pub name: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub editable: bool,
}

fn profiles(doc: &DocumentMut, feed: &Feed) -> BTreeMap<String, Vec<Profile>> {
    let mut entries: BTreeMap<String, (String, Profile)> = match built_in_table(feed, "profiles") {
        Some(built_in) => built_in
            .iter()
            .filter_map(|(name, row)| {
                Some((
                    name.clone(),
                    (
                        optional(row, "harness")?,
                        Profile {
                            name: name.clone(),
                            model: optional(row, "model"),
                            effort: optional(row, "effort"),
                            ..Profile::default()
                        },
                    ),
                ))
            })
            .collect(),
        // Current engine defaults; prefer the daemon's table whenever it provides one.
        None => keys()
            .map(|key| {
                (
                    key.into(),
                    (
                        key.into(),
                        Profile {
                            name: key.into(),
                            ..Profile::default()
                        },
                    ),
                )
            })
            .chain(std::iter::once((
                "omp-sol".into(),
                (
                    "omp".into(),
                    Profile {
                        name: "omp-sol".into(),
                        ..Profile::default()
                    },
                ),
            )))
            .collect(),
    };
    if let Some(table) = doc.get("profiles").and_then(toml_edit::Item::as_table_like) {
        for (name, item) in table.iter() {
            // An explicit table replaces a built-in even when the draft lacks a valid harness.
            entries.remove(name);
            let Some(row) = item.as_table_like() else {
                continue;
            };
            let field = |key| {
                row.get(key)
                    .and_then(toml_edit::Item::as_str)
                    .map(str::to_owned)
            };
            if let Some(harness) = field("harness") {
                entries.insert(
                    name.into(),
                    (
                        harness,
                        Profile {
                            name: name.into(),
                            model: field("model"),
                            effort: field("effort"),
                            editable: true,
                        },
                    ),
                );
            }
        }
    }
    let mut by_harness = BTreeMap::<String, Vec<Profile>>::new();
    for (_, (harness, profile)) in entries {
        by_harness.entry(harness).or_default().push(profile);
    }
    by_harness
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Detection {
    Detected,
    NotFound,
    Unknown,
    Draft,
    Stale,
    #[default]
    NotChecked,
}

impl Detection {
    pub fn label(self) -> &'static str {
        match self {
            Self::Detected => "Detected",
            Self::NotFound => "Not found",
            Self::Unknown => "Unknown",
            Self::Draft => "Not checked for this draft",
            Self::Stale => "Host data is stale",
            Self::NotChecked => "Not checked",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Discovery {
    pub status: Detection,
    pub executable: Option<String>,
}

fn discovery(doc: &DocumentMut, feed: &Feed, key: &str) -> Discovery {
    if standing(feed) == Standing::Stale {
        return Discovery {
            status: Detection::Stale,
            ..Discovery::default()
        };
    }
    let Some(settings) = feed
        .settings
        .as_ref()
        .filter(|settings| settings.problem.is_none())
    else {
        return Discovery::default();
    };
    let Some(fact) = settings
        .harness_facts
        .as_ref()
        .and_then(|rows| rows.iter().find(|row| row.name == key))
    else {
        return Discovery::default();
    };
    let draft = match document::get(doc, &Path::of(&["harnesses", key])) {
        Some(row) => row
            .as_table_like()
            .and_then(|row| row.get("command"))
            .and_then(toml_edit::Item::as_str)
            .map(str::to_owned),
        None => Template::of(feed, key).map(|template| template.command),
    };
    let served = settings
        .config
        .get("harnesses")
        .and_then(|table| table.get(key))
        .and_then(|row| row.get("command"))
        .and_then(Value::as_str);
    if served.is_none() || served != draft.as_deref() {
        return Discovery {
            status: Detection::Draft,
            ..Discovery::default()
        };
    }
    Discovery {
        status: match fact.status {
            HarnessStatus::Installed => Detection::Detected,
            HarnessStatus::NotFound => Detection::NotFound,
            HarnessStatus::Unknown => Detection::Unknown,
        },
        executable: fact
            .resolved_path
            .clone()
            .or_else(|| fact.executable.clone()),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Children {
    pub path: String,
    pub status: &'static str,
    pub profiles: Vec<Profile>,
}

impl Children {
    fn read(feed: &Feed) -> Self {
        let default = |status| Self {
            path: PI_PATH.into(),
            status,
            profiles: Vec::new(),
        };
        if standing(feed) == Standing::Stale {
            return default("Pi configuration data is stale.");
        }
        let Some(pi) = feed
            .settings
            .as_ref()
            .and_then(|settings| settings.pi_config.as_ref())
        else {
            return default("The daemon has not reported child profiles.");
        };
        let status = match pi.status {
            PiConfigStatus::Read if pi.child_profiles.is_empty() => {
                "No named child profiles in this file."
            }
            PiConfigStatus::Read => "",
            PiConfigStatus::Missing => "Pi configuration file not found on the daemon host.",
            PiConfigStatus::Unavailable => "The daemon could not read the child profiles.",
        };
        Self {
            path: pi.path.clone(),
            status,
            profiles: if matches!(pi.status, PiConfigStatus::Read) {
                pi.child_profiles
                    .iter()
                    .map(|profile| Profile {
                        name: profile.name.clone(),
                        model: Some(profile.model.clone()),
                        effort: profile.effort.clone(),
                        editable: false,
                    })
                    .collect()
            } else {
                Vec::new()
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Facts {
    pub discovery: Discovery,
    pub profiles: Vec<Profile>,
}

impl Facts {
    pub fn usage(&self) -> String {
        match self.profiles.len() {
            0 => "No task profiles".into(),
            1 => "1 task profile".into(),
            n => format!("{n} task profiles"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Details {
    pub rows: BTreeMap<String, Facts>,
    pub children: Children,
}

impl Details {
    pub fn read(editor: &Editor, feed: &Feed) -> Self {
        let rows = document::parse(&editor.draft)
            .map(|doc| {
                let mut profiles = profiles(&doc, feed);
                entries(&document::names(&doc, "harnesses"))
                    .into_iter()
                    .map(|item| {
                        let key = item.key;
                        let facts = Facts {
                            discovery: discovery(&doc, feed, &key),
                            profiles: profiles.remove(&key).unwrap_or_default(),
                        };
                        (key, facts)
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            rows,
            children: Children::read(feed),
        }
    }
}

pub fn choice(editor: &Editor) -> Option<&str> {
    editor.chosen.get(&Part::Harnesses).map(String::as_str)
}

#[cfg(test)]
mod tests;

pub fn invalid_custom(name: &str, configured: &[String]) -> Option<&'static str> {
    let name = name.trim();
    if builtin(name).is_some() {
        return Some("That name is built in. Select it to create an override.");
    }
    if configured.iter().any(|held| held == name) {
        return Some("This harness is already in the catalog.");
    }
    None
}
