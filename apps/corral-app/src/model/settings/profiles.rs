//! Settings › Configuration › Profiles: what the profile list says of each profile, which
//! profiles a search keeps, the name a new or duplicated profile takes and what it would land
//! on, the values a model or effort field offers, and what still names a profile the owner is
//! about to remove. Read from the draft and corral's reported built-ins only: corrald decides
//! whether a saved file holds together.

use serde_json::{Map, Value as Json};
use toml_edit::{DocumentMut, Item};

use super::document::{self, Path, Seg};
use super::form::{Field, Values, place};
use crate::model::daemon::Feed;

const ROOT: &str = "profiles";
/// How many configured values a model or effort field offers at once.
const OFFERED: usize = 8;

/// One profile as its list row shows it: what it runs, and whether the workspace defaults to it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Summary {
    pub harness: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub default: bool,
}

fn set(values: &Values, path: &Path) -> Option<String> {
    Some(values.text(path)).filter(|text| !text.trim().is_empty())
}

/// `default` is the profile the workspace defaults to, as [`Taken`] reads it.
pub fn summary(values: &Values, name: &str, default: Option<&str>) -> Summary {
    let key = |key: &str| set(values, &Path::entry(ROOT, name, key));
    Summary {
        harness: key("harness"),
        model: key("model"),
        effort: key("effort"),
        default: default == Some(name),
    }
}

/// The key a profile's field edits: harness, model, effort or args.
pub fn key_of(field: &Field) -> Option<&str> {
    match field.path.0.last() {
        Some(Seg::Key(key)) => Some(key.as_str()),
        _ => None,
    }
}

impl Summary {
    /// The model in full, or that the harness picks it.
    pub fn model_words(&self) -> String {
        self.model
            .clone()
            .unwrap_or_else(|| "harness default model".into())
    }

    /// The harness and effort: "claude · effort high".
    pub fn runs_words(&self) -> String {
        let harness = self.harness.as_deref().unwrap_or("harness unset");
        match &self.effort {
            Some(effort) => format!("{harness} · effort {effort}"),
            None => format!("{harness} · default effort"),
        }
    }
}

/// Said beside the profile the workspace defaults to.
pub const DEFAULT_MARK: &str = "workspace default";

/// The profiles whose name, harness, model or effort contain every word of `query`, ignoring
/// case; all of them for an empty query.
pub fn filtered(names: &[String], values: &Values, query: &str) -> Vec<String> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    names
        .iter()
        .filter(|name| {
            if words.is_empty() {
                return true;
            }
            let shown = summary(values, name, None);
            let haystack = [
                Some(name.as_str()),
                shown.harness.as_deref(),
                shown.model.as_deref(),
                shown.effort.as_deref(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
            words.iter().all(|word| haystack.contains(word.as_str()))
        })
        .cloned()
        .collect()
}

pub const SEARCH: &str = "Search profiles";
pub const NEW: &str = "New profile";
pub const DUPLICATE: &str = "Duplicate";
pub const BACK: &str = "Profiles";
pub const MORE: &str = "More";
pub const EMPTY: &str = "No profiles defined in this file; corral's built-in profiles still apply.";
pub const NOTE: &str = "A profile is what a worker or reviewer runs: a harness, its model and effort, and any extra arguments.";

pub fn no_match(query: &str) -> String {
    format!("No profile matches “{}”.", query.trim())
}

/// What the name panel makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Naming {
    New,
    /// A copy of the named profile, its whole table.
    Duplicate(String),
}

impl Naming {
    pub fn title(&self) -> String {
        match self {
            Naming::New => "Name the new profile".into(),
            Naming::Duplicate(from) => format!("Name the copy of {from}"),
        }
    }

