//! The settings form model: which rows each section shows for a staged `Config`, how an edit changes the draft,
//! and how a saved draft is applied on top of the file as it is now (so concurrent edits to other keys survive).

use std::collections::BTreeMap;

use corral_core::{Config, FALLBACK_PROFILE, Project, ReviewOverride};
use corral_ops::HarnessReport;

use super::catalog;

use super::automation::{self, Key, Scope};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    General,
    Profiles,
    Harnesses,
    Projects,
    Artifacts,
    Automation,
}

pub(crate) const SECTIONS: [Section; 6] = [
    Section::General,
    Section::Profiles,
    Section::Harnesses,
    Section::Projects,
    Section::Artifacts,
    Section::Automation,
];

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Profiles => "Profiles",
            Section::Harnesses => "Harnesses",
            Section::Projects => "Projects",
            Section::Artifacts => "Artifacts",
            Section::Automation => "Automation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Field {
    SlotLimit,
    DefaultProfile,
    PmAgent,
    HerdrSession,
    Mouse,
    /// A project's header row (`x` removes the project).
    Project(String),
    ProjectPath(String),
    ProjectBase(String),
    ProjectProfile(String),
    ProjectCap(String),
    AddProject,
    Glob(usize),
    AddGlob,
    MaxBytes,
    /// A profile's row in the Profiles list, or the editor's header.
    Profile(String),
    AddProfile,
    ProfileHarness(String),
    ProfileModel(String),
    ProfileEffort(String),
    ProfileArgs(String),
    /// A harness's header row (install state).
    Harness(String),
    HarnessCommand(String),
    HarnessFlags(String),
    /// `[review] autopilot`.
    Autopilot,
    /// A review-policy key of one scope (Automation section).
    Review(Scope, Key),
}

/// How a row is edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Free text; `optional` means empty = unset (inherit).
    Text {
        optional: bool,
    },
    /// Whole number; `optional` means empty = unset (inherit).
    Number {
        optional: bool,
    },
    Toggle,
    /// Pills; `None` is the "inherit" choice of an optional pick.
    Pick(Vec<Option<String>>),
    /// One entry of a string list (add / remove / reorder).
    ListItem,
    /// The "+ add" row of a list or of the project table.
    Add,
    /// A project's header row (`x` removes the project).
    Group,
    /// A list entry that opens an editor (`⏎`): a profile in the Profiles list.
    Entry,
    /// A value chosen in the model picker popup (`⏎`).
    Picker,
    /// A heading with a status value and no remove button.
    Header,
    /// Read-only facts.
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub field: Field,
    pub label: String,
    pub kind: Kind,
    /// Staged value as text; empty when unset.
    pub value: String,
    pub help: &'static str,
    /// What applies while the value is unset (inherited or built-in default).
    pub inherited: Option<String>,
    /// Dotted config key that validation messages start with; empty for rows with no key.
    pub key: String,
    /// Pill labels drawn dimmed and struck out (harnesses that are not installed).
    pub muted: Vec<String>,
}

impl Row {
    pub(super) fn new(field: Field, label: impl Into<String>, kind: Kind, value: String) -> Row {
        Row {
            field,
            label: label.into(),
            kind,
            value,
            help: "",
            inherited: None,
            key: String::new(),
            muted: Vec::new(),
        }
    }

    pub(super) fn help(mut self, help: &'static str) -> Row {
        self.help = help;
        self
    }

    pub(super) fn key(mut self, key: impl Into<String>) -> Row {
        self.key = key.into();
        self
    }

    pub(super) fn muted(mut self, labels: Vec<String>) -> Row {
        self.muted = labels;
        self
    }

    pub(super) fn inherits(mut self, what: impl Into<String>) -> Row {
        self.inherited = Some(what.into());
        self
    }

    /// Whether the validation message `err` is about this row.
    pub fn owns(&self, err: &str) -> bool {
        if self.key.is_empty() {
            return false;
        }
        let Some(rest) = err.strip_prefix(&self.key) else {
            return false;
        };
        let boundary = rest.is_empty() || rest.starts_with([' ', ':', '.']);
        match self.kind {
            Kind::ListItem => boundary && err.contains(&self.value),
            // Whole-entry rows own `<key>: ...` only; their field rows own `<key>.<field> ...`.
            Kind::Entry | Kind::Header | Kind::Info => rest.is_empty() || rest.starts_with(':'),
            _ => boundary,
        }
    }
}

