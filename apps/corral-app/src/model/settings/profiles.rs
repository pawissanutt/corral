//! Settings › Configuration › Profiles: what the profile list says of each profile, which
//! profiles a search keeps, the name a new or duplicated profile takes, the values a model or
//! effort field offers, and what still names a profile the owner is about to remove. Read from
//! the draft only: corrald decides whether a saved file holds together.

use toml_edit::{DocumentMut, Item};

use super::document::{self, Path};
use super::form::{Values, place};

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

pub fn summary(values: &Values, name: &str) -> Summary {
    let key = |key: &str| set(values, &Path::entry(ROOT, name, key));
    Summary {
        harness: key("harness"),
        model: key("model"),
        effort: key("effort"),
        default: set(values, &Path::of(&["defaults", "profile"])).as_deref() == Some(name),
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
            let shown = summary(values, name);
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

    pub fn action(&self) -> &'static str {
        match self {
            Naming::New => "Add profile",
            Naming::Duplicate(_) => "Duplicate profile",
        }
    }

    /// The name the field starts with: empty for a new profile, a free `<name>-copy` for a
    /// duplicate.
    pub fn initial(&self, names: &[String]) -> String {
        match self {
            Naming::New => String::new(),
            Naming::Duplicate(from) => copy_name(names, from),
        }
    }
}

pub const NAME_LABEL: &str = "Profile name";

/// A free name for a copy of `from`: `from-copy`, then `from-copy-2` and on.
pub fn copy_name(names: &[String], from: &str) -> String {
    let taken = |name: &str| names.iter().any(|held| held == name);
    let first = format!("{from}-copy");
    if !taken(&first) {
        return first;
    }
    (2..)
        .map(|n| format!("{from}-copy-{n}"))
        .find(|name| !taken(name))
        .unwrap_or(first)
}

/// Why `typed` cannot name a new profile; none when it can. An empty name says nothing: the
/// button waits for one.
pub fn name_problem(names: &[String], typed: &str) -> Option<String> {
    let name = typed.trim();
    if name.is_empty() {
        return None;
    }
    names
        .iter()
        .any(|held| held == name)
        .then(|| format!("A profile named {name} already exists."))
}

/// Whether `typed` can name a new profile.
pub fn name_ok(names: &[String], typed: &str) -> bool {
    !typed.trim().is_empty() && name_problem(names, typed).is_none()
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

    /// A taken name would have made the new profile's table land on an existing one.
    #[test]
    fn a_taken_name_is_refused_and_a_copy_finds_a_free_one() {
        let mut held = names();
        assert!(!name_ok(&held, " deep "));
        assert!(name_ok(&held, "quick"));
        held.push("deep-copy".into());
        assert_eq!(copy_name(&held, "deep"), "deep-copy-2");
    }
}
