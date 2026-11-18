//! Settings › Configuration's editor: `corral.toml` as `config.get` returned it, and the draft
//! the owner changes. Every change edits the draft document in place (see `document`), the
//! form and the plain-text view alike; Save sends the whole draft in one `config.put`. corrald
//! checks it with the engine and either replaces the file or refuses with reasons, each naming
//! the key it is about where it can; a refusal leaves the draft as it was, each reason shown on
//! its field or at the top.

use std::collections::{BTreeMap, BTreeSet};

use corral_protocol::config::ConfigDocument;
use toml_edit::{DocumentMut, Value};

use super::State;
use super::document::{self, ListEdit, Path, RuleEdit, Seg};
use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;

/// The parts of the editor, one shown at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Part {
    #[default]
    Workspace,
    Projects,
    Profiles,
    Harnesses,
    Routing,
    Artifacts,
    /// The whole file as text, for anything the form does not cover.
    File,
}

impl Part {
    pub const ALL: [Part; 7] = [
        Part::Workspace,
        Part::Projects,
        Part::Profiles,
        Part::Harnesses,
        Part::Routing,
        Part::Artifacts,
        Part::File,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Part::Workspace => "Workspace",
            Part::Projects => "Projects",
            Part::Profiles => "Profiles",
            Part::Harnesses => "Harnesses",
            Part::Routing => "Routing",
            Part::Artifacts => "Artifacts",
            Part::File => "File",
        }
    }

    /// The root table whose entries this part lists, for the parts that list entries.
    pub const fn root(self) -> Option<&'static str> {
        match self {
            Part::Projects => Some("projects"),
            Part::Profiles => Some("profiles"),
            Part::Harnesses => Some("harnesses"),
            _ => None,
        }
    }
}

/// The request corrald has not answered yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiting {
    pub key: Id,
    pub save: bool,
}

/// One reason a save was refused: the key it names (a field, or an entry such as a profile),
/// or none when it names no key, and what corrald says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    pub at: Option<Path>,
    pub line: String,
}

/// How a typed field's text becomes a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Typed {
    /// A whole number; empty unsets it.
    Number,
    /// Comma-separated words, written as a list; empty unsets it.
    Words,
}

