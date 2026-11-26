//! What Settings › Configuration's form shows of the draft: each part's fields in the order an
//! owner tunes them, the entries a part lists, and each field's value (what an unset field
//! inherits is `inherit`'s).

use std::collections::BTreeMap;

use serde_json::{Map, Value as Json};
use toml_edit::{DocumentMut, Item};

use super::document::{self, Path};
use super::editor::{Editor, Part, Typed};
use super::{extensions, harnesses};
use crate::model::daemon::Feed;

/// What a field edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Text,
    Multiline,
    Typed(Typed),
    /// On or off; `inherit` adds the choice to leave it unset.
    Switch {
        inherit: bool,
    },
    /// An ordered list, one line per entry.
    Lines {
        count: usize,
        add: &'static str,
    },
    /// Names with string values; the names in document order.
    Pairs {
        names: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub path: Path,
    pub label: &'static str,
    pub kind: Kind,
    pub note: Option<&'static str>,
    /// Values offered beside what is typed, under `caption`.
    pub offers: Vec<String>,
    pub caption: &'static str,
}

/// Fields under one heading: a routing rule carries its place, for Up, Down and Remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub title: Option<String>,
    pub rule: Option<usize>,
    pub fields: Vec<Field>,
}

/// The entries a listing part offers, and the one it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entries {
    pub part: Part,
    pub names: Vec<String>,
    pub chosen: Option<String>,
    pub noun: &'static str,
}

/// One part's structure. It changes only when entries, lines or rules are added, removed or
/// moved, so typing redraws values and never the fields being typed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shape {
    Form {
        note: Option<&'static str>,
        entries: Option<Entries>,
        groups: Vec<Group>,
        empty: Option<&'static str>,
    },
    Harnesses(Vec<String>),
    Automations,
    File,
    /// The draft is not TOML, so the form cannot read it; the text view can.
    Unparsed(String),
}

const PROJECT_KEYS: [&str; 11] = [
    "path",
    "base",
    "gate",
    "reviewer",
    "profile",
    "running",
    "env",
    "auto_dispatch",
    "auto_land",
    "max_rounds",
    "branch_prefix",
];
const PROFILE_KEYS: [&str; 4] = ["harness", "model", "effort", "args"];
const HARNESS_KEYS: [&str; 3] = ["command", "model_flag", "effort_flag"];
const RULE_KEYS: [&str; 6] = [
    "kind",
    "size",
    "difficulty",
    "title_regex",
    "project",
    "profile",
];
const WORKSPACE: [(&str, &str); 7] = [
    ("limits", "running"),
    ("defaults", "profile"),
    ("defaults", "base"),
    ("defaults", "max_rounds"),
    ("defaults", "branch_prefix"),
    ("defaults", "auto_dispatch"),
    ("defaults", "auto_land"),
];
const ARTIFACTS: [&str; 2] = ["globs", "max_bytes"];

/// The owner's word for a key.
fn label(key: &str) -> &'static str {
    match key {
        "running" => "Dispatch slots",
        "profile" => "Worker profile",
        "base" => "Base branch",
        "max_rounds" => "Review rounds",
        "branch_prefix" => "Branch prefix",
        "auto_dispatch" => "Auto dispatch",
        "auto_land" => "Auto land",
        "path" => "Path",
        "gate" => "Gate lines",
        "reviewer" => "Reviewer",
        "env" => "Environment",
        "harness" => "Harness",
        "model" => "Model",
        "effort" => "Effort",
        "args" => "Arguments",
        "command" => "Command",
        "model_flag" => "Model flag",
        "effort_flag" => "Effort flag",
        "kind" => "Kinds",
        "size" => "Sizes",
        "difficulty" => "Difficulties",
        "title_regex" => "Title pattern",
        "project" => "Project",
        "globs" => "Globs",
        "max_bytes" => "Size limit",
        _ => "Value",
    }
}

fn field(path: Path, key: &str, kind: Kind) -> Field {
    Field {
        path,
        label: label(key),
        kind,
        note: None,
        offers: Vec::new(),
        caption: "",
    }
}

impl Field {
    fn note(mut self, note: &'static str) -> Self {
        self.note = Some(note);
        self
    }

    fn offer(mut self, caption: &'static str, offers: Vec<String>) -> Self {
        self.caption = caption;
        self.offers = offers;
        self
    }
}