/// Configured profile names, or the fallback profile when none is configured.
pub(super) fn profile_names(cfg: &Config) -> Vec<String> {
    let mut names = cfg.profile_names();
    if names.is_empty() {
        names.push(FALLBACK_PROFILE.into());
    }
    names
}

fn profile_choices(cfg: &Config, current: Option<&str>, optional: bool) -> Vec<Option<String>> {
    let mut names = profile_names(cfg);
    if let Some(c) = current
        && !names.iter().any(|n| n == c)
    {
        names.push(c.to_string());
    }
    let mut out: Vec<Option<String>> = Vec::new();
    if optional {
        out.push(None);
    }
    out.extend(names.into_iter().map(Some));
    out
}

fn global_profile(cfg: &Config) -> String {
    match &cfg.defaults.profile {
        Some(p) => format!("{p} (from General)"),
        None => format!("{FALLBACK_PROFILE} (built-in default)"),
    }
}

/// The rows of `section` for the staged `cfg` (the Profiles list, not a profile's editor). `herdr_env` is
/// `$CORRAL_HERDR_SESSION`, which overrides the configured session; `report` is harness detection, if done.
pub(crate) fn rows(
    section: Section,
    cfg: &Config,
    herdr_env: Option<&str>,
    report: Option<&HarnessReport>,
) -> Vec<Row> {
    match section {
        Section::General => general_rows(cfg, herdr_env),
        Section::Profiles => catalog::profile_list(cfg),
        Section::Harnesses => catalog::harness_rows(cfg, report),
        Section::Projects => project_rows(cfg),
        Section::Artifacts => artifact_rows(cfg),
        Section::Automation => automation::rows(cfg),
    }
}

fn general_rows(cfg: &Config, herdr_env: Option<&str>) -> Vec<Row> {
    let session = cfg.herdr.session.clone().unwrap_or_default();
    let session_effective = match herdr_env {
        Some(env) => format!("{env} (from $CORRAL_HERDR_SESSION, which overrides this)"),
        None => "the herdr session corral runs in".into(),
    };
    vec![
        Row::new(
            Field::SlotLimit,
            "Global slot limit",
            Kind::Number { optional: false },
            cfg.limits.running.to_string(),
        )
        .help("Runs that may hold a slot at once (Running + Needs me) across all projects.")
        .key("limits.running"),
        Row::new(
            Field::DefaultProfile,
            "Default profile",
            Kind::Pick(profile_choices(cfg, cfg.defaults.profile.as_deref(), true)),
            cfg.defaults.profile.clone().unwrap_or_default(),
        )
        .help("Profile for runs whose task and project name none.")
        .inherits(format!("{FALLBACK_PROFILE} (built-in default)"))
        .key("defaults.profile"),
        Row::new(
            Field::PmAgent,
            "PM agent profile",
            Kind::Pick(profile_choices(cfg, Some(&cfg.pm.agent), false)),
            cfg.pm.agent.clone(),
        )
        .help("Profile launched in the workspace PM pane (P on the board).")
        .key("pm.agent"),
        Row::new(
            Field::HerdrSession,
            "herdr session",
            Kind::Text { optional: true },
            session,
        )
        .help("Named herdr session to target (herdr --session <name>); empty = the caller's.")
        .inherits(session_effective)
        .key("herdr.session"),
        Row::new(
            Field::Mouse,
            "Mouse",
            Kind::Toggle,
            if cfg.ui.mouse { "on" } else { "off" }.into(),
        )
        .help("Capture clicks, wheel and drag ([ui] mouse); --no-mouse and CORRAL_MOUSE=0 also turn it off.")
        .key("ui.mouse"),
    ]
}

