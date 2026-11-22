//! Settings › Configuration › Routing in the owner's words: which worker profile a dispatch
//! runs, in the engine's order (`corral-engine` `Config::resolve_profile`): a profile chosen at
//! dispatch, the task's own, the first `[[routing]]` rule that matches, the task's project's,
//! then the workspace default. A rule's keys are ANDed; `kind`, `size` and `difficulty` match
//! any listed word, and an unset or empty key restricts nothing, so a rule without keys is a
//! catch-all and every rule below it, and both defaults, are never reached. A matched rule whose
//! profile does not exist is refused, never passed over, so nothing here names a fallback for it.

use serde_json::{Map, Value as Json};
use toml_edit::{DocumentMut, Item, Value};

use super::document::{self, Path, Seg};
use super::editor::Editor;

/// The profile a rule or the workspace default names, as the engine finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A `[profiles]` entry of this draft, which the Profiles part opens.
    Configured(String),
    /// One of corral's built-in profiles, which the file need not list.
    BuiltIn(String),
    /// Neither, so corrald refuses to save the draft.
    Unknown(String),
    /// Not in this file, and this corrald does not report its built-in profiles.
    Unchecked(String),
    /// No profile, which corrald refuses as an empty name.
    Missing,
}

impl Target {
    fn of(name: Option<String>, known: &Known) -> Self {
        let Some(name) = name else {
            return Self::Missing;
        };
        if known.file.contains(&name) {
            Self::Configured(name)
        } else {
            match &known.built_in {
                Some(built_in) if built_in.contains(&name) => Self::BuiltIn(name),
                Some(_) => Self::Unknown(name),
                None => Self::Unchecked(name),
            }
        }
    }

    /// The profile's name, without what is known about it.
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Configured(name)
            | Self::BuiltIn(name)
            | Self::Unknown(name)
            | Self::Unchecked(name) => Some(name),
            Self::Missing => None,
        }
    }

    /// The profile in the owner's words: its name, marked when it is built in.
    pub fn words(&self) -> String {
        match self {
            Self::Configured(name) | Self::Unchecked(name) => name.clone(),
            Self::BuiltIn(name) => format!("{name} (built in)"),
            Self::Unknown(name) if name.is_empty() => "an empty profile name".into(),
            Self::Unknown(name) => name.clone(),
            Self::Missing => "no profile".into(),
        }
    }

    /// Why corrald refuses it, or what only corrald can tell.
    pub fn problem(&self) -> Option<String> {
        match self {
            Self::Configured(_) | Self::BuiltIn(_) => None,
            Self::Unknown(name) => Some(format!(
                "No profile is named “{name}”, so corrald refuses to save this."
            )),
            Self::Unchecked(name) => Some(format!(
                "“{name}” is not in this file; corrald checks whether it is built in when you save."
            )),
            Self::Missing => {
                Some("A rule must name a profile; corrald refuses to save one without.".into())
            }
        }
    }

    /// Whether corrald refuses a save naming it.
    pub fn refused(&self) -> bool {
        matches!(self, Self::Unknown(_) | Self::Missing)
    }
}

/// One rule as its row says it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub index: usize,
    /// What it matches: "Kind is bug or docs and project is site", or "Every task".
    pub matcher: String,
    /// It restricts nothing, so it matches every task that reaches it.
    pub catch_all: bool,
    /// The catch-all above it, which every task matches first.
    pub shadowed_by: Option<usize>,
    pub target: Target,
    /// What else corrald refuses in it.
    pub problems: Vec<String>,
}

/// How a row's note reads: an aside, a caution, or what corrald refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Aside,
    Caution,
    Refused,
}

