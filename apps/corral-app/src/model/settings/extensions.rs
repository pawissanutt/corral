//! Settings › Configuration's Extensions part: one row per `[extensions.<name>]` block of the
//! draft (what it configures, for which projects, whether it is on) beside what corrald reports
//! of it (its last error and the events it skipped, `SettingsSnapshot.extensions`), and the
//! chosen block's fields. Command lines are text fields; a block is added or removed in File.

use toml_edit::{DocumentMut, Item};

use super::document::{self, Path, Seg};
use super::editor::{Part, Typed};
use super::form::{Field, Group, Kind};
use crate::model::daemon::Feed;

/// What the part says when the draft configures no extension.
pub const EMPTY: &str = "No extensions: an extension is a named block of your own command lines that Corral runs to give agents tools, fill a built-in view's slot, or react to events.";

/// Where the guide is, for the link beside `EMPTY` and under the part.
pub const GUIDE_URL: &str =
    "https://github.com/pawissanutt/corral/blob/main/docs/guide/extensions.md";
pub const GUIDE_LABEL: &str = "Extensions guide";

/// What the part says over its rows.
pub const NOTE: &str = "Command lines run with sh -c on the daemon's machine, with your permissions. Add or remove an extension in File.";

const KEYS: [&str; 5] = ["enabled", "projects", "run", "on", "facts"];
const TOOL_KEYS: [&str; 3] = ["run", "about", "record"];

/// One configured extension as the part lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    /// Its lines: tools by name, the slot it fills, the events it reacts to.
    pub shapes: String,
    pub projects: String,
    pub enabled: bool,
    /// What corrald reports; none until the block is saved and the daemon reads it.
    pub health: Option<Health>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    pub last_error: Option<String>,
    pub skipped: u64,
}

impl Row {
    /// Whether it is on and how it runs, in one line.
    pub fn status(&self) -> String {
        let on = if self.enabled { "on" } else { "off" };
        match &self.health {
            None => format!("{on} · not reported by corrald yet"),
            Some(health) => {
                let failing = if health.last_error.is_some() {
                    "failing"
                } else {
                    "no errors"
                };
                // A reported zero is said, so it reads apart from no report at all.
                match health.skipped {
                    1 => format!("{on} · {failing} · 1 event skipped"),
                    n => format!("{on} · {failing} · {n} events skipped"),
                }
            }
        }
    }

    pub fn failing(&self) -> bool {
        self.health
            .as_ref()
            .is_some_and(|health| health.last_error.is_some())
    }
}

fn path(name: &str, key: &str) -> Path {
    Path::entry("extensions", name, key)
}

fn tool_path(name: &str, tool: &str, key: &str) -> Path {
    Path::of(&["extensions", name, "tools", tool, key])
}