fn lines(doc: &DocumentMut, path: Path, key: &str, add: &'static str) -> Field {
    let count = document::lines(document::get(doc, &path)).len();
    field(path, key, Kind::Lines { count, add })
}

/// The part's structure from the draft and corrald's project rows.
pub fn shape(editor: &Editor, feed: &Feed) -> Shape {
    if editor.part == Part::Automations {
        return Shape::Automations;
    }
    if editor.part == Part::File {
        return Shape::File;
    }
    let doc = match document::parse(&editor.draft) {
        Ok(doc) => doc,
        Err(error) => return Shape::Unparsed(error),
    };
    let profiles = document::names(&doc, "profiles");
    let (note, entries, groups, empty) = match editor.part {
        Part::Workspace => (None, None, workspace(&profiles), None),
        Part::Artifacts => (
            Some("Files the artifact finder lists, matched from each project's root."),
            None,
            vec![Group {
                title: None,
                rule: None,
                fields: vec![
                    lines(&doc, Path::of(&["artifacts", "globs"]), "globs", "Add glob"),
                    field(
                        Path::of(&["artifacts", "max_bytes"]),
                        "max_bytes",
                        Kind::Typed(Typed::Number),
                    )
                    .note("bytes; larger files are listed but not opened"),
                ],
            }],
            None,
        ),
        Part::Routing => {
            let mut targets = profiles.clone();
            append_offers(&mut targets, built_in_names(feed, "profiles"));
            (None, None, routing(&doc, &targets), None)
        }
        Part::Harnesses => return Shape::Harnesses(document::names(&doc, "harnesses")),
        part => {
            let entries = listed(editor, &doc, part);
            let groups = entries
                .chosen
                .as_deref()
                .map(|name| match part {
                    Part::Extensions => extensions::groups(&doc, name),
                    _ => vec![entry(&doc, feed, part, name, &profiles)],
                })
                .unwrap_or_default();
            let (note, empty) = match part {
                Part::Projects => (None, Some("No projects: corral manages no repository yet.")),
                Part::Profiles => (None, Some("No profiles: dispatch has nothing to run.")),
                Part::Extensions => (Some(extensions::NOTE), Some(extensions::EMPTY)),
                _ => (None, None),
            };
            (note, Some(entries), groups, empty)
        }
    };
    Shape::Form {
        note,
        entries,
        groups,
        empty,
    }
}

fn workspace(profiles: &[String]) -> Vec<Group> {
    let path = |table: &str, key: &str| Path::of(&[table, key]);
    vec![
        Group {
            title: Some("Limits".into()),
            rule: None,
            fields: vec![
                field(
                    path("limits", "running"),
                    "running",
                    Kind::Typed(Typed::Number),
                )
                .note("tasks in Running, Blocked or Review at once, including every review stage"),
            ],
        },
        Group {
            title: Some("Defaults".into()),
            rule: None,
            fields: vec![
                field(path("defaults", "profile"), "profile", Kind::Text)
                    .offer("Profiles", profiles.to_vec()),
                field(path("defaults", "base"), "base", Kind::Text),
                field(
                    path("defaults", "max_rounds"),
                    "max_rounds",
                    Kind::Typed(Typed::Number),
                ),
                field(
                    path("defaults", "branch_prefix"),
                    "branch_prefix",
                    Kind::Text,
                ),
                field(
                    path("defaults", "auto_dispatch"),
                    "auto_dispatch",
                    Kind::Switch { inherit: false },
                )
                .note("start Ready tasks when a slot is free"),
                field(
                    path("defaults", "auto_land"),
                    "auto_land",
                    Kind::Switch { inherit: false },
                )
                .note("land after gate and review without you"),
            ],
        },
    ]
}

fn listed(editor: &Editor, doc: &DocumentMut, part: Part) -> Entries {
    let names = part
        .root()
        .map(|root| document::names(doc, root))
        .unwrap_or_default();
    let chosen = editor
        .chosen
        .get(&part)
        .filter(|name| names.contains(name))
        .or_else(|| names.first())
        .cloned();
    Entries {
        part,
        names,
        chosen,
        noun: match part {
            Part::Projects => "project",
            Part::Profiles => "profile",
            Part::Extensions => "extension",
            _ => "harness",
        },
    }
}