    /// The button's words for the name as checked: a name that overrides a built-in profile
    /// says so on the button that makes it.
    pub fn action(&self, check: &Check) -> &'static str {
        match (self, check) {
            (_, Check::Overrides(_)) => "Override built-in profile",
            (Naming::New, _) => "Add profile",
            (Naming::Duplicate(_), _) => "Duplicate profile",
        }
    }

    /// The name the field starts with: empty for a new profile, a free `<name>-copy` for a
    /// duplicate.
    pub fn initial(&self, taken: &Taken) -> String {
        match self {
            Naming::New => String::new(),
            Naming::Duplicate(from) => copy_name(taken, from),
        }
    }
}

pub const NAME_LABEL: &str = "Profile name";

/// What a new profile's name may land on: every key under the draft's `[profiles]`, a table
/// or not, and each profile corral reports built in; with the profile the workspace defaults
/// to, the draft's `defaults.profile` else corral's.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Taken {
    tables: Vec<String>,
    values: Vec<String>,
    built_in: Vec<String>,
    pub default: Option<String>,
    /// `profiles` holds a value rather than a table, so nothing can be added under it.
    blocked: bool,
}

impl Taken {
    pub fn read(draft: &str, feed: &Feed) -> Self {
        let built_in = feed.settings.as_ref();
        let built_in = built_in.and_then(|settings| settings.built_in_defaults.as_ref());
        Self::of(document::parse(draft).ok().as_ref(), built_in)
    }

    /// `built_in` is the daemon's `built_in_defaults`.
    pub fn of(doc: Option<&DocumentMut>, built_in: Option<&Map<String, Json>>) -> Self {
        let mut taken = Self::default();
        match doc.and_then(|doc| doc.get(ROOT)).map(Item::as_table_like) {
            Some(Some(table)) => {
                for (key, item) in table.iter() {
                    let held = if item.is_table_like() {
                        &mut taken.tables
                    } else {
                        &mut taken.values
                    };
                    held.push(key.to_owned());
                }
            }
            Some(None) => taken.blocked = true,
            None => {}
        }
        let table = |name: &str| built_in?.get(name)?.as_object();
        taken.built_in = table(ROOT)
            .map(|profiles| profiles.keys().cloned().collect())
            .unwrap_or_default();
        let named = doc.and_then(|doc| document::get(doc, &Path::of(&["defaults", "profile"])));
        taken.default = match named {
            Some(item) => item.as_str().map(str::to_owned),
            None => table("defaults")
                .and_then(|defaults| defaults.get("profile")?.as_str())
                .map(str::to_owned),
        }
        .filter(|name| !name.trim().is_empty());
        taken
    }

    fn holds(&self, name: &str) -> bool {
        [&self.tables, &self.values, &self.built_in]
            .iter()
            .any(|names| names.iter().any(|held| held == name))
    }

    /// What `typed` would land on as a new profile's name.
    pub fn check(&self, typed: &str) -> Check {
        let name = typed.trim();
        let has = |names: &[String]| names.iter().any(|held| held == name);
        if name.is_empty() {
            Check::Empty
        } else if self.blocked {
            Check::Refused(
                "profiles in this file holds a value, not a table, so no profile can be added here. Fix it in File.".into(),
            )
        } else if has(&self.tables) {
            Check::Refused(format!("A profile named {name} already exists."))
        } else if has(&self.values) {
            Check::Refused(format!(
                "profiles.{name} is already set in this file, but not as a profile table. Change or remove it in File."
            ))
        } else if has(&self.built_in) {
            let default = if self.default.as_deref() == Some(name) {
                ", including as the workspace default"
            } else {
                ""
            };
            Check::Overrides(format!(
                "{name} is one of corral's built-in profiles. A profile of that name in this file overrides it everywhere {name} is used{default}."
            ))
        } else {
            Check::Free
        }
    }
}

/// What the name panel says of the name typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    /// Nothing typed: the button waits for a name.
    Empty,
    Free,
    /// Why the name cannot be used.
    Refused(String),
    /// The name is a built-in profile's, which a profile here overrides: the warning, which
    /// the button's words confirm.
    Overrides(String),
}

impl Check {
    pub fn ready(&self) -> bool {
        matches!(self, Check::Free | Check::Overrides(_))
    }
}