/// One change the owner makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// A string key; empty unsets it.
    Text(Path, String),
    Typed(Path, Typed, String),
    /// An on/off key; `None` unsets it so the inherited value applies.
    Switch(Path, Option<bool>),
    List(Path, ListEdit),
    /// One name of a table of strings (an environment); `None` removes it.
    Pair(Path, String, Option<String>),
    AddEntry(Part, String),
    RemoveEntry(Part, String),
    Rules(RuleEdit),
    /// The whole file, from the plain-text view.
    File(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Editor {
    /// The file as corrald last returned or accepted it; none until `config.get` answers.
    pub saved: Option<String>,
    pub draft: String,
    pub waiting: Option<Waiting>,
    /// Why the last load or save failed outside a refusal, or why a save was not sent.
    pub problem: Option<String>,
    /// The last refused save's reasons.
    pub reasons: Vec<Reason>,
    pub part: Part,
    /// The entry each listing part shows.
    pub chosen: BTreeMap<Part, String>,
    /// What was typed into number and word fields, shown as typed while the draft holds the
    /// value it means.
    pub raw: BTreeMap<Path, String>,
    /// Number fields whose text is not a whole number; the draft keeps their last value.
    pub invalid: BTreeSet<Path>,
    /// The field the last edit added (a list's new line, a new entry's first key, a new rule's
    /// first key), which takes the keyboard focus when it shows.
    pub added: Option<Path>,
}

impl Editor {
    /// Edits not yet saved.
    pub fn dirty(&self) -> bool {
        !self.invalid.is_empty()
            || self
                .saved
                .as_ref()
                .is_some_and(|saved| *saved != self.draft)
    }

    /// The reasons that name `path`.
    pub fn reasons_at<'a>(&'a self, path: &'a Path) -> impl Iterator<Item = &'a str> + 'a {
        self.reasons
            .iter()
            .filter(move |reason| reason.at.as_ref() == Some(path))
            .map(|reason| reason.line.as_str())
    }

    fn apply(&mut self, edit: &Edit) -> Result<(), String> {
        if let Edit::File(text) = edit {
            self.draft.clone_from(text);
            self.raw.clear();
            self.invalid.clear();
            return Ok(());
        }
        let mut doc = document::parse(&self.draft)?;
        self.added = None;
        match edit {
            Edit::Text(path, text) if text.is_empty() => document::remove(&mut doc, path),
            Edit::Text(path, text) => document::set(&mut doc, path, Value::from(text.as_str())),
            Edit::Typed(path, typed, text) => self.typed(&mut doc, path, *typed, text),
            Edit::Switch(path, None) => document::remove(&mut doc, path),
            Edit::Switch(path, Some(on)) => document::set(&mut doc, path, Value::from(*on)),
            Edit::List(path, edit) => {
                document::edit_list(&mut doc, path, edit);
                if *edit == ListEdit::Add {
                    let last = document::lines(document::get(&doc, path)).len();
                    let mut line = path.clone();
                    line.0.push(Seg::Index(last.saturating_sub(1)));
                    self.added = Some(line);
                }
            }
            Edit::Pair(path, name, value) => {
                document::set_pair(&mut doc, path, name, value.as_deref());
            }
            Edit::AddEntry(part, name) => {
                let name = name.trim();
                let Some(root) = part.root().filter(|_| !name.is_empty()) else {
                    return Ok(());
                };
                document::add_entry(&mut doc, root, name);
                self.chosen.insert(*part, name.to_owned());
                let first = match part {
                    Part::Projects => "path",
                    Part::Profiles => "harness",
                    _ => "command",
                };
                self.added = Some(Path::entry(root, name, first));
            }
            Edit::RemoveEntry(part, name) => {
                if let Some(root) = part.root() {
                    document::remove_entry(&mut doc, root, name);
                    self.chosen.remove(part);
                }
            }
            Edit::Rules(edit) => {
                document::edit_rules(&mut doc, edit);
                if matches!(edit, RuleEdit::Add(_)) {
                    let last = document::rule_count(&doc).saturating_sub(1);
                    self.added = Some(Path::rule(last, "kind"));
                }
            }
            Edit::File(_) => {}
        }
        self.draft = doc.to_string();
        Ok(())
    }

    fn typed(&mut self, doc: &mut DocumentMut, path: &Path, typed: Typed, text: &str) {
        self.raw.insert(path.clone(), text.to_owned());
        self.invalid.remove(path);
        let words = text.trim();
        if words.is_empty() {
            document::remove(doc, path);
            return;
        }
        match typed {
            Typed::Number => match words.parse::<i64>() {
                Ok(number) => document::set(doc, path, Value::from(number)),
                Err(_) => {
                    self.invalid.insert(path.clone());
                }
            },
            Typed::Words => {
                let mut list = toml_edit::Array::new();
                for word in words
                    .split(',')
                    .map(str::trim)
                    .filter(|word| !word.is_empty())
                {
                    list.push(word);
                }
                document::set(doc, path, Value::Array(list));
            }
        }
    }

    /// Sorts each line of a refusal under the longest key of the draft it starts with.
    fn refuse(&mut self, reason: &str) {
        let anchors = document::parse(&self.draft)
            .map(|doc| super::form::anchors(&doc))
            .unwrap_or_default();
        self.reasons = reason
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| place(line, &anchors))
            .collect();
    }
}