fn entry(doc: &DocumentMut, feed: &Feed, part: Part, name: &str, profiles: &[String]) -> Group {
    let root = part.root().unwrap_or_default();
    let at = |key: &str| Path::entry(root, name, key);
    let fields = match part {
        Part::Projects => PROJECT_KEYS
            .iter()
            .map(|key| {
                let path = at(key);
                match *key {
                    "gate" => lines(doc, path, key, "Add gate line")
                        .note("each line runs with sh -c in the worktree, in order; exit 0 passes"),
                    "env" => {
                        let names = document::pairs(document::get(doc, &path))
                            .into_iter()
                            .map(|(name, _)| name)
                            .collect();
                        field(path, key, Kind::Pairs { names })
                            .note("exported to gate lines and sessions")
                    }
                    "running" => Field {
                        label: "Slot cap",
                        ..field(path, key, Kind::Typed(Typed::Number))
                            .note("this project's tasks in Running, Blocked or Review at once")
                    },
                    "max_rounds" => field(path, key, Kind::Typed(Typed::Number)),
                    "auto_dispatch" | "auto_land" => {
                        field(path, key, Kind::Switch { inherit: true })
                    }
                    "base" => field(path, key, Kind::Text).offer(
                        crate::model::projects::BRANCHES_CAPTION,
                        crate::model::projects::branches(feed, Some(name)),
                    ),
                    "profile" | "reviewer" => {
                        field(path, key, Kind::Text).offer("Profiles", profiles.to_vec())
                    }
                    "path" => field(path, key, Kind::Text).note("relative to the workspace root"),
                    _ => field(path, key, Kind::Text),
                }
            })
            .collect(),
        Part::Profiles => PROFILE_KEYS
            .iter()
            .map(|key| match *key {
                "harness" => {
                    let mut harnesses = document::names(doc, "harnesses");
                    if let Some(built_in) = built_in_table(feed, "harnesses") {
                        append_offers(&mut harnesses, built_in.keys().map(String::as_str));
                    } else {
                        append_offers(&mut harnesses, harnesses::keys());
                    }
                    field(at(key), key, Kind::Text).offer("Harnesses", harnesses)
                }
                "args" => lines(doc, at(key), key, "Add argument")
                    .note("one argument per line, after the model and effort flags"),
                _ => field(at(key), key, Kind::Text),
            })
            .collect(),
        _ => Vec::new(),
    };
    Group {
        title: None,
        rule: None,
        fields,
    }
}

pub fn harness_fields(name: &str) -> Vec<Field> {
    HARNESS_KEYS
        .into_iter()
        .map(|key| {
            let control = field(Path::entry("harnesses", name, key), key, Kind::Text);
            if key == "command" {
                control.note("An owner-written shell line. Placeholders: {prompt_ref}, {prompt_file}, {model_args}, {name}, {task}, {worktree}, {corral}.")
            } else {
                control
            }
        })
        .collect()
}

pub(super) fn built_in_table<'a>(
    feed: &'a Feed,
    table: &str,
) -> Option<&'a serde_json::Map<String, serde_json::Value>> {
    feed.settings
        .as_ref()?
        .built_in_defaults
        .as_ref()?
        .get(table)?
        .as_object()
}

fn built_in_names<'a>(feed: &'a Feed, table: &str) -> impl Iterator<Item = &'a str> {
    built_in_table(feed, table)
        .into_iter()
        .flat_map(|table| table.keys().map(String::as_str))
}

fn append_offers<'a>(offers: &mut Vec<String>, extra: impl IntoIterator<Item = &'a str>) {
    for name in extra {
        if !offers.iter().any(|current| current == name) {
            offers.push(name.to_owned());
        }
    }
}

fn routing(doc: &DocumentMut, profiles: &[String]) -> Vec<Group> {
    let projects = document::names(doc, "projects");
    (0..document::rule_count(doc))
        .map(|index| Group {
            title: Some(format!("Rule {}", index + 1)),
            rule: Some(index),
            fields: RULE_KEYS
                .iter()
                .map(|key| {
                    let path = Path::rule(index, key);
                    match *key {
                        "kind" | "size" | "difficulty" => {
                            field(path, key, Kind::Typed(Typed::Words))
                                .note("comma-separated; any when empty")
                        }
                        "project" => {
                            field(path, key, Kind::Text).offer("Projects", projects.clone())
                        }
                        "profile" => {
                            field(path, key, Kind::Text).offer("Profiles", profiles.to_vec())
                        }
                        _ => field(path, key, Kind::Text).note("a regular expression"),
                    }
                })
                .collect(),
        })
        .collect()
}