impl Rule {
    /// What the row says under its match and profile, given the last refused save's reasons
    /// about it.
    pub fn notes(&self, reasons: usize) -> Vec<(Tone, String)> {
        let mut notes = Vec::new();
        match self.shadowed_by {
            Some(above) => notes.push((
                Tone::Caution,
                format!(
                    "Never reached: rule {} above matches every task.",
                    above + 1
                ),
            )),
            None if self.catch_all => notes.push((
                Tone::Caution,
                "Catch-all: every task that gets here matches it.".into(),
            )),
            None => {}
        }
        if let Some(problem) = self.target.problem() {
            let tone = if self.target.refused() {
                Tone::Refused
            } else {
                Tone::Aside
            };
            notes.push((tone, problem));
        }
        notes.extend(
            self.problems
                .iter()
                .map(|problem| (Tone::Refused, problem.clone())),
        );
        match reasons {
            0 => {}
            1 => notes.push((
                Tone::Refused,
                "corrald refused the last save over this rule; Edit shows where.".into(),
            )),
            n => notes.push((
                Tone::Refused,
                format!(
                    "corrald refused the last save over this rule ({n} reasons); Edit shows where."
                ),
            )),
        }
        notes
    }
}

/// What a task falls back to after the rules: the workspace's `[defaults] profile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fallback {
    /// This draft sets it.
    Set(Target),
    /// Unset, so corral's built-in default applies.
    BuiltIn(Target),
    /// Unset, and corral has no built-in default: a task nothing else routes is refused.
    Refused,
    /// Unset, and this corrald does not report its built-in default.
    Unreported,
}

impl Fallback {
    /// The default in the owner's words.
    pub fn words(&self) -> String {
        match self {
            Self::Set(target) => target.words(),
            Self::BuiltIn(target) => format!("unset · corral's default, {}", target.words()),
            Self::Refused => {
                "unset · none, so a task that nothing above routes is refused at dispatch".into()
            }
            Self::Unreported => {
                "unset · corral's built-in default (this corrald does not say which)".into()
            }
        }
    }

    pub fn target(&self) -> Option<&Target> {
        match self {
            Self::Set(target) | Self::BuiltIn(target) => Some(target),
            Self::Refused | Self::Unreported => None,
        }
    }
}

/// The routing part of the draft, read once per change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routing {
    pub rules: Vec<Rule>,
    /// The first rule that matches every task.
    pub catch_all: Option<usize>,
    /// The projects that name their own worker profile, in file order.
    pub projects: Vec<(String, Target)>,
    pub fallback: Fallback,
}

impl Default for Routing {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            catch_all: None,
            projects: Vec::new(),
            fallback: Fallback::Unreported,
        }
    }
}

/// The profile names the engine knows: the file's, and corral's built-in ones when reported.
struct Known {
    file: Vec<String>,
    built_in: Option<Vec<String>>,
}

/// A key as the engine reads it: unset, text, or a value of another type it refuses.
enum Text {
    Unset,
    Set(String),
    Wrong(String),
}

fn text(doc: &DocumentMut, path: &Path) -> Text {
    match document::get(doc, path).and_then(Item::as_value) {
        None => Text::Unset,
        Some(Value::String(text)) => Text::Set(text.value().clone()),
        Some(other) => Text::Wrong(document::shown(other)),
    }
}

impl Text {
    fn known(self) -> Option<String> {
        match self {
            Self::Set(text) => Some(text),
            Self::Unset | Self::Wrong(_) => None,
        }
    }
}