/// The reason under the longest anchor `line` starts with, the anchor cut from its words.
fn place(line: &str, anchors: &[Path]) -> Reason {
    let found = anchors
        .iter()
        .map(|anchor| (anchor, anchor.to_string()))
        .filter(|(_, name)| {
            line.strip_prefix(name.as_str())
                .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', ' ', '.', '[']))
        })
        .max_by_key(|(_, name)| name.len());
    match found {
        Some((anchor, name)) => Reason {
            at: Some(anchor.clone()),
            line: line[name.len()..].trim_start_matches([':', ' ']).to_owned(),
        },
        None => Reason {
            at: None,
            line: line.to_owned(),
        },
    }
}

impl State {
    /// Reads `corral.toml` once the Configuration section opens, unless it is read or being read.
    pub fn open_config(&mut self) -> Vec<Effect> {
        if self.config.saved.is_some() || self.config.waiting.is_some() {
            return Vec::new();
        }
        self.request_config(None)
    }

    /// Reads the file again; only offered without unsaved edits.
    pub fn reload_config(&mut self) -> Vec<Effect> {
        if self.config.waiting.is_some() || self.config.dirty() {
            return Vec::new();
        }
        self.request_config(None)
    }

    fn request_config(&mut self, put: Option<String>) -> Vec<Effect> {
        let key = self.ids.next_id();
        self.config.waiting = Some(Waiting {
            key: key.clone(),
            save: put.is_some(),
        });
        self.config.problem = None;
        vec![Effect::Config { key, put }]
    }

    pub fn config_part(&mut self, part: Part) -> Vec<Effect> {
        self.config.part = part;
        Vec::new()
    }

    pub fn choose_entry(&mut self, part: Part, name: String) -> Vec<Effect> {
        self.config.chosen.insert(part, name);
        Vec::new()
    }

    /// Applies one change to the draft; the reasons on the changed key are now out of date.
    pub fn edit_config(&mut self, edit: Edit) -> Vec<Effect> {
        if self.config.saved.is_none() {
            return Vec::new();
        }
        match self.config.apply(&edit) {
            Ok(()) => {
                let changed = match &edit {
                    Edit::Text(path, _)
                    | Edit::Typed(path, ..)
                    | Edit::Switch(path, _)
                    | Edit::List(path, _)
                    | Edit::Pair(path, ..) => Some(path),
                    _ => None,
                };
                if let Some(path) = changed {
                    self.config
                        .reasons
                        .retain(|reason| reason.at.as_ref() != Some(path));
                }
                self.config.problem = None;
            }
            Err(error) => self.config.problem = Some(format!("{UNPARSED} {error}")),
        }
        Vec::new()
    }

    /// Sends the whole draft in one `config.put`; a number field that is not a number keeps it
    /// here, since the file could not say what was typed.
    pub fn save_config(&mut self) -> Vec<Effect> {
        if self.config.waiting.is_some() || self.config.saved.is_none() {
            return Vec::new();
        }
        if !self.config.invalid.is_empty() {
            self.config.problem = Some(NOT_A_NUMBER.into());
            return Vec::new();
        }
        let draft = self.config.draft.clone();
        self.request_config(Some(draft))
    }

    /// Shows the part, and the entry, a reason is about.
    pub fn show_config(&mut self, path: &Path) -> Vec<Effect> {
        if let Some((part, entry, _)) = super::form::locate(path) {
            self.config.part = part;
            if let (Some(entry), Some(_)) = (entry, part.root()) {
                self.config.chosen.insert(part, entry);
            }
        }
        Vec::new()
    }

    /// The request never left this device; the draft stays as it is.
    pub fn config_unsent(&mut self, why: String) -> Vec<Effect> {
        self.config.waiting = None;
        self.config.problem = Some(why);
        Vec::new()
    }

    /// Drops every unsaved edit and the last refusal.
    pub fn discard_config(&mut self) -> Vec<Effect> {
        if let Some(saved) = &self.config.saved {
            self.config.draft.clone_from(saved);
        }
        self.config.raw.clear();
        self.config.invalid.clear();
        self.config.reasons.clear();
        self.config.problem = None;
        Vec::new()
    }

    /// Reads corrald's answer to the request in flight, once it has one.
    pub fn settle_config(&mut self, feed: &Feed) -> Vec<Effect> {
        let Some(waiting) = self.config.waiting.clone() else {
            return Vec::new();
        };
        let Some(operation) = feed.operations.get(&waiting.key) else {
            return Vec::new();
        };
        if operation.loading {
            return Vec::new();
        }
        self.config.waiting = None;
        let editor = &mut self.config;
        if let Some(ConfigDocument { toml }) = operation.decode::<ConfigDocument>() {
            let fresh = !editor.dirty();
            if waiting.save || fresh {
                editor.draft.clone_from(&toml);
                editor.raw.clear();
                editor.invalid.clear();
            }
            editor.saved = Some(toml);
            editor.problem = None;
            editor.reasons.clear();
            return if waiting.save {
                vec![Effect::Toast(SAVED.into())]
            } else {
                Vec::new()
            };
        }
        if let Some(refusal) = &operation.refusal {
            editor.problem = None;
            editor.refuse(&refusal.reason);
        } else if let Some(error) = &operation.error {
            editor.problem = Some(if waiting.save {
                format!("Not saved: {}", error.message)
            } else {
                format!("corral.toml could not be read: {}", error.message)
            });
        }
        Vec::new()
    }
}

/// The toast after corrald accepts a save.
pub const SAVED: &str = "Saved corral.toml; corrald uses it from now on.";
const NOT_A_NUMBER: &str =
    "Not sent: a number field holds something other than a whole number. Fix the marked field.";
/// Why a form edit did not apply: the draft is not TOML, which the text view says where.
const UNPARSED: &str = "The form cannot change the file until it parses again:";

#[cfg(test)]
mod tests {
    use corral_types::Refusal;

    use super::*;
    use crate::model::daemon::Operation;

    const FILE: &str = "# owner notes\n[limits]\nrunning = 4\n\n[projects.app] # main repo\npath = \"app\"\ngate = [\"just check\"]\n\n[unknown_table]\nkept = 1\n";

    fn answered(state: &mut State, feed: &mut Feed, value: Option<String>, refusal: Option<&str>) {
        let key = state
            .config
            .waiting
            .as_ref()
            .map(|w| w.key.clone())
            .unwrap();
        feed.operations.insert(
            key,
            Operation {
                method: "config".into(),
                loading: false,
                value: value.map(|toml| serde_json::json!({ "toml": toml })),
                error: refusal.map(|reason| crate::model::daemon::ClientError {
                    code: "refused".into(),
                    message: reason.into(),
                }),
                refusal: refusal.map(|reason| Refusal {
                    reason: reason.into(),
                    next: Vec::new(),
                }),
            },
        );
        state.settle_config(feed);
    }

    fn loaded() -> (State, Feed) {
        let mut state = State::default();
        let mut feed = Feed::default();
        state.open_config();
        answered(&mut state, &mut feed, Some(FILE.into()), None);
        (state, feed)
    }

    #[test]
    fn a_save_sends_the_edited_document_with_its_comments_and_unknown_tables() {
        let (mut state, _) = loaded();
        let gate = Path::entry("projects", "app", "gate");
        state.edit_config(Edit::List(gate.clone(), ListEdit::Add));
        state.edit_config(Edit::List(gate, ListEdit::Set(1, "just lint".into())));
        let effects = state.save_config();
        let [
            Effect::Config {
                put: Some(sent), ..
            },
        ] = effects.as_slice()
        else {
            panic!("one config.put: {effects:?}");
        };
        assert!(
            sent.contains("gate = [\"just check\", \"just lint\"]"),
            "{sent}"
        );
        assert!(sent.contains("# owner notes") && sent.contains("[projects.app] # main repo"));
        assert!(sent.contains("[unknown_table]\nkept = 1"), "{sent}");
    }

    #[test]
    fn a_refused_save_keeps_the_draft_and_puts_each_reason_on_its_field() {
        let (mut state, mut feed) = loaded();
        let profile = Path::entry("projects", "app", "profile");
        state.edit_config(Edit::Text(profile.clone(), "nope".into()));
        let draft = state.config.draft.clone();
        state.save_config();
        answered(
            &mut state,
            &mut feed,
            None,
            Some("projects.app.profile: unknown profile \"nope\"\nprofile names must not be empty"),
        );
        assert_eq!(state.config.draft, draft);
        assert!(state.config.dirty());
        assert_eq!(
            state.config.reasons_at(&profile).collect::<Vec<_>>(),
            ["unknown profile \"nope\""]
        );
        assert!(
            state
                .config
                .reasons
                .iter()
                .any(|reason| reason.at.is_none())
        );
    }
}