/// The tools an extension's block names, in document order.
fn tools(doc: &DocumentMut, name: &str) -> Vec<String> {
    document::get(doc, &path(name, "tools"))
        .and_then(Item::as_table_like)
        .map(|table| {
            table
                .iter()
                .filter(|(_, item)| item.is_table_like())
                .map(|(key, _)| key.to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn words(doc: &DocumentMut, path: &Path) -> Option<Vec<String>> {
    document::get(doc, path).map(|item| document::lines(Some(item)))
}

/// Every extension of the draft, in document order, with corrald's report of it.
pub fn rows(draft: &str, feed: &Feed) -> Vec<Row> {
    let Ok(doc) = document::parse(draft) else {
        return Vec::new();
    };
    let served = feed.settings.as_ref().map(|s| s.extensions.as_slice());
    document::names(&doc, "extensions")
        .into_iter()
        .map(|name| {
            let mut shapes = Vec::new();
            let tools = tools(&doc, &name);
            if !tools.is_empty() {
                let noun = if tools.len() == 1 { "tool" } else { "tools" };
                shapes.push(format!("{noun} {}", tools.join(", ")));
            }
            if document::get(&doc, &path(&name, "facts")).is_some() {
                shapes.push("slot facts".to_owned());
            }
            if document::get(&doc, &path(&name, "run")).is_some() {
                shapes.push(match words(&doc, &path(&name, "on")) {
                    None => "on every event".to_owned(),
                    Some(kinds) if kinds.is_empty() => "on no event".to_owned(),
                    Some(kinds) => format!("on {}", kinds.join(", ")),
                });
            }
            let shapes = if shapes.is_empty() {
                "no command lines".to_owned()
            } else {
                shapes.join(" · ")
            };
            let projects = match words(&doc, &path(&name, "projects")) {
                None => "every project".to_owned(),
                Some(projects) if projects.is_empty() => "no project".to_owned(),
                Some(projects) => projects.join(", "),
            };
            let enabled = document::get(&doc, &path(&name, "enabled"))
                .and_then(Item::as_bool)
                .unwrap_or(true);
            let health = served
                .and_then(|served| served.iter().find(|health| health.name == name))
                .map(|health| Health {
                    last_error: health.last_error.clone(),
                    skipped: health.skipped,
                });
            Row {
                name,
                shapes,
                projects,
                enabled,
                health,
            }
        })
        .collect()
}

fn field(path: Path, label: &'static str, kind: Kind, note: &'static str) -> Field {
    Field {
        path,
        label,
        kind,
        note: Some(note),
        offers: Vec::new(),
        caption: "",
    }
}

/// The chosen extension's fields: its own keys, then each tool's.
pub fn groups(doc: &DocumentMut, name: &str) -> Vec<Group> {
    let mut groups = vec![Group {
        title: None,
        rule: None,
        fields: KEYS
            .iter()
            .map(|key| {
                let at = path(name, key);
                let label = label(key);
                match *key {
                    "enabled" => field(
                        at,
                        label,
                        Kind::Switch { inherit: false },
                        "off keeps the block but runs none of its lines",
                    ),
                    "projects" => field(
                        at,
                        label,
                        Kind::Typed(Typed::Words),
                        "comma-separated project names",
                    ),
                    "run" => field(
                        at,
                        label,
                        Kind::Text,
                        "React: runs once per matching event, the event as JSON on stdin",
                    ),
                    "on" => field(
                        at,
                        label,
                        Kind::Typed(Typed::Words),
                        "comma-separated event kinds React runs for",
                    ),
                    _ => field(
                        at,
                        label,
                        Kind::Text,
                        "Ask: fills the system map's facts slot",
                    ),
                }
            })
            .collect(),
    }];
    for tool in tools(doc, name) {
        groups.push(Group {
            title: Some(format!("Tool {tool}")),
            rule: None,
            fields: vec![
                field(
                    tool_path(name, &tool, "run"),
                    label("tool.run"),
                    Kind::Text,
                    "what corral-next tool runs, with the caller's facts as JSON on stdin",
                ),
                field(
                    tool_path(name, &tool, "about"),
                    label("about"),
                    Kind::Text,
                    "the line the run prompt lists beside the tool's name",
                ),
                field(
                    tool_path(name, &tool, "record"),
                    label("record"),
                    Kind::Switch { inherit: false },
                    "keep the tool's first output in a run as an artifact",
                ),
            ],
        });
    }
    groups
}

fn label(key: &str) -> &'static str {
    match key {
        "enabled" => "Enabled",
        "projects" => "Projects",
        "run" => "React command",
        "on" => "Events",
        "facts" => "Facts command",
        "tool.run" => "Tool command",
        "about" => "About",
        "record" => "Record",
        _ => "Value",
    }
}

/// Every extension key of the draft a refusal can name or the form reads.
pub fn anchors(doc: &DocumentMut) -> Vec<Path> {
    let mut anchors = Vec::new();
    for name in document::names(doc, "extensions") {
        anchors.push(Path::of(&["extensions", &name]));
        anchors.extend(KEYS.iter().map(|key| path(&name, key)));
        for tool in tools(doc, &name) {
            anchors.extend(TOOL_KEYS.iter().map(|key| tool_path(&name, &tool, key)));
        }
    }
    anchors
}

/// Where an extension key shows: the part, the extension, and its label.
pub fn locate(keys: &[&Seg]) -> Option<(Part, Option<String>, Option<&'static str>)> {
    let key = |seg: &Seg| match seg {
        Seg::Key(key) => Some(key.clone()),
        Seg::Index(_) => None,
    };
    let entry = key(keys.get(1)?)?;
    let label = match keys.len() {
        2 => None,
        3 => Some(label(&key(keys[2])?)),
        5 if key(keys[2])? == "tools" => match key(keys[4])?.as_str() {
            "run" => Some(label("tool.run")),
            other => Some(label(other)),
        },
        _ => return None,
    };
    Some((Part::Extensions, Some(entry), label))
}

/// Whether an unset switch reads as on: an extension's `enabled`, which the engine defaults on.
pub fn unset_on(path: &Path) -> bool {
    matches!(
        path.0.as_slice(),
        [Seg::Key(root), Seg::Key(_), Seg::Key(key)] if root == "extensions" && key == "enabled"
    )
}

/// What an unset extension key means.
pub fn unset(keys: &[&str]) -> Option<&'static str> {
    Some(match keys {
        [_, "enabled"] => "unset · on",
        [_, "projects"] => "unset · every project",
        [_, "on"] => "unset · every event",
        [_, "run"] => "unset · reacts to no event",
        [_, "facts"] => "unset · fills no slot",
        [_, "tools", _, "about"] => "unset · the run prompt lists the name alone",
        [_, "tools", _, "record"] => "unset · off",
        [_, "tools", _, "run"] => "unset · corrald refuses a tool without a command",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use corral_protocol::rows::{ExtensionHealth, SettingsSnapshot};

    use super::*;
    use crate::model::settings::form::{Shown, Values};

    const DRAFT: &str = r#"
[extensions.notify]
on = ["blocked"]
run = "ntfy publish t"
enabled = false

[extensions.codeindex]
facts = "idx facts"
[extensions.codeindex.tools]
context = { run = "idx context", about = "Starting points" }
"#;

    /// A served report joins its block by name, and an unset `enabled` reads as on, as the
    /// engine defaults it.
    #[test]
    fn rows_join_health_and_default_enabled_on() {
        let feed = Feed {
            settings: Some(SettingsSnapshot {
                extensions: vec![ExtensionHealth {
                    name: "codeindex".into(),
                    shapes: vec!["tool".into(), "ask".into()],
                    enabled: true,
                    last_error: Some("exit 2".into()),
                    skipped: 0,
                }],
                ..SettingsSnapshot::default()
            }),
            ..Feed::default()
        };
        let rows = rows(DRAFT, &feed);
        assert_eq!(rows.len(), 2);
        assert!(!rows[0].enabled && rows[0].health.is_none());
        assert!(rows[1].enabled && rows[1].failing());
        assert_eq!(rows[1].shapes, "tool context · slot facts");
        // A reported zero skipped count is shown; no report says none.
        assert!(rows[1].status().contains("0 events skipped"));
        assert!(!rows[0].status().contains("skipped"));
    }

    /// A tool's command line inside an inline table is a form value, so its text field shows
    /// and edits it.
    #[test]
    fn tool_command_in_inline_table_is_a_form_value() {
        let values = Values::of(DRAFT);
        assert_eq!(
            values.get(&tool_path("codeindex", "context", "run")),
            Some(&Shown::Text("idx context".into()))
        );
    }
}
