//! Settings › Configuration's editor: `corral.toml` as `config.get` returned it, and the draft
//! the owner changes. Every change edits the draft document in place (see `document`), the
//! form and the plain-text view alike; Save sends the whole draft in one `config.put`. corrald
//! checks it with the engine and either replaces the file or refuses with its problems, each
//! naming the key it is about or, for a syntax error, its line and column; a refusal leaves the
//! draft as it was, each problem shown on its field or at the top. The settings topic states the
//! file's revision, so an edit made elsewhere is noticed, and a file on disk that does not parse.

use std::collections::{BTreeMap, BTreeSet};

use corral_protocol::config::ConfigDocument;
use corral_types::{ConfigProblem, Refusal};
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
/// or none when it names no key, what corrald says about it, and for a syntax error its line
/// and column (1-based).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    pub at: Option<Path>,
    pub line: String,
    pub position: Option<(usize, usize)>,
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
    DuplicateProfile {
        from: String,
        name: String,
    },
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
    /// On a narrow layout, whether to show the selected profile or its list.
    pub profile_focus: bool,
    /// The profile list's filter, independent of the document.
    pub profile_query: String,
    /// The one expanded routing editor, in the draft's current order.
    pub open_rule: Option<usize>,
    /// What was typed into number and word fields, shown as typed while the draft holds the
    /// value it means.
    pub raw: BTreeMap<Path, String>,
    /// Number fields whose text is not a whole number; the draft keeps their last value.
    pub invalid: BTreeSet<Path>,
    /// The field that takes the keyboard focus when it shows: the one the last edit added (a
    /// list's new line, a new entry's first key, a new rule's first key), or the one that took
    /// a removed one's place (the next line, rule or entry), so the focus stays where the owner
    /// was working rather than leaving with the removed field's controls.
    pub added: Option<Path>,
    /// The revision of `saved`, as corrald stated it.
    pub revision: Option<String>,
    /// The revision the last accepted save replaced: a settings snapshot still stating it has
    /// not caught up with the save yet, rather than showing an edit made elsewhere.
    pub replaced: Option<String>,
    /// The problem of the file on disk the editor last opened the File part for.
    pub disk: Option<ConfigProblem>,
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

    /// Whether corral.toml changed since the editor read it: the settings topic states another
    /// revision than the one the draft was read at (or the one a save of ours replaced).
    pub fn changed_elsewhere(&self, feed: &Feed) -> bool {
        let Some(now) = feed.settings.as_ref().and_then(|s| s.revision.as_ref()) else {
            return false;
        };
        self.revision.as_ref().is_some_and(|read| read != now)
            && self.replaced.as_ref() != Some(now)
    }

    fn apply(&mut self, edit: &Edit) -> Result<(), String> {
        if let Edit::File(text) = edit {
            self.draft.clone_from(text);
            self.raw.clear();
            self.invalid.clear();
            self.added = None;
            self.open_rule = None;
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
                let count = document::lines(document::get(&doc, path)).len();
                let focused = match edit {
                    ListEdit::Add => count.checked_sub(1),
                    ListEdit::Remove(index) => (count > 0).then(|| (*index).min(count - 1)),
                    _ => None,
                };
                if let Some(at) = focused {
                    let mut line = path.clone();
                    line.0.push(Seg::Index(at));
                    self.added = Some(line);
                }
            }
            Edit::Pair(path, name, value) => {
                let names = |doc: &DocumentMut| {
                    document::pairs(document::get(doc, path))
                        .into_iter()
                        .map(|(name, _)| name)
                        .collect::<Vec<_>>()
                };
                let at = names(&doc).iter().position(|held| held == name);
                document::set_pair(&mut doc, path, name, value.as_deref());
                if let (None, Some(at)) = (value, at) {
                    // The value of the pair now in its place, else the new name's field (the
                    // pairs' own path).
                    let left = names(&doc);
                    let mut focus = path.clone();
                    if let Some(next) = left.get(at.min(left.len().saturating_sub(1))) {
                        focus.0.push(Seg::Key(next.clone()));
                    }
                    self.added = Some(focus);
                }
            }
            Edit::AddEntry(part, name) => {
                let name = name.trim();
                let Some(root) = part.root().filter(|_| !name.is_empty()) else {
                    return Ok(());
                };
                document::add_entry(&mut doc, root, name);
                self.chosen.insert(*part, name.to_owned());
                self.added = Some(Path::entry(root, name, entry_first_key(*part)));
                if *part == Part::Profiles {
                    self.profile_focus = true;
                }
            }
            Edit::DuplicateProfile { from, name } => {
                let name = name.trim();
                if !name.is_empty() && document::duplicate_entry(&mut doc, "profiles", from, name) {
                    self.chosen.insert(Part::Profiles, name.to_owned());
                    self.profile_focus = true;
                    self.added = Some(Path::entry("profiles", name, "harness"));
                }
            }
            Edit::RemoveEntry(part, name) => {
                if let Some(root) = part.root() {
                    let removed = Path::of(&[root, name]);
                    self.reasons.retain(|reason| {
                        !reason
                            .at
                            .as_ref()
                            .is_some_and(|path| path.0.starts_with(&removed.0))
                    });
                    if *part == Part::Profiles {
                        self.profile_focus = false;
                    }
                    document::remove_entry(&mut doc, root, name);
                    self.chosen.remove(part);
                    if let Some(next) = document::names(&doc, root).into_iter().next() {
                        self.added = Some(Path::entry(root, &next, entry_first_key(*part)));
                        self.chosen.insert(*part, next);
                    }
                }
            }
            Edit::Rules(edit) => {
                let before = document::rule_count(&doc);
                document::edit_rules(&mut doc, edit);
                self.rule_layout(edit, before, document::rule_count(&doc));
            }
            Edit::File(_) => {}
        }
        self.draft = doc.to_string();
        Ok(())
    }

    /// Rule editors are addressed by position; drafts and error anchors must follow the rule.
    fn rule_layout(&mut self, edit: &RuleEdit, before: usize, after: usize) {
        let (removed, moved) = match *edit {
            RuleEdit::Add(_) if after > before => {
                self.open_rule = after.checked_sub(1);
                self.added = self.open_rule.map(|index| Path::rule(index, "kind"));
                return;
            }
            RuleEdit::Remove(index) if after < before => (Some(index), None),
            RuleEdit::Move(from, up) if from < before => {
                let to = if up {
                    from.checked_sub(1)
                } else {
                    from.checked_add(1)
                };
                let Some(to) = to.filter(|to| *to < before) else {
                    return;
                };
                (None, Some((from, to)))
            }
            _ => return,
        };
        let index = |at| match (removed, moved) {
            (Some(removed), _) if at == removed => None,
            (Some(removed), _) if at > removed => Some(at - 1),
            (_, Some((from, to))) if at == from => Some(to),
            (_, Some((from, to))) if at == to => Some(from),
            _ => Some(at),
        };
        self.open_rule = self.open_rule.and_then(index);
        let relocate = |path: &mut Path| {
            if let [Seg::Key(root), Seg::Index(at), ..] = path.0.as_mut_slice()
                && root == "routing"
            {
                let Some(next) = index(*at) else { return false };
                *at = next;
            }
            true
        };
        self.raw = std::mem::take(&mut self.raw)
            .into_iter()
            .filter_map(|(mut path, value)| relocate(&mut path).then_some((path, value)))
            .collect();
        self.invalid = std::mem::take(&mut self.invalid)
            .into_iter()
            .filter_map(|mut path| relocate(&mut path).then_some(path))
            .collect();
        self.reasons
            .retain_mut(|reason| reason.at.as_mut().is_none_or(relocate));
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

    /// Places each problem of a refused save: a syntax error at the top with its line and
    /// column, a keyed one on the field (or entry) its key names, any other at the top. A
    /// refusal without problems is said at the top as corrald worded it.
    fn refuse(&mut self, refusal: &Refusal) {
        let Some(problems) = &refusal.problems else {
            self.reasons = vec![Reason {
                at: None,
                line: refusal.reason.clone(),
                position: None,
            }];
            return;
        };
        let anchors = document::parse(&self.draft)
            .map(|doc| super::form::anchors(&doc))
            .unwrap_or_default();
        self.reasons = problems
            .iter()
            .map(|problem| place(problem, &anchors))
            .collect();
        self.reasons.sort_by_key(|reason| reason.position.is_none());
    }
}