/// A free name for a copy of `from`: `from-copy`, then `from-copy-2` and on, past every key
/// of the draft and every built-in profile.
pub fn copy_name(taken: &Taken, from: &str) -> String {
    let first = format!("{from}-copy");
    if !taken.holds(&first) {
        return first;
    }
    (2..)
        .map(|n| format!("{from}-copy-{n}"))
        .find(|name| !taken.holds(name))
        .unwrap_or(first)
}

/// A key of the draft that names a profile, with where it shows in owner words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub path: Path,
    pub place: String,
}

/// Every key of the draft the engine reads as a profile name and that names `name`: the
/// workspace's worker profile, each project's worker profile and reviewer, and each routing
/// rule's profile.
pub fn references(doc: &DocumentMut, name: &str) -> Vec<Reference> {
    let mut paths = vec![Path::of(&["defaults", "profile"])];
    for project in document::names(doc, "projects") {
        for key in ["profile", "reviewer"] {
            paths.push(Path::entry("projects", &project, key));
        }
    }
    paths.extend((0..document::rule_count(doc)).map(|index| Path::rule(index, "profile")));
    paths
        .into_iter()
        .filter(|path| document::get(doc, path).and_then(Item::as_str) == Some(name))
        .map(|path| Reference {
            place: place(&path),
            path,
        })
        .collect()
}

/// The words over Remove: what removing changes, and what in the file still names the profile.
/// A built-in profile of the same name may stay behind it, so they do not predict a refusal.
pub fn removal(name: &str, references: &[Reference]) -> String {
    let unchanged =
        "References are unchanged; update them if this profile will no longer be available.";
    match references.len() {
        0 => format!(
            "Remove {name} from the draft? Nothing else in this file names it. Save sends the change; Discard brings it back."
        ),
        1 => format!("1 setting in this file names {name}. {unchanged}"),
        n => format!("{n} settings in this file name {name}. {unchanged}"),
    }
}

pub const REMOVE: &str = "Remove profile";
pub const REMOVE_ANYWAY: &str = "Remove anyway";
pub const REMOVE_MENU: &str = "Remove profile…";
pub const CANCEL: &str = "Cancel";

/// The values of `key` (model or effort) the other profiles running the same harness as
/// `profile` set, in list order, each once.
pub fn offers(values: &Values, names: &[String], profile: &str, key: &str) -> Vec<String> {
    let harness = |name: &str| set(values, &Path::entry(ROOT, name, "harness"));
    let own = harness(profile);
    let mut offered: Vec<String> = Vec::new();
    for name in names.iter().filter(|name| name.as_str() != profile) {
        if harness(name) != own {
            continue;
        }
        if let Some(value) = set(values, &Path::entry(ROOT, name, key))
            && !offered.contains(&value)
        {
            offered.push(value);
        }
    }
    offered.truncate(OFFERED);
    offered
}

/// The caption before a model or effort field's offered values.
pub fn offers_caption(key: &str) -> &'static str {
    match key {
        "model" => "Configured model suggestions",
        _ => "Configured effort suggestions",
    }
}

/// The button that unsets a model or effort, so the harness picks it.
pub const HARNESS_DEFAULT: &str = "Use harness default";

pub fn field_note(key: &str) -> &'static str {
    match key {
        "model" => "as the harness's CLI names it; any text",
        _ => "as the harness's CLI takes it; any text",
    }
}

