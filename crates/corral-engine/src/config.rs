//! The simple-engine configuration contract. Relative project paths are resolved
//! against the workspace root.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use corral_types::{
    ConfigProblem, TaskDifficulty,
    pricing::{Price, pricing_problems},
};
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};

use crate::error::refuse;
use crate::model::Task;

mod extension;
mod profile;
mod project;
pub use extension::{Extension, ExtensionTool, Slot};
pub use profile::{ModelSelector, Profile};
pub use project::{EffectiveProject, Project};

mod wrap;
pub(crate) use wrap::render_wrap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub limits: Limits,
    pub defaults: Defaults,
    pub runner: RunnerConfig,
    pub sync: SyncConfig,
    pub artifacts: ArtifactConfig,
    #[serde(deserialize_with = "deserialize_harnesses")]
    pub harnesses: BTreeMap<String, Harness>,
    #[serde(deserialize_with = "deserialize_profiles")]
    pub profiles: BTreeMap<String, Profile>,
    pub projects: BTreeMap<String, Project>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, Extension>,
    pub routing: Vec<RoutingRule>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub pricing: BTreeMap<String, Price>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daemon: Option<DaemonConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            defaults: Defaults::default(),
            runner: RunnerConfig::default(),
            sync: SyncConfig::default(),
            artifacts: ArtifactConfig::default(),
            harnesses: builtin_harnesses(),
            profiles: builtin_profiles(),
            projects: BTreeMap::new(),
            extensions: BTreeMap::new(),
            routing: Vec::new(),
            pricing: BTreeMap::new(),
            daemon: None,
        }
    }
}