/// Reads the draft's routing against corral's built-in configuration (`built_in_defaults` of
/// the settings snapshot), when corrald reports it.
pub fn read(draft: &str, built_in: Option<&Map<String, Json>>) -> Routing {
    let Ok(doc) = document::parse(draft) else {
        return Routing::default();
    };
    let known = Known {
        file: document::names(&doc, "profiles"),
        built_in: built_in.and_then(|built_in| {
            Some(
                built_in
                    .get("profiles")?
                    .as_object()?
                    .keys()
                    .cloned()
                    .collect(),
            )
        }),
    };
    let projects = document::names(&doc, "projects");
    let mut catch_all = None;
    let rules = (0..document::rule_count(&doc))
        .map(|index| {
            let rule = rule(&doc, index, &known, &projects, catch_all);
            if rule.catch_all && catch_all.is_none() {
                catch_all = Some(index);
            }
            rule
        })
        .collect();
    let fallback = match text(&doc, &Path::of(&["defaults", "profile"])) {
        Text::Set(name) => Fallback::Set(Target::of(Some(name), &known)),
        Text::Wrong(shown) => Fallback::Set(Target::Unknown(shown)),
        Text::Unset => match built_in.map(|built_in| {
            built_in
                .get("defaults")
                .and_then(|defaults| defaults.get("profile"))
        }) {
            Some(Some(Json::String(name))) => {
                Fallback::BuiltIn(Target::of(Some(name.clone()), &known))
            }
            Some(Some(Json::Null)) => Fallback::Refused,
            _ => Fallback::Unreported,
        },
    };
    Routing {
        rules,
        catch_all,
        projects: projects
            .iter()
            .filter_map(|name| {
                let profile = text(&doc, &Path::entry("projects", name, "profile")).known()?;
                Some((name.clone(), Target::of(Some(profile), &known)))
            })
            .collect(),
        fallback,
    }
}

fn rule(
    doc: &DocumentMut,
    index: usize,
    known: &Known,
    projects: &[String],
    shadowed_by: Option<usize>,
) -> Rule {
    let mut problems = Vec::new();
    let mut clauses = Vec::new();
    for (key, noun) in [
        ("kind", "kind"),
        ("size", "size"),
        ("difficulty", "difficulty"),
    ] {
        let item = document::get(doc, &Path::rule(index, key));
        if let Some(Value::String(_)) = item.and_then(Item::as_value) {
            problems.push(format!(
                "The {noun} is one word, not a list; corrald refuses it."
            ));
        }
        let words = document::lines(item);
        if !words.is_empty() {
            clauses.push(format!("{noun} is {}", words.join(" or ")));
        }
    }
    match text(doc, &Path::rule(index, "title_regex")) {
        // An empty pattern matches every title, as no pattern does.
        Text::Set(pattern) if !pattern.is_empty() => {
            clauses.push(format!("title matches “{pattern}”"));
        }
        Text::Wrong(shown) => {
            problems.push(format!(
                "The title pattern {shown} is not text; corrald refuses it."
            ));
            clauses.push(format!("title matches {shown}"));
        }
        Text::Set(_) | Text::Unset => {}
    }
    match text(doc, &Path::rule(index, "project")) {
        Text::Set(project) => {
            if !projects.contains(&project) {
                problems.push(format!(
                    "No project is named “{project}”, so corrald refuses to save this."
                ));
            }
            clauses.push(format!("project is {project}"));
        }
        Text::Wrong(shown) => {
            problems.push(format!(
                "The project {shown} is not a name; corrald refuses it."
            ));
            clauses.push(format!("project is {shown}"));
        }
        Text::Unset => {}
    }
    let target = match text(doc, &Path::rule(index, "profile")) {
        Text::Set(name) => Target::of(Some(name), known),
        Text::Wrong(shown) => Target::Unknown(shown),
        Text::Unset => Target::Missing,
    };
    let catch_all = clauses.is_empty();
    Rule {
        index,
        matcher: if catch_all {
            "Every task".into()
        } else {
            sentence(&clauses.join(" and "))
        },
        catch_all,
        shadowed_by,
        target,
        problems,
    }
}

fn sentence(words: &str) -> String {
    let mut chars = words.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// One step of the order a worker's profile is chosen in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub title: &'static str,
    pub detail: String,
    /// A catch-all rule above means no task gets this far.
    pub unreached: bool,
}