/// Advanced's toggle: the arguments it holds, folded or open.
pub fn advanced(open: bool, count: usize) -> String {
    let held = match count {
        0 => "none".to_owned(),
        1 => "1 argument".to_owned(),
        n => format!("{n} arguments"),
    };
    if open {
        format!("Hide arguments ({held})")
    } else {
        format!("Advanced arguments ({held})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "[defaults]\nprofile = \"fast\"\n\n[profiles.fast]\nharness = \"claude\"\nmodel = \"claude-sonnet-5-5\"\n\n[profiles.deep]\nharness = \"claude\"\nmodel = \"claude-opus-5-5\"\neffort = \"high\"\n\n[profiles.local]\nharness = \"pi\"\nmodel = \"qwen\"\n\n[profiles.twin]\nharness = \"claude\"\nmodel = \"claude-opus-5-5\"\n\n[projects.site]\npath = \"site\"\nreviewer = \"fast\"\n\n[projects.app]\npath = \"app\"\nprofile = \"deep\"\n\n[[routing]]\nprofile = \"fast\"\n\n[[routing]]\nprofile = \"deep\"\n";

    fn names() -> Vec<String> {
        ["fast", "deep", "local", "twin"].map(String::from).to_vec()
    }

    /// Removing a profile left a project's reviewer or a routing rule naming it unannounced, so
    /// the refusal came only at Save.
    #[test]
    fn every_key_the_engine_reads_as_a_profile_is_a_reference() {
        let doc = document::parse(FILE).unwrap();
        let found: Vec<String> = references(&doc, "fast")
            .into_iter()
            .map(|reference| reference.path.to_string())
            .collect();
        assert_eq!(
            found,
            [
                "defaults.profile",
                "projects.site.reviewer",
                "routing[0].profile"
            ]
        );
        assert!(references(&doc, "local").is_empty());
    }

    /// A model offered from another harness's profile runs nowhere on this one.
    #[test]
    fn offered_models_come_from_other_profiles_of_the_same_harness_once() {
        let values = Values::of(FILE);
        assert_eq!(
            offers(&values, &names(), "fast", "model"),
            ["claude-opus-5-5"]
        );
        assert_eq!(
            offers(&values, &names(), "local", "model"),
            Vec::<String>::new()
        );
    }

    fn built_in() -> Map<String, Json> {
        let json =
            r#"{"defaults": {"profile": "default"}, "profiles": {"default": {}, "deep-copy": {}}}"#;
        serde_json::from_str(json).unwrap()
    }

    /// A taken name would have made the new profile's table land on an existing one, and a
    /// scalar under `[profiles]` was missed because only tables were listed, so Add did nothing.
    #[test]
    fn every_key_under_profiles_is_refused_and_a_copy_passes_them_and_the_built_ins() {
        let file = format!("[profiles]\nstray = \"x\"\n\n{FILE}\n[profiles.\"deep-copy-2\"]\n");
        let doc = document::parse(&file).unwrap();
        let taken = Taken::of(Some(&doc), Some(&built_in()));
        assert!(matches!(taken.check(" deep "), Check::Refused(_)));
        assert!(matches!(taken.check("stray"), Check::Refused(words) if words.contains("File")));
        assert_eq!(taken.check("quick"), Check::Free);
        assert_eq!(taken.check(" "), Check::Empty);
        assert_eq!(copy_name(&taken, "deep"), "deep-copy-3");
        let blocked = document::parse("profiles = \"x\"\n").unwrap();
        let blocked = Taken::of(Some(&blocked), None);
        assert!(matches!(blocked.check("quick"), Check::Refused(_)));
    }

    /// New and Duplicate silently shadowed a built-in profile, the implicit workspace default
    /// among them.
    #[test]
    fn a_built_in_name_warns_and_names_the_workspace_default_it_overrides() {
        let doc = document::parse("[profiles.fast]\nharness = \"claude\"\n").unwrap();
        let taken = Taken::of(Some(&doc), Some(&built_in()));
        assert_eq!(taken.default.as_deref(), Some("default"));
        let check = taken.check("default");
        assert!(matches!(&check, Check::Overrides(words) if words.contains("workspace default")));
        assert!(check.ready());
        assert_eq!(Naming::New.action(&check), "Override built-in profile");
        assert_eq!(Naming::New.action(&taken.check("quick")), "Add profile");
        // The draft's own default wins over corral's.
        let doc = document::parse(FILE).unwrap();
        assert_eq!(
            Taken::of(Some(&doc), Some(&built_in())).default.as_deref(),
            Some("fast")
        );
        let values = Values::of(FILE);
        assert!(summary(&values, "fast", Some("fast")).default);
        assert!(!summary(&values, "deep", Some("fast")).default);
    }
}