/// Listener settings are consumed by corrald-next, not by the engine.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DaemonConfig {
    pub listen: Option<std::net::SocketAddr>,
    pub web_origin: Vec<String>,
    pub web_dir: Option<PathBuf>,
    pub web_endpoint: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub running: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { running: 4 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    pub base: Option<String>,
    pub branch_prefix: String,
    pub max_rounds: u32,
    pub profile: Option<String>,
    pub auto_dispatch: bool,
    pub auto_land: bool,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            base: None,
            branch_prefix: "corral/".into(),
            max_rounds: 2,
            profile: Some("omp-sol".into()),
            auto_dispatch: false,
            auto_land: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RunnerConfig {
    pub mode: String,
    pub herdr_session: Option<String>,
    pub wrap: Option<String>,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            mode: "herdr".into(),
            herdr_session: None,
            wrap: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SyncConfig {
    pub quiet_after_secs: u64,
    pub watch_secs: u64,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            quiet_after_secs: 120,
            watch_secs: 5,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ArtifactConfig {
    pub globs: Vec<String>,
    pub max_bytes: u64,
}

impl Default for ArtifactConfig {
    fn default() -> Self {
        Self {
            globs: vec!["docs/**/records/*".into(), "**/*report*.html".into()],
            max_bytes: 20_000_000,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Harness {
    pub command: String,
    pub model_flag: Option<String>,
    pub effort_flag: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RoutingRule {
    pub kind: Vec<String>,
    pub size: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub difficulty: Vec<TaskDifficulty>,
    pub title_regex: Option<String>,
    pub project: Option<String>,
    pub profile: String,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading configuration {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parsing configuration {}", path.display()))
    }

    pub fn parse(text: &str) -> std::result::Result<Self, ConfigProblem> {
        let deserializer = toml::de::Deserializer::parse(text)
            .map_err(|error| parse_problem(text, &error, None))?;
        serde_path_to_error::deserialize(deserializer).map_err(|error| {
            let mut key = error.path().to_string();
            if key == "." {
                key.clear();
            }
            let inner = error.inner();
            // The TOML deserializer can reject table keys before visiting
            // them. For that error its span points at the first unknown key.
            if inner.message().starts_with("unexpected keys in table:")
                && let Some(field) = inner.span().and_then(|span| text.get(span))
            {
                if !key.is_empty() {
                    key.push('.');
                }
                key.push_str(field.trim_matches(['\'', '"']));
            }
            parse_problem(text, inner, (!key.is_empty()).then_some(key))
        })
    }

    /// Parse and validate the same candidate for the CLI and the daemon.
    pub fn check(root: &Path, text: &str) -> std::result::Result<Self, Vec<ConfigProblem>> {
        let candidate = Self::parse(text).map_err(|problem| vec![problem])?;
        let problems = candidate.validate(root);
        if problems.is_empty() {
            Ok(candidate)
        } else {
            Err(problems)
        }
    }

    /// Collect independent problems rather than stopping at the first bad reference.
    pub fn validate(&self, root: &Path) -> Vec<ConfigProblem> {
        let mut problems: Vec<_> = pricing_problems(&self.pricing)
            .into_iter()
            .map(|(key, message)| problem(key, message))
            .collect();
        if self.limits.running == 0 {
            problems.push(problem("limits.running", "must be greater than zero"));
        }
        if self.sync.watch_secs == 0 {
            problems.push(problem("sync.watch_secs", "must be greater than zero"));
        }
        if !matches!(self.runner.mode.as_str(), "process" | "herdr") {
            problems.push(problem("runner.mode", "must be process or herdr"));
        }
        wrap::check(
            self.runner.wrap.as_deref(),
            || "runner.wrap".into(),
            &mut problems,
        );
        if self
            .defaults
            .base
            .as_ref()
            .is_some_and(|base| base.trim().is_empty())
        {
            problems.push(problem("defaults.base", "must not be empty"));
        }
        if let Some(profile) = &self.defaults.profile {
            self.check_profile(profile, "defaults.profile", &mut problems);
        }
        for pattern in &self.artifacts.globs {
            if let Err(error) = glob::Pattern::new(pattern) {
                problems.push(problem(
                    "artifacts.globs",
                    format!("invalid glob {pattern:?}: {error}"),
                ));
            }
        }
        for (name, harness) in &self.harnesses {
            if name.trim().is_empty() {
                problems.push(problem("harnesses", "names must not be empty"));
            }
            if harness.command.trim().is_empty() {
                problems.push(problem(
                    format!("harnesses.{name}.command"),
                    "must not be empty",
                ));
            } else if harness.command.contains('\0') {
                problems.push(problem(format!("harnesses.{name}.command"), "contains NUL"));
            }
            for (key, flag) in [
                ("model_flag", &harness.model_flag),
                ("effort_flag", &harness.effort_flag),
            ] {
                if flag
                    .as_ref()
                    .is_some_and(|f| f.trim().is_empty() || f.contains('\0'))
                {
                    problems.push(problem(
                        format!("harnesses.{name}.{key}"),
                        "must be a nonempty argument",
                    ));
                }
            }
        }
        for (name, profile) in &self.profiles {
            if name.trim().is_empty() {
                problems.push(problem("profiles", "names must not be empty"));
            }
            profile.validate(name, self.harnesses.get(&profile.harness), &mut problems);
        }
        for (name, project) in &self.projects {
            wrap::check(
                project.wrap.as_deref(),
                || format!("projects.{name}.wrap"),
                &mut problems,
            );
            if name.trim().is_empty() {
                problems.push(problem("projects", "names must not be empty"));
            } else if reserved_project_name(name) {
                problems.push(problem(
                    format!("projects.{name}"),
                    "project names must not look like task IDs or contain ':'",
                ));
            }
            if project.path.as_os_str().is_empty() {
                problems.push(problem(
                    format!("projects.{name}.path"),
                    "must not be empty",
                ));
            } else {
                let path = root.join(&project.path);
                if !path.is_dir() {
                    problems.push(problem(
                        format!("projects.{name}.path"),
                        format!("is not a directory: {}", path.display()),
                    ));
                }
            }
            if project
                .base
                .as_ref()
                .is_some_and(|base| base.trim().is_empty())
            {
                problems.push(problem(
                    format!("projects.{name}.base"),
                    "must not be empty",
                ));
            }
            if !corral_facts::lenses::valid_path(project.lens_dir()) {
                problems.push(problem(
                    format!("projects.{name}.lenses"),
                    "must be a repository-relative directory without empty, . or .. segments",
                ));
            }
            if project.running == Some(0) {
                problems.push(problem(
                    format!("projects.{name}.running"),
                    "must be greater than zero",
                ));
            }
            for (key, profile) in [
                ("profile", &project.profile),
                ("reviewer", &project.reviewer),
            ] {
                if let Some(profile) = profile {
                    self.check_profile(profile, format!("projects.{name}.{key}"), &mut problems);
                }
            }
            if project
                .gate
                .iter()
                .any(|line| line.trim().is_empty() || line.contains('\0'))
            {
                problems.push(problem(
                    format!("projects.{name}.gate"),
                    "contains an empty line or NUL",
                ));
            }
            for (key, value) in &project.env {
                if !valid_env_key(key) || value.contains('\0') {
                    problems.push(problem(
                        format!("projects.{name}.env"),
                        format!("invalid value or name {key:?}"),
                    ));
                }
            }
        }
        extension::validate(self, &mut problems);
        for (index, rule) in self.routing.iter().enumerate() {
            self.check_profile(
                &rule.profile,
                format!("routing[{index}].profile"),
                &mut problems,
            );
            if let Some(project) = &rule.project
                && !self.projects.contains_key(project)
            {
                problems.push(problem(
                    format!("routing[{index}].project"),
                    format!("unknown project {project:?}"),
                ));
            }
            if let Some(pattern) = &rule.title_regex
                && let Err(error) = Regex::new(pattern)
            {
                problems.push(problem(
                    format!("routing[{index}].title_regex"),
                    error.to_string(),
                ));
            }
        }
        problems
    }

    fn check_profile(
        &self,
        name: &str,
        field: impl Into<String>,
        problems: &mut Vec<ConfigProblem>,
    ) {
        if !self.profiles.contains_key(name) {
            problems.push(problem(field, format!("unknown profile {name:?}")));
        }
    }

    pub fn effective_project(&self, name: &str) -> Result<EffectiveProject> {
        let project = self.projects.get(name).ok_or_else(|| {
            refuse(
                format!("unknown project {name:?}; update .corral-next/corral.toml"),
                vec![],
            )
        })?;
        Ok(self.resolve_project(project))
    }

    /// Settings inherited by a project that has no overrides.
    pub fn project_defaults(&self) -> EffectiveProject {
        self.resolve_project(&Project::default())
    }

    fn resolve_project(&self, project: &Project) -> EffectiveProject {
        EffectiveProject {
            path: project.path.clone(),
            base: project.base.clone().or_else(|| self.defaults.base.clone()),
            gate: project.gate.clone(),
            reviewer: project.reviewer.clone(),
            profile: project
                .profile
                .clone()
                .or_else(|| self.defaults.profile.clone()),
            wrap: project.wrap.clone().or_else(|| self.runner.wrap.clone()),
            running: project.running,
            env: project.env.clone(),
            auto_dispatch: project.auto_dispatch.unwrap_or(self.defaults.auto_dispatch),
            auto_land: project.auto_land.unwrap_or(self.defaults.auto_land),
            max_rounds: project.max_rounds.unwrap_or(self.defaults.max_rounds),
            branch_prefix: project
                .branch_prefix
                .clone()
                .unwrap_or_else(|| self.defaults.branch_prefix.clone()),
        }
    }

    /// The pipeline's review-round limit for a task, including unassigned tasks.
    pub fn task_max_rounds(&self, task: &Task) -> u32 {
        task.project
            .as_ref()
            .and_then(|name| self.projects.get(name))
            .and_then(|project| project.max_rounds)
            .unwrap_or(self.defaults.max_rounds)
    }

    pub fn resolve_profile(&self, task: &Task, flag: Option<&str>) -> Result<String> {
        if let Some(name) = flag.or(task.profile.as_deref()) {
            return self.require_profile(name);
        }
        for (index, rule) in self.routing.iter().enumerate() {
            if !matches_values(&rule.kind, task.kind.as_deref())
                || !matches_values(&rule.size, task.size.as_deref())
                || (!rule.difficulty.is_empty()
                    && !task
                        .difficulty
                        .is_some_and(|value| rule.difficulty.contains(&value)))
                || rule
                    .project
                    .as_deref()
                    .is_some_and(|p| Some(p) != task.project.as_deref())
            {
                continue;
            }
            if let Some(pattern) = &rule.title_regex {
                let regex = Regex::new(pattern)
                    .with_context(|| format!("compiling routing[{index}].title_regex"))?;
                if !regex.is_match(&task.title) {
                    continue;
                }
            }
            return self.require_profile(&rule.profile);
        }
        let project_profile = task
            .project
            .as_deref()
            .map(|name| {
                self.projects.get(name).ok_or_else(|| {
                    refuse(
                        format!("unknown project {name:?}; update .corral-next/corral.toml"),
                        vec![],
                    )
                })
            })
            .transpose()?
            .and_then(|project| project.profile.as_deref());
        let name = project_profile
            .or(self.defaults.profile.as_deref())
            .ok_or_else(|| {
                refuse(
                    "no worker profile resolves for this task; set --profile",
                    self.profiles
                        .keys()
                        .map(|name| {
                            crate::Next::run(crate::Op::Dispatch {
                                id: task.id.clone(),
                                profile: Some(name.clone()),
                                base: None,
                            })
                        })
                        .collect(),
                )
            })?;
        self.require_profile(name)
    }

    pub(crate) fn require_profile(&self, name: &str) -> Result<String> {
        if !self.profiles.contains_key(name) {
            return Err(refuse(
                format!(
                    "unknown profile {name:?}; update .corral-next/corral.toml or set --profile"
                ),
                vec![],
            ));
        }
        Ok(name.to_owned())
    }
}

fn problem(key: impl Into<String>, message: impl Into<String>) -> ConfigProblem {
    ConfigProblem {
        key: Some(key.into()),
        message: message.into(),
        line: None,
        column: None,
    }
}

fn parse_problem(text: &str, error: &toml::de::Error, key: Option<String>) -> ConfigProblem {
    let (line, column) = error.span().map_or((None, None), |span| {
        let before = &text[..text.floor_char_boundary(span.start)];
        let line = before.bytes().filter(|&b| b == b'\n').count() + 1;
        let column = before
            .rsplit('\n')
            .next()
            .map_or(1, |s| s.chars().count() + 1);
        (Some(line), Some(column))
    });
    ConfigProblem {
        key,
        message: error.message().to_owned(),
        line,
        column,
    }
}

fn matches_values(values: &[String], value: Option<&str>) -> bool {
    values.is_empty() || value.is_some_and(|value| values.iter().any(|item| item == value))
}

fn builtin_harnesses() -> BTreeMap<String, Harness> {
    [
        ("omo", "omo -n {name} {model_args} {prompt_ref}"),
        ("omp", "omp {model_args} {prompt_ref}"),
        ("claude", "claude {model_args} {prompt_ref}"),
        (
            "pi",
            "corral-pi run {model_args} --prompt-file {prompt_file} --corral {corral}",
        ),
        ("generic", "cat {prompt_file}"),
    ]
    .into_iter()
    .map(|(name, command)| {
        let has_model = name != "generic";
        (
            name.to_owned(),
            Harness {
                command: command.into(),
                model_flag: has_model.then(|| "--model".into()),
                effort_flag: has_model.then(|| "--effort".into()),
            },
        )
    })
    .collect()
}

fn builtin_profiles() -> BTreeMap<String, Profile> {
    let mut profiles: BTreeMap<_, _> = ["omo", "omp", "claude", "pi", "generic"]
        .into_iter()
        .map(|name| {
            (
                name.to_owned(),
                Profile {
                    harness: name.into(),
                    ..Profile::default()
                },
            )
        })
        .collect();
    profiles.insert(
        "omp-sol".into(),
        Profile {
            harness: "omp".into(),
            model: Some("gpt-6.2-sol".into()),
            effort: Some("medium".into()),
            ..Profile::default()
        },
    );
    profiles
}

fn deserialize_harnesses<'de, D: Deserializer<'de>>(
    d: D,
) -> std::result::Result<BTreeMap<String, Harness>, D::Error> {
    let supplied = BTreeMap::<String, Harness>::deserialize(d)?;
    let mut harnesses = builtin_harnesses();
    harnesses.extend(supplied);
    Ok(harnesses)
}

fn deserialize_profiles<'de, D: Deserializer<'de>>(
    d: D,
) -> std::result::Result<BTreeMap<String, Profile>, D::Error> {
    let supplied = BTreeMap::<String, Profile>::deserialize(d)?;
    let mut profiles = builtin_profiles();
    profiles.extend(supplied);
    Ok(profiles)
}

pub struct CommandContext<'a> {
    pub prompt_file: &'a Path,
    pub name: &'a str,
    pub task: &'a str,
    pub worktree: &'a Path,
    pub corral: &'a Path,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Quote {
    None,
    Single,
    Double,
}

/// Substitute only original template text, never text introduced by a substitution.
/// Existing single/double quotes are respected; a whole quoted `{model_args}` is
/// unwrapped because it represents multiple argv elements, not one string.
pub fn render_command(
    harness: &Harness,
    profile: &Profile,
    context: &CommandContext<'_>,
) -> Result<String> {
    if harness.command.trim().is_empty() || harness.command.contains('\0') {
        bail!("harness command must be nonempty and contain no NUL");
    }
    if let Some(error) = profile.fallback_harness_error() {
        bail!("{error}");
    }
    let mut output = String::with_capacity(harness.command.len());
    let mut remaining = harness.command.as_str();
    let mut quote = Quote::None;
    while !remaining.is_empty() {
        if quote == Quote::None {
            let wrapped = ["'{model_args}'", "\"{model_args}\""]
                .into_iter()
                .find(|token| remaining.starts_with(token));
            if let Some(token) = wrapped {
                append_model_args(&mut output, harness, profile)?;
                remaining = &remaining[token.len()..];
                continue;
            }
        }
        let token = [
            "prompt_file",
            "prompt_ref",
            "name",
            "task",
            "worktree",
            "corral",
            "model_args",
        ]
        .into_iter()
        .find(|name| {
            remaining
                .strip_prefix('{')
                .and_then(|rest| rest.strip_prefix(name))
                .is_some_and(|rest| rest.starts_with('}'))
        });
        if let Some(token) = token {
            if token == "model_args" {
                if quote != Quote::None {
                    bail!("{{model_args}} must be unquoted or alone inside its quotes");
                }
                append_model_args(&mut output, harness, profile)?;
            } else {
                match token {
                    "prompt_file" => {
                        append_quoted(&mut output, &absolute_text(context.prompt_file)?, quote)?
                    }
                    "prompt_ref" => append_quoted(
                        &mut output,
                        &format!(
                            "Read {} and follow its instructions.",
                            absolute_text(context.prompt_file)?
                        ),
                        quote,
                    )?,
                    "name" => append_quoted(&mut output, context.name, quote)?,
                    "task" => append_quoted(&mut output, context.task, quote)?,
                    "worktree" => {
                        append_quoted(&mut output, &absolute_text(context.worktree)?, quote)?
                    }
                    "corral" => append_quoted(&mut output, &absolute_text(context.corral)?, quote)?,
                    _ => bail!("unsupported harness placeholder {token}"),
                }
            }
            remaining = &remaining[token.len() + 2..];
            continue;
        }
        append_shell_char(&mut output, &mut remaining, &mut quote);
    }
    // Pi launch flags apply even when an overridden template omits {model_args}.
    for argument in profile.fallback_args() {
        output.push(' ');
        append_quoted(&mut output, argument, Quote::None)?;
    }
    Ok(output)
}

// Copy literal shell syntax and update the template's quoting context.
fn append_shell_char(output: &mut String, remaining: &mut &str, quote: &mut Quote) {
    let Some(ch) = remaining.chars().next() else {
        return;
    };
    output.push(ch);
    *remaining = &remaining[ch.len_utf8()..];
    if ch == '\\' && *quote != Quote::Single {
        if let Some(escaped) = remaining.chars().next() {
            output.push(escaped);
            *remaining = &remaining[escaped.len_utf8()..];
        }
    } else {
        *quote = match (*quote, ch) {
            (Quote::None, '\'') => Quote::Single,
            (Quote::None, '"') => Quote::Double,
            (Quote::Single, '\'') | (Quote::Double, '"') => Quote::None,
            _ => *quote,
        };
    }
}

fn absolute_text(path: &Path) -> Result<String> {
    let absolute = std::path::absolute(path)
        .with_context(|| format!("making command path absolute: {}", path.display()))?;
    absolute
        .to_str()
        .map(str::to_owned)
        .with_context(|| format!("command path is not UTF-8: {}", path.display()))
}

fn append_model_args(output: &mut String, harness: &Harness, profile: &Profile) -> Result<()> {
    let mut first = true;
    let mut append = |argument: &str| -> Result<()> {
        if !first {
            output.push(' ');
        }
        first = false;
        append_quoted(output, argument, Quote::None)
    };
    for (value, flag, name) in [
        (&profile.model, &harness.model_flag, "model_flag"),
        (&profile.effort, &harness.effort_flag, "effort_flag"),
    ] {
        if let Some(value) = value {
            let flag = flag
                .as_deref()
                .filter(|flag| !flag.trim().is_empty())
                .with_context(|| format!("profile requires a harness {name}"))?;
            append(flag)?;
            append(value)?;
        }
    }
    for argument in &profile.args {
        append(argument)?;
    }
    Ok(())
}

fn append_quoted(output: &mut String, value: &str, quote: Quote) -> Result<()> {
    if value.contains('\0') {
        bail!("command argument contains NUL");
    }
    if quote == Quote::None {
        output.push('\'');
    }
    for ch in value.chars() {
        match (quote, ch) {
            (Quote::None | Quote::Single, '\'') => output.push_str("'\"'\"'"),
            (Quote::Double, '\\' | '"' | '$' | '`') => {
                output.push('\\');
                output.push(ch);
            }
            _ => output.push(ch),
        }
    }
    if quote == Quote::None {
        output.push('\'');
    }
    Ok(())
}

pub(crate) fn shell_quote(value: &str) -> Result<String> {
    let mut quoted = String::with_capacity(value.len() + 2);
    append_quoted(&mut quoted, value, Quote::None)?;
    Ok(quoted)
}

pub(crate) fn reserved_project_name(name: &str) -> bool {
    name.contains(':')
        || name.strip_prefix("T-").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
}

pub(crate) fn valid_env_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