/// Every key and entry of the draft a refusal can name, so each reason finds its place.
pub fn anchors(doc: &DocumentMut) -> Vec<Path> {
    let mut anchors: Vec<Path> = WORKSPACE
        .iter()
        .map(|(table, key)| Path::of(&[table, key]))
        .chain(ARTIFACTS.iter().map(|key| Path::of(&["artifacts", key])))
        .collect();
    for (root, keys) in [
        ("projects", &PROJECT_KEYS[..]),
        ("profiles", &PROFILE_KEYS[..]),
        ("harnesses", &HARNESS_KEYS[..]),
    ] {
        for name in document::names(doc, root) {
            anchors.push(Path::of(&[root, &name]));
            anchors.extend(keys.iter().map(|key| Path::entry(root, &name, key)));
        }
    }
    for index in 0..document::rule_count(doc) {
        anchors.push(Path(vec![
            document::Seg::Key("routing".into()),
            document::Seg::Index(index),
        ]));
        anchors.extend(RULE_KEYS.iter().map(|key| Path::rule(index, key)));
    }
    anchors.extend(extensions::anchors(doc));
    for (section, keys) in [
        ("agents", &super::automation::AGENT_KEYS[..]),
        ("rules", &super::automation::RULE_KEYS[..]),
    ] {
        for id in super::automation::names(doc, section) {
            anchors.extend(
                keys.iter()
                    .map(|key| super::automation::path(section, &id, key)),
            );
        }
    }
    anchors
}

/// Where a key shows: its part, its entry (or rule), and its label (none for an entry).
pub fn locate(path: &Path) -> Option<(Part, Option<String>, Option<&'static str>)> {
    use super::document::Seg;
    let keys: Vec<&Seg> = path.0.iter().collect();
    if matches!(keys.first(), Some(Seg::Key(root)) if root == "extensions") {
        return extensions::locate(&keys);
    }
    match keys.as_slice() {
        [Seg::Key(root), Seg::Key(section), Seg::Key(id), ..] if root == "automation" => {
            Some((Part::Automations, Some(format!("{section}: {id}")), None))
        }
        [Seg::Key(root), Seg::Key(key)] => {
            let part = match root.as_str() {
                "artifacts" => Part::Artifacts,
                "limits" | "defaults" => Part::Workspace,
                "projects" => return Some((Part::Projects, Some(key.clone()), None)),
                "profiles" => return Some((Part::Profiles, Some(key.clone()), None)),
                "harnesses" => return Some((Part::Harnesses, Some(key.clone()), None)),
                _ => return None,
            };
            Some((part, None, Some(label(key))))
        }
        [Seg::Key(root), Seg::Key(name), Seg::Key(key)] => {
            let part = match root.as_str() {
                "projects" => Part::Projects,
                "profiles" => Part::Profiles,
                "harnesses" => Part::Harnesses,
                _ => return None,
            };
            Some((part, Some(name.clone()), Some(label(key))))
        }
        [Seg::Key(root), Seg::Index(index)] if root == "routing" => {
            Some((Part::Routing, Some(format!("Rule {}", index + 1)), None))
        }
        [Seg::Key(root), Seg::Index(index), Seg::Key(key)] if root == "routing" => Some((
            Part::Routing,
            Some(format!("Rule {}", index + 1)),
            Some(label(key)),
        )),
        _ => None,
    }
}

/// Where a reason is, in the owner's words: "Projects › site › Worker profile".
pub fn place(path: &Path) -> String {
    match locate(path) {
        Some((part, entry, label)) => [
            Some(part.label().to_owned()),
            entry,
            label.map(str::to_owned),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" › "),
        None => path.to_string(),
    }
}

/// A field's value in the draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    Text(String),
    Flag(bool),
    Lines(Vec<String>),
    Pairs(Vec<(String, String)>),
}

/// Every form key the draft sets, read once per change to the draft.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Values(BTreeMap<Path, Shown>);

impl Values {
    pub fn of(text: &str) -> Self {
        let Ok(doc) = document::parse(text) else {
            return Self::default();
        };
        Self(
            anchors(&doc)
                .into_iter()
                // An entry's own anchor is its whole table; its keys are read one by one.
                .filter(|path| {
                    path.0.len() != 2 || !matches!(locate(path), Some((_, Some(_), None)))
                })
                .filter_map(|path| {
                    let item = document::get(&doc, &path)?;
                    Some((path, shown(item)?))
                })
                .collect(),
        )
    }

    pub fn get(&self, path: &Path) -> Option<&Shown> {
        self.0.get(path)
    }