fn project_rows(cfg: &Config) -> Vec<Row> {
    let mut out = Vec::new();
    for (k, p) in &cfg.projects {
        out.push(
            Row::new(
                Field::Project(k.clone()),
                k.clone(),
                Kind::Group,
                String::new(),
            )
            .key(format!("projects.{k}")),
        );
        out.push(
            Row::new(
                Field::ProjectPath(k.clone()),
                "Path",
                Kind::Text { optional: false },
                p.path.clone(),
            )
            .help("Repository directory, relative to the workspace root or absolute.")
            .key(format!("projects.{k}.path")),
        );
        out.push(
            Row::new(
                Field::ProjectBase(k.clone()),
                "Base branch",
                Kind::Text { optional: true },
                p.base.clone().unwrap_or_default(),
            )
            .help("Branch new runs start from.")
            .inherits("the repo's current branch at dispatch time")
            .key(format!("projects.{k}.base")),
        );
        out.push(
            Row::new(
                Field::ProjectProfile(k.clone()),
                "Default profile",
                Kind::Pick(profile_choices(cfg, p.default_profile.as_deref(), true)),
                p.default_profile.clone().unwrap_or_default(),
            )
            .help("Profile for this project's runs when the task names none.")
            .inherits(global_profile(cfg))
            .key(format!("projects.{k}.default_profile")),
        );
        out.push(
            Row::new(
                Field::ProjectCap(k.clone()),
                "Slot cap",
                Kind::Number { optional: true },
                cfg.limits
                    .per_project
                    .get(k)
                    .map(ToString::to_string)
                    .unwrap_or_default(),
            )
            .help("At most this many of the project's runs hold a slot at once.")
            .inherits(format!(
                "no cap below the global limit ({})",
                cfg.limits.running
            ))
            .key(format!("limits.per_project.{k}")),
        );
    }
    out.push(Row::new(
        Field::AddProject,
        "+ add project",
        Kind::Add,
        String::new(),
    ));
    out
}

fn artifact_rows(cfg: &Config) -> Vec<Row> {
    let mut out: Vec<Row> = cfg
        .artifacts
        .globs
        .iter()
        .enumerate()
        .map(|(i, g)| {
            Row::new(
                Field::Glob(i),
                format!("Glob {}", i + 1),
                Kind::ListItem,
                g.clone(),
            )
            .help("Scanned in the run worktree at `corral done` for new or changed files.")
            .key("artifacts.globs")
        })
        .collect();
    out.push(Row::new(
        Field::AddGlob,
        "+ add glob",
        Kind::Add,
        String::new(),
    ));
    out.push(
        Row::new(
            Field::MaxBytes,
            "Max bytes",
            Kind::Number { optional: false },
            cfg.artifacts.max_bytes.to_string(),
        )
        .help("Larger files are skipped by the scan and refused by `corral artifact add`.")
        .key("artifacts.max_bytes"),
    );
    out
}

pub(super) fn number<T: std::str::FromStr>(text: &str) -> Result<T, String> {
    text.trim()
        .replace('_', "")
        .parse()
        .map_err(|_| format!("{:?} is not a whole number", text.trim()))
}

pub(super) fn opt(text: &str) -> Option<String> {
    let t = text.trim();
    (!t.is_empty()).then(|| t.to_string())
}

pub(crate) fn set_value(cfg: &mut Config, field: &Field, text: &str) -> Result<(), String> {
    match field {
        Field::SlotLimit => cfg.limits.running = number(text)?,
        Field::DefaultProfile => cfg.defaults.profile = opt(text),
        Field::PmAgent => cfg.pm.agent = text.trim().to_string(),
        Field::HerdrSession => cfg.herdr.session = opt(text),
        Field::Mouse => cfg.ui.mouse = text == "on",
        Field::ProjectPath(k) => project(cfg, k)?.path = text.trim().to_string(),
        Field::ProjectBase(k) => project(cfg, k)?.base = opt(text),
        Field::ProjectProfile(k) => project(cfg, k)?.default_profile = opt(text),
        Field::ProjectCap(k) => match opt(text) {
            None => {
                cfg.limits.per_project.remove(k);
            }
            Some(t) => {
                cfg.limits.per_project.insert(k.clone(), number(&t)?);
            }
        },
        Field::Glob(i) => match (opt(text), cfg.artifacts.globs.get_mut(*i)) {
            (Some(t), Some(g)) => *g = t,
            (None, Some(_)) => {
                cfg.artifacts.globs.remove(*i);
            }
            (_, None) => return Err(format!("glob {} no longer exists", i + 1)),
        },
        Field::AddGlob => {
            if let Some(t) = opt(text) {
                cfg.artifacts.globs.push(t);
            }
        }
        Field::MaxBytes => cfg.artifacts.max_bytes = number(text)?,
        Field::AddProject => return add_project(cfg, text),
        Field::Project(_) => return Err("a project header has no value".into()),
        Field::AddProfile => return catalog::add_profile(cfg, text, None),
        Field::ProfileHarness(_)
        | Field::ProfileModel(_)
        | Field::ProfileEffort(_)
        | Field::ProfileArgs(_)
        | Field::HarnessCommand(_) => return catalog::set_value(cfg, field, text),
        Field::Profile(_) | Field::Harness(_) | Field::HarnessFlags(_) => {
            return Err("this row has no value of its own".into());
        }
        Field::Autopilot | Field::Review(..) => return automation::set_value(cfg, field, text),
    }
    Ok(())
}