/// Where `problem` shows: at its position, else on the longest anchor its key is or lies under
/// (what lies below the anchor kept in the words), else at the top with its key.
fn place(problem: &ConfigProblem, anchors: &[Path]) -> Reason {
    if let (Some(line), Some(column)) = (problem.line, problem.column) {
        return Reason {
            at: None,
            line: format!("Line {line}, column {column}: {}", problem.message),
            position: Some((line, column)),
        };
    }
    let Some(key) = problem.key.as_deref().filter(|key| !key.is_empty()) else {
        return Reason {
            at: None,
            line: problem.message.clone(),
            position: None,
        };
    };
    let found = anchors
        .iter()
        .map(|anchor| (anchor, anchor.to_string()))
        .filter_map(|(anchor, name)| {
            let rest = key.strip_prefix(name.as_str())?;
            (rest.is_empty() || rest.starts_with(['.', '['])).then(|| (anchor, rest, name.len()))
        })
        .max_by_key(|(_, _, len)| *len);
    match found {
        Some((anchor, rest, _)) => Reason {
            at: Some(anchor.clone()),
            line: match rest.trim_start_matches('.') {
                "" => problem.message.clone(),
                below => format!("{below}: {}", problem.message),
            },
            position: None,
        },
        None => Reason {
            at: None,
            line: format!("{key}: {}", problem.message),
            position: None,
        },
    }
}