/// The order a dispatch picks a worker's profile in, with this draft's facts.
pub fn steps(routing: &Routing) -> Vec<Step> {
    let unreached = routing
        .catch_all
        .map(|index| format!("Never reached: rule {} matches every task.", index + 1));
    let rules = match routing.rules.len() {
        0 => "No rules.".to_owned(),
        1 => "The one rule below, if it matches.".to_owned(),
        n => format!("The first of the {n} rules below that matches, top to bottom."),
    };
    let projects = if routing.projects.is_empty() {
        "No project names its own profile.".to_owned()
    } else {
        routing
            .projects
            .iter()
            .map(|(project, target)| format!("{project}: {}", target.words()))
            .collect::<Vec<_>>()
            .join(" · ")
    };
    vec![
        Step {
            title: "Dispatch choice",
            detail: "A profile picked when dispatching wins over everything below.".into(),
            unreached: false,
        },
        Step {
            title: "Task profile",
            detail: "The task's own profile, when it names one.".into(),
            unreached: false,
        },
        Step {
            title: "Routing rules",
            detail: rules,
            unreached: false,
        },
        Step {
            title: "Project profile",
            detail: unreached.clone().unwrap_or(projects),
            unreached: unreached.is_some(),
        },
        Step {
            title: "Workspace default",
            detail: unreached.clone().unwrap_or_else(|| {
                let words = routing.fallback.words();
                match routing.fallback.target().and_then(Target::problem) {
                    Some(problem) => format!("{words}. {problem}"),
                    None => words,
                }
            }),
            unreached: unreached.is_some(),
        },
    ]
}

/// Said under the order: what routing does not decide.
pub const WORKER_ONLY: &str =
    "Routing picks worker profiles only; a review runs each project's Reviewer.";

/// Said by Add rule: where a new rule goes and what it matches until narrowed.
pub const PREVIEW_NOTE: &str =
    "Preview saves routing rules, but sample dispatch does not apply them.";

pub const NEW_RULE: &str = "A new rule goes last and has no conditions, so it matches every task that reaches it until you add a kind, size, difficulty, title pattern or project.";

/// Said in an open catch-all rule's editor.
pub const CATCH_ALL: &str = "No conditions: this rule matches every task that reaches it. Add a kind, size, difficulty, title pattern or project to narrow it.";

/// Said when there are no rules.
pub const NO_RULES: &str = "No rules: a task without its own profile runs its project's profile, else the workspace default.";

/// The callout over the rules while one matches every task.
pub fn catch_all_notice(index: usize, below: usize) -> String {
    let rest = match below {
        0 => String::new(),
        1 => "the rule below it and ".into(),
        _ => "the rules below it and ".into(),
    };
    format!(
        "Rule {} matches every task, so {rest}the project and workspace profiles are never used for workers.",
        index + 1
    )
}

/// How many reasons of the last refused save are about rule `index`.
pub fn reasons(editor: &Editor, index: usize) -> usize {
    editor
        .reasons
        .iter()
        .filter(|reason| {
            reason.at.as_ref().is_some_and(|path| {
                matches!(path.0.as_slice(),
                    [Seg::Key(root), Seg::Index(at), ..] if root == "routing" && *at == index)
            })
        })
        .count()
}

/// The profile a new rule names: the file's first, else the workspace default's, else none
/// (which corrald refuses until one is typed).
pub fn new_rule_profile(draft: &str, routing: &Routing) -> String {
    document::parse(draft)
        .ok()
        .and_then(|doc| document::names(&doc, "profiles").into_iter().next())
        .or_else(|| {
            routing
                .fallback
                .target()
                .and_then(Target::name)
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rule with only empty keys was taken for a narrowing rule, so the rules below it read
    /// as reachable while the engine never got past it.
    #[test]
    fn a_rule_restricting_nothing_shadows_the_rules_and_defaults_below() {
        let draft = "[projects.site]\npath = \"site\"\n\n[[routing]]\nproject = \"site\"\nprofile = \"omp\"\n\n[[routing]]\nkind = []\ntitle_regex = \"\"\nprofile = \"omp\"\n\n[[routing]]\nkind = [\"bug\"]\nprofile = \"omp\"\n";
        let routing = read(draft, None);
        let shadowed: Vec<_> = routing.rules.iter().map(|rule| rule.shadowed_by).collect();
        assert_eq!(routing.catch_all, Some(1));
        assert_eq!(shadowed, [None, None, Some(1)]);
        assert!(steps(&routing)[3..].iter().all(|step| step.unreached));
    }
}