    pub fn text(&self, path: &Path) -> String {
        match self.get(path) {
            Some(Shown::Text(text)) => text.clone(),
            Some(Shown::Flag(on)) => on.to_string(),
            Some(Shown::Lines(lines)) => lines.join(", "),
            Some(Shown::Pairs(_)) | None => String::new(),
        }
    }

    pub fn flag(&self, path: &Path) -> Option<bool> {
        match self.get(path) {
            Some(Shown::Flag(on)) => Some(*on),
            _ => None,
        }
    }

    pub fn lines(&self, path: &Path) -> Vec<String> {
        match self.get(path) {
            Some(Shown::Lines(lines)) => lines.clone(),
            Some(Shown::Text(line)) => vec![line.clone()],
            _ => Vec::new(),
        }
    }

    pub fn pair(&self, path: &Path, name: &str) -> String {
        match self.get(path) {
            Some(Shown::Pairs(pairs)) => pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .unwrap_or_default(),
            _ => String::new(),
        }
    }
}

fn shown(item: &Item) -> Option<Shown> {
    if item.is_table_like() {
        return Some(Shown::Pairs(document::pairs(Some(item))));
    }
    Some(match item.as_value()? {
        toml_edit::Value::Boolean(on) => Shown::Flag(*on.value()),
        toml_edit::Value::Array(_) => Shown::Lines(document::lines(Some(item))),
        value => Shown::Text(document::shown(value)),
    })
}

/// The draft's and the saved file's values, which an unset field's inheritance reads.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Live {
    pub draft: Values,
    pub saved: Values,
}

impl Live {
    pub fn of(editor: &Editor) -> Self {
        Self {
            draft: Values::of(&editor.draft),
            saved: editor.saved.as_deref().map(Values::of).unwrap_or_default(),
        }
    }
}

/// Under a number field whose text is not a whole number.
pub const WHOLE_NUMBER: &str = "Type a whole number, or leave it empty to unset it.";

/// How many form keys the draft changes from the saved file.
fn changed(live: &Live) -> usize {
    let draft = &live.draft.0;
    let saved = &live.saved.0;
    draft
        .iter()
        .filter(|(path, value)| saved.get(*path) != Some(*value))
        .count()
        + saved
            .keys()
            .filter(|path| !draft.contains_key(*path))
            .count()
}

/// Where the file stands, for the save bar.
pub fn status(editor: &Editor, live: &Live) -> String {
    match (&editor.saved, &editor.waiting) {
        (_, Some(waiting)) if waiting.save => "Saving… corrald checks the file first.".into(),
        (None, Some(_)) => "Reading corral.toml from corrald…".into(),
        (None, None) => "corral.toml is not read yet.".into(),
        (Some(_), _) if editor.dirty() => match changed(live) {
            0 => "Unsaved edits to the file's text.".into(),
            1 => "1 unsaved change.".into(),
            n => format!("{n} unsaved changes."),
        },
        (Some(_), _) => "No unsaved changes.".into(),
    }
}

/// The refusal's heading over its reasons.
pub const REFUSED: &str =
    "corrald refused the save, so corral.toml is unchanged and your edits are kept.";

/// What the File part says above the text.
pub const FILE_NOTE: &str = "The whole file, for anything the form does not cover. The form and this text are one draft; Save sends it.";

/// A byte count in owner words.
pub fn bytes(value: &str) -> String {
    match value.trim().parse::<u64>() {
        Ok(bytes) if bytes >= 1_000_000 => format!("{} MB", bytes as f64 / 1_000_000.0),
        Ok(bytes) if bytes >= 1_000 => format!("{} kB", bytes as f64 / 1_000.0),
        Ok(bytes) => format!("{bytes} B"),
        Err(_) => value.to_owned(),
    }
}

/// The phone root list's line for Configuration, from corrald's parsed configuration.
pub fn subtitle(config: &Map<String, Json>) -> String {
    let projects = config
        .get("projects")
        .and_then(Json::as_object)
        .map_or(0, Map::len);
    let flag = |key: &str| {
        config
            .get("defaults")
            .and_then(|defaults| defaults.get(key))
            .and_then(super::inherit::json_words)
            .unwrap_or_else(|| "off".into())
    };
    let noun = if projects == 1 { "project" } else { "projects" };
    format!(
        "{projects} {noun} · auto dispatch {} · auto land {}",
        flag("auto_dispatch"),
        flag("auto_land")
    )
}