/// An entry's first key, where the focus goes when the entry shows after an edit.
fn entry_first_key(part: Part) -> &'static str {
    match part {
        Part::Projects => "path",
        Part::Profiles => "harness",
        _ => "command",
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

    /// The owner's choice once the file changed elsewhere: drop the draft and read the file as
    /// it is now.
    pub fn reread_config(&mut self) -> Vec<Effect> {
        if self.config.waiting.is_some() {
            return Vec::new();
        }
        self.discard_config();
        self.request_config(None)
    }

    /// Opens the plain-text view once for each problem the file on disk newly has, since the
    /// form cannot read a file that does not parse; forgets it once the file parses again.
    pub fn disk_problem(&mut self, problem: Option<ConfigProblem>) -> Vec<Effect> {
        if problem.is_some() && problem != self.config.disk {
            self.config.part = Part::File;
        }
        self.config.disk = problem;
        Vec::new()
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
        if part != self.config.part {
            self.config.added = None;
            self.config.profile_focus = false;
        }
        self.config.part = part;
        Vec::new()
    }

    pub fn choose_entry(&mut self, part: Part, name: String) -> Vec<Effect> {
        self.config.added = None;
        self.config.chosen.insert(part, name);
        if part == Part::Profiles {
            self.config.part = part;
            self.config.profile_focus = true;
        }
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
            self.config.profile_focus = part == Part::Profiles;
            if let [Seg::Key(root), Seg::Index(index), ..] = path.0.as_slice()
                && root == "routing"
            {
                self.config.open_rule = Some(*index);
            }
            self.config.added = Some(path.clone());
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
        self.config.added = None;
        self.config.profile_focus = false;
        self.config.open_rule = None;
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
        if let Some(ConfigDocument { toml, revision }) = operation.decode::<ConfigDocument>() {
            let fresh = !editor.dirty();
            if waiting.save || fresh {
                editor.draft.clone_from(&toml);
                editor.raw.clear();
                editor.invalid.clear();
            }
            editor.replaced = if waiting.save {
                editor.revision.take()
            } else {
                None
            };
            editor.revision = revision;
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
            editor.refuse(refusal);
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
/// Said over the editor once corral.toml changed since the draft was read; it warns that a
/// re-read drops edits only when there are some.
pub fn changed_elsewhere_notice(dirty: bool) -> &'static str {
    if dirty {
        "corral.toml changed outside this editor since it was read. Read again to take the file as it is now (your unsaved edits are dropped), or Save to write this draft over it."
    } else {
        "corral.toml changed outside this editor since it was read. Read again to take the file as it is now."
    }
}

/// Said over the editor while the file on disk does not parse, and where.
pub fn on_disk(problem: &ConfigProblem) -> String {
    match (problem.line, problem.column) {
        (Some(line), Some(column)) => format!(
            "corral.toml on disk is not valid at line {line}, column {column}: {}. Fix it in File and Save.",
            problem.message
        ),
        _ => format!("corral.toml on disk is not valid: {problem}. Fix it in File and Save."),
    }
}

#[cfg(test)]
mod tests {
    use corral_protocol::rows::SettingsSnapshot;

    use super::*;
    use crate::model::daemon::Operation;

    const FILE: &str = "# owner notes\n[limits]\nrunning = 4\n\n[projects.app] # main repo\npath = \"app\"\ngate = [\"just check\"]\n\n[unknown_table]\nkept = 1\n";

    #[test]
    fn duplicate_profile_preserves_unshown_fields_and_leaves_source_in_place() {
        let original = "[profiles.source]\nharness = 'pi' # keep me\nfallback = ['provider/model']\n\n[profiles.other]\nharness = 'omp'\n";
        let mut editor = Editor {
            draft: original.to_owned(),
            ..Editor::default()
        };
        editor
            .apply(&Edit::DuplicateProfile {
                from: "source".into(),
                name: "copy".into(),
            })
            .unwrap();
        assert!(editor.draft.starts_with(original));
        let doc = document::parse(&editor.draft).unwrap();
        assert_eq!(
            document::lines(document::get(
                &doc,
                &Path::entry("profiles", "copy", "fallback")
            )),
            ["provider/model"]
        );
        assert_eq!(
            editor.chosen.get(&Part::Profiles).map(String::as_str),
            Some("copy")
        );
        assert!(editor.profile_focus);
        let duplicated = editor.draft.clone();
        editor
            .apply(&Edit::DuplicateProfile {
                from: "source".into(),
                name: "other".into(),
            })
            .unwrap();
        assert_eq!(editor.draft, duplicated);
    }

    #[test]
    fn moving_and_removing_rules_reindex_open_editor_raw_values_and_refusals() {
        let mut editor = Editor {
            open_rule: Some(0),
            ..Editor::default()
        };
        let raw = |at| Path::rule(at, "kind");
        editor.raw.insert(raw(0), "ui,".into());
        editor.raw.insert(raw(1), "docs".into());
        editor.reasons.push(Reason {
            at: Some(Path::rule(1, "profile")),
            line: String::new(),
            position: None,
        });
        editor.rule_layout(&RuleEdit::Move(0, false), 2, 2);
        assert_eq!(editor.open_rule, Some(1));
        assert_eq!(editor.raw.get(&raw(1)).map(String::as_str), Some("ui,"));
        assert_eq!(editor.reasons[0].at, Some(Path::rule(0, "profile")));
        editor.rule_layout(&RuleEdit::Remove(0), 2, 1);
        assert_eq!(editor.open_rule, Some(0));
        assert_eq!(editor.raw.get(&raw(0)).map(String::as_str), Some("ui,"));
        assert!(editor.reasons.is_empty());
    }

    fn problem(key: Option<&str>, message: &str, at: Option<(usize, usize)>) -> ConfigProblem {
        ConfigProblem {
            key: key.map(str::to_owned),
            message: message.into(),
            line: at.map(|(line, _)| line),
            column: at.map(|(_, column)| column),
        }
    }

    /// corrald answers the request in flight: the file at a revision, or a refusal.
    fn answered(state: &mut State, feed: &mut Feed, value: Option<&str>, refusal: Option<Refusal>) {
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
                value: value.map(
                    |toml| serde_json::json!({ "toml": toml, "revision": toml.len().to_string() }),
                ),
                error: refusal
                    .as_ref()
                    .map(|refusal| crate::model::daemon::ClientError {
                        code: "refused".into(),
                        message: refusal.reason.clone(),
                    }),
                refusal,
            },
        );
        state.settle_config(feed);
    }

    fn loaded() -> (State, Feed) {
        let mut state = State::default();
        let mut feed = Feed::default();
        state.open_config();
        answered(&mut state, &mut feed, Some(FILE), None);
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

    /// The keys came out of the refusal's words, so a problem whose message did not start with
    /// its key fell to the top, and a syntax error lost the place it names.
    #[test]
    fn a_keyed_problem_lands_on_its_field_and_a_syntax_error_at_the_top_with_its_position() {
        let (mut state, mut feed) = loaded();
        let profile = Path::entry("projects", "app", "profile");
        state.edit_config(Edit::Text(profile.clone(), "nope".into()));
        let draft = state.config.draft.clone();
        state.save_config();
        let problems = vec![
            problem(
                Some("projects.app.profile"),
                "unknown profile \"nope\"",
                None,
            ),
            problem(Some("profiles"), "names must not be empty", None),
            problem(None, "expected `]`", Some((3, 7))),
        ];
        let refusal = Refusal {
            reason: "configuration problems".into(),
            next: Vec::new(),
            problems: Some(problems),
        };
        answered(&mut state, &mut feed, None, Some(refusal));
        assert_eq!(state.config.draft, draft);
        assert!(state.config.dirty());
        assert_eq!(state.config.reasons_at(&profile).count(), 1);
        let top: Vec<_> = state
            .config
            .reasons
            .iter()
            .filter(|reason| reason.at.is_none())
            .map(|reason| reason.position)
            .collect();
        assert_eq!(top, [Some((3, 7)), None], "the syntax error leads the top");
    }

    /// An edit made on disk went unseen, and a save silently wrote the old draft over it.
    #[test]
    fn a_changed_revision_offers_read_again_and_keeps_the_draft_until_chosen() {
        let (mut state, mut feed) = loaded();
        let read = state.config.revision.clone();
        feed.settings = Some(SettingsSnapshot {
            revision: read,
            ..SettingsSnapshot::default()
        });
        assert!(!state.config.changed_elsewhere(&feed));
        let running = Path::of(&["limits", "running"]);
        state.edit_config(Edit::Typed(running, Typed::Number, "6".into()));
        let draft = state.config.draft.clone();
        if let Some(settings) = &mut feed.settings {
            settings.revision = Some("elsewhere".into());
        }
        assert!(state.config.changed_elsewhere(&feed));
        state.settle_config(&feed);
        assert_eq!(
            state.config.draft, draft,
            "the draft stays until the owner chooses"
        );
        assert!(
            changed_elsewhere_notice(state.config.dirty()).contains("dropped"),
            "unsaved edits are named as dropped"
        );
        let effects = state.reread_config();
        assert!(matches!(
            effects.as_slice(),
            [Effect::Config { put: None, .. }]
        ));
        let elsewhere = "[limits]\nrunning = 2\n";
        answered(&mut state, &mut feed, Some(elsewhere), None);
        assert_eq!(state.config.draft, elsewhere);
        assert!(!state.config.dirty());
        assert!(
            !changed_elsewhere_notice(state.config.dirty()).contains("dropped"),
            "no edits, nothing to drop"
        );
    }

    /// Removing a line or a name and value pair took the keyboard focus with its Remove button,
    /// so the next Tab started at the top of the app.
    #[test]
    fn removing_a_line_or_a_pair_hands_the_focus_to_what_takes_its_place() {
        let (mut state, _) = loaded();
        let gate = Path::entry("projects", "app", "gate");
        state.edit_config(Edit::List(gate.clone(), ListEdit::Add));
        state.edit_config(Edit::List(gate.clone(), ListEdit::Remove(0)));
        let mut first = gate;
        first.0.push(Seg::Index(0));
        assert_eq!(state.config.added, Some(first));

        let env = Path::entry("projects", "app", "env");
        for name in ["A", "B"] {
            state.edit_config(Edit::Pair(env.clone(), name.into(), Some("1".into())));
        }
        state.edit_config(Edit::Pair(env.clone(), "A".into(), None));
        let mut next = env.clone();
        next.0.push(Seg::Key("B".into()));
        assert_eq!(state.config.added, Some(next));
        state.edit_config(Edit::Pair(env.clone(), "B".into(), None));
        assert_eq!(state.config.added, Some(env), "the new name's field");
    }
}