fn project<'a>(cfg: &'a mut Config, key: &str) -> Result<&'a mut Project, String> {
    cfg.projects
        .get_mut(key)
        .ok_or_else(|| format!("project {key} no longer exists"))
}

fn add_project(cfg: &mut Config, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("type a project name".into());
    }
    if cfg.projects.contains_key(name) {
        return Err(format!("project {name} already exists"));
    }
    cfg.projects.insert(
        name.to_string(),
        Project {
            path: name.to_string(),
            default_profile: None,
            base: None,
            review: ReviewOverride::default(),
        },
    );
    Ok(())
}

pub(crate) fn remove_project(cfg: &mut Config, key: &str) {
    cfg.projects.remove(key);
    cfg.limits.per_project.remove(key);
}

pub(crate) fn move_glob(cfg: &mut Config, i: usize, up: bool) -> Option<usize> {
    let globs = &mut cfg.artifacts.globs;
    let j = if up { i.checked_sub(1)? } else { i + 1 };
    if j >= globs.len() || i >= globs.len() {
        return None;
    }
    globs.swap(i, j);
    Some(j)
}

pub(super) fn take<T: PartialEq + Clone>(base: &T, draft: &T, target: &mut T) {
    if base != draft {
        *target = draft.clone();
    }
}

/// Entry-level `take` for a keyed table: entries removed from the draft are removed, changed or new ones are
/// written whole, untouched ones keep what is on disk.
fn take_entries<V: PartialEq + Clone>(
    base: &BTreeMap<String, V>,
    draft: &BTreeMap<String, V>,
    target: &mut BTreeMap<String, V>,
) {
    for k in base.keys() {
        if !draft.contains_key(k) {
            target.remove(k);
        }
    }
    for (k, d) in draft {
        if base.get(k) != Some(d) {
            target.insert(k.clone(), d.clone());
        }
    }
}

/// Applies what changed from `base` to `draft` (only the keys this screen edits) onto `target`, the config as
/// it is on disk now, so keys someone else changed since the screen opened are kept.
pub(crate) fn apply_changes(base: &Config, draft: &Config, target: &mut Config) {
    take(
        &base.limits.running,
        &draft.limits.running,
        &mut target.limits.running,
    );
    take(
        &base.defaults.profile,
        &draft.defaults.profile,
        &mut target.defaults.profile,
    );
    take(&base.pm.agent, &draft.pm.agent, &mut target.pm.agent);
    take(
        &base.herdr.session,
        &draft.herdr.session,
        &mut target.herdr.session,
    );
    take(&base.ui.mouse, &draft.ui.mouse, &mut target.ui.mouse);
    take_entries(&base.profiles, &draft.profiles, &mut target.profiles);
    take_entries(&base.harnesses, &draft.harnesses, &mut target.harnesses);
    take_entries(&base.agents, &draft.agents, &mut target.agents);
    take(
        &base.artifacts.globs,
        &draft.artifacts.globs,
        &mut target.artifacts.globs,
    );
    take(
        &base.artifacts.max_bytes,
        &draft.artifacts.max_bytes,
        &mut target.artifacts.max_bytes,
    );
    automation::apply(&base.review, &draft.review, &mut target.review);
    for k in base.projects.keys() {
        if !draft.projects.contains_key(k) {
            target.projects.remove(k);
        }
    }
    for (k, d) in &draft.projects {
        match (base.projects.get(k), target.projects.get_mut(k)) {
            (Some(b), Some(t)) => {
                take(&b.path, &d.path, &mut t.path);
                take(&b.base, &d.base, &mut t.base);
                take(
                    &b.default_profile,
                    &d.default_profile,
                    &mut t.default_profile,
                );
                automation::apply(&b.review, &d.review, &mut t.review);
            }
            (Some(b), None) if b == d => {}
            _ => {
                target.projects.insert(k.clone(), d.clone());
            }
        }
    }
    let caps = base
        .limits
        .per_project
        .keys()
        .chain(draft.limits.per_project.keys());
    for k in caps.cloned().collect::<Vec<_>>() {
        let (b, d) = (
            base.limits.per_project.get(&k),
            draft.limits.per_project.get(&k),
        );
        if b != d {
            match d {
                Some(n) => target.limits.per_project.insert(k, *n),
                None => target.limits.per_project.remove(&k),
            };
        }
    }
}
