//! User preferences underneath workspace policy. Merge raw TOML before applying schema defaults.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::config_validation::{SemanticInputs, independent_problems};
use crate::profile::ProfileProblem;
use crate::{Config, ConfigDoc, ConfigErrors};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConfigLayer {
    Default,
    User,
    Workspace,
    Mixed,
}

impl fmt::Display for ConfigLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Default => "default",
            Self::User => "user",
            Self::Workspace => "workspace",
            Self::Mixed => "mixed",
        })
    }
}

#[derive(Debug, Clone)]
pub struct ConfigLayers {
    workspace_path: PathBuf,
    user_path: Option<PathBuf>,
    workspace: String,
    user: String,
}

/// Parsed provenance for one immutable snapshot of the configuration layers.
#[derive(Debug, Clone)]
pub struct ConfigSources {
    user: toml::Value,
    workspace: toml::Value,
    effective: toml::Value,
    raw: toml::Value,
}

impl ConfigLayers {
    pub fn user_path() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|p| !p.is_empty())
                    .map(|p| PathBuf::from(p).join(".config"))
            })
            .map(|p| p.join("corral/config.toml"))
    }

    pub fn load(workspace: &Path) -> Result<Self> {
        Self::load_paths(workspace, Self::user_path().as_deref())
    }

    /// Explicit paths keep tests independent of the process's HOME and XDG environment.
    pub fn load_paths(workspace: &Path, user: Option<&Path>) -> Result<Self> {
        let read = |path: &Path| {
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
        };
        let workspace_text = read(workspace);
        let user_text = match user {
            Some(path) => match std::fs::read_to_string(path) {
                Ok(text) => Ok(text),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
                Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
            },
            None => Ok(String::new()),
        };
        let mut errors = Vec::new();
        for result in [&workspace_text, &user_text] {
            if let Err(e) = result {
                errors.push(format!("{e:#}"));
            }
        }
        if !errors.is_empty() {
            bail!("{}", errors.join("\n"));
        }
        Ok(Self {
            workspace_path: workspace.to_path_buf(),
            user_path: user.map(Path::to_path_buf),
            workspace: workspace_text?,
            user: user_text?,
        })
    }

    pub fn path(&self, layer: ConfigLayer) -> Result<PathBuf> {
        match layer {
            ConfigLayer::Workspace => Ok(self.workspace_path.clone()),
            ConfigLayer::User => self
                .user_path
                .clone()
                .context("user configuration needs XDG_CONFIG_HOME or HOME"),
            ConfigLayer::Default | ConfigLayer::Mixed => bail!("{layer} is not a writable layer"),
        }
    }

    pub fn doc(&self, layer: ConfigLayer) -> Result<ConfigDoc> {
        let text = match layer {
            ConfigLayer::User => &self.user,
            ConfigLayer::Workspace => &self.workspace,
            ConfigLayer::Default | ConfigLayer::Mixed => bail!("{layer} is not a writable layer"),
        };
        let path = self.path(layer)?;
        ConfigDoc::parse(text).with_context(|| format!("parsing {}", path.display()))
    }

    pub(crate) fn replace(&mut self, layer: ConfigLayer, doc: &ConfigDoc) -> Result<()> {
        match layer {
            ConfigLayer::User => self.user = doc.to_string(),
            ConfigLayer::Workspace => self.workspace = doc.to_string(),
            ConfigLayer::Default | ConfigLayer::Mixed => bail!("{layer} is not a writable layer"),
        }
        Ok(())
    }

    /// Keep the existing workspace-only hash contract when no user file contributes.
    pub(crate) fn identity(&self) -> String {
        if self.user.is_empty() {
            self.workspace.clone()
        } else {
            format!(
                "user:{}\n{}workspace:\n{}",
                self.user.len(),
                self.user,
                self.workspace
            )
        }
    }

    fn tables(&self) -> Result<(toml::Value, toml::Value)> {
        let mut errors = BTreeSet::new();
        let tables = self.parse_tables(&mut errors)?;
        if !errors.is_empty() {
            return Err(ConfigErrors(errors.into_iter().collect()).into());
        }
        Ok(tables)
    }

    fn parse_tables(&self, errors: &mut BTreeSet<String>) -> Result<(toml::Value, toml::Value)> {
        let mut parse = |text: &str, layer| -> Result<toml::Value> {
            let path = self.path(layer)?;
            match toml::from_str::<toml::Table>(text) {
                Ok(table) => Ok(toml::Value::Table(table)),
                Err(e) => {
                    errors.insert(format!("{}: {e}", path.display()));
                    Ok(toml::Value::Table(toml::Table::new()))
                }
            }
        };
        let user = if self.user_path.is_some() {
            parse(&self.user, ConfigLayer::User)?
        } else {
            toml::Value::Table(toml::Table::new())
        };
        let workspace = parse(&self.workspace, ConfigLayer::Workspace)?;
        Ok((user, workspace))
    }

    /// Schema-check both layers, including user values hidden by a workspace override.
    pub fn config(&self) -> Result<Config> {
        self.check(None)
    }

    fn check(&self, root: Option<&Path>) -> Result<Config> {
        let mut errors = BTreeSet::new();
        let (user, workspace) = self.parse_tables(&mut errors)?;
        let tables_complete = errors.is_empty();
        if let Some(table) = user.as_table() {
            for key in table.keys().filter(|key| !personal_key(key)) {
                errors.insert(format!(
                    "{}: {key} is not allowed in the user layer; only profiles, harnesses, ui, defaults are personal",
                    self.path(ConfigLayer::User)?.display()
                ));
            }
        }
        let mut effective = None;
        let mut decoded = Vec::new();
        for user_wins in [false, true] {
            if user_wins && self.user.is_empty() {
                continue;
            }
            let mut value = if user_wins {
                merge(workspace.clone(), user.clone())
            } else {
                merge(user.clone(), workspace.clone())
            };
            // These checks do not require the parent to survive schema recovery.
            if root.is_some() {
                for problem in independent_problems(&value, tables_complete) {
                    self.add_field_problem(problem, &user, &workspace, user_wins, &mut errors)?;
                }
            }
            let declared_profiles: BTreeSet<String> = ["profiles", "agents"]
                .iter()
                .filter_map(|section| value.get(section)?.as_table())
                .flat_map(|table| table.keys().cloned())
                .collect();
            let mut rejected: Vec<Vec<String>> = Vec::new();
            let config = loop {
                match value.clone().try_into::<Config>() {
                    Ok(config) => break config,
                    Err(e) => {
                        let diagnostic = e.to_string();
                        let key = schema_key(&e)?;
                        let parts = schema_path(&value, &key)
                            .context("schema error is missing its input value")?;
                        let user_key = problem_key_len(&user, &key, "");
                        let workspace_key = problem_key_len(&workspace, &key, "");
                        let layer = if user_key > workspace_key
                            || user_wins && user_key > 0 && user_key == workspace_key
                        {
                            ConfigLayer::User
                        } else {
                            ConfigLayer::Workspace
                        };
                        // Removing a bad required field can make its parent incomplete.
                        // That missing-field error is synthetic, not another input problem.
                        if !e.message().starts_with("missing field `")
                            || tables_complete
                                && !rejected.iter().any(|path| path.starts_with(&parts))
                        {
                            errors.insert(format!("{}: {diagnostic}", self.path(layer)?.display()));
                        }
                        remove_path(&mut value, &parts)?;
                        rejected.push(parts);
                    }
                }
            };
            let rejected_profiles: Vec<_> = declared_profiles
                .into_iter()
                .filter(|name| {
                    !config.profiles.contains_key(name) && !config.agents.contains_key(name)
                })
                .collect();
            let inputs = SemanticInputs {
                tables_complete,
                independent_checked: root.is_some(),
                rejected_profiles,
                rejected_fields: rejected,
            };
            decoded.push((user_wins, value, config, inputs));
        }
        for (user_wins, value, config, inputs) in decoded {
            if let Some(root) = root {
                self.add_semantics(
                    &config,
                    config.problems(root, &inputs),
                    (&user, &workspace, &value),
                    user_wins,
                    &inputs,
                    &mut errors,
                )?;
            }
            if !user_wins {
                effective = Some(config);
            }
        }
        if !errors.is_empty() {
            return Err(ConfigErrors(errors.into_iter().collect()).into());
        }
        effective.context("missing effective configuration")
    }

    /// Validate effective policy and masked personal values, with a file on every diagnostic.
    pub fn validate(&self, root: &Path) -> Result<()> {
        self.check(Some(root)).map(|_| ())
    }

    fn add_semantics(
        &self,
        config: &Config,
        problems: Vec<String>,
        inputs: (&toml::Value, &toml::Value, &toml::Value),
        user_wins: bool,
        availability: &SemanticInputs,
        errors: &mut BTreeSet<String>,
    ) -> Result<()> {
        let (user, workspace, decoded) = inputs;
        for problem in problems {
            // Validators display dynamic names literally, not as quoted TOML paths.
            // Match known keys directly so names containing dots or spaces keep ownership.
            let shown = problem.trim_start_matches('[');
            // Defaults and removed schema-invalid values must not manufacture
            // dependent semantic errors while checking a partial configuration.
            if !errors.is_empty() && problem_key_len(decoded, shown, "") == 0 {
                continue;
            }
            let user_key = problem_key_len(user, shown, "");
            let workspace_key = problem_key_len(workspace, shown, "");
            let layer = if user_key > workspace_key
                || user_wins && user_key > 0 && user_key == workspace_key
            {
                ConfigLayer::User
            } else {
                ConfigLayer::Workspace
            };
            if !user_wins || layer == ConfigLayer::User {
                errors.insert(format!("{}: {problem}", self.path(layer)?.display()));
            }
        }
        for problem in config.profile_problems() {
            // Use actual path segments, never parse a displayed profile name.
            if lookup(decoded, &problem.key).is_none()
                || !availability.profile_problem_available(config, decoded, &problem)
            {
                continue;
            }
            if let [section, name, field] = problem.key.as_slice()
                && section == "profiles"
                && field == "harness"
            {
                let harness = &config.profiles[name].harness;
                // Unknown references require a complete inventory. A declared harness
                // removed by schema recovery is not evidence of an unknown name either.
                if ["harnesses", "agents"].iter().any(|section| {
                    let path = [section.to_string(), harness.clone()];
                    !availability.available(&path)
                        || lookup(user, &path).is_some()
                        || lookup(workspace, &path).is_some()
                }) {
                    continue;
                }
            }
            self.add_field_problem(problem, user, workspace, user_wins, errors)?;
        }
        Ok(())
    }

    fn add_field_problem(
        &self,
        problem: ProfileProblem,
        user: &toml::Value,
        workspace: &toml::Value,
        user_wins: bool,
        errors: &mut BTreeSet<String>,
    ) -> Result<()> {
        let user_supplies = lookup(user, &problem.key).is_some();
        let workspace_supplies = lookup(workspace, &problem.key).is_some();
        let layer = if user_supplies && (user_wins || !workspace_supplies) {
            ConfigLayer::User
        } else {
            ConfigLayer::Workspace
        };
        if !user_wins || layer == ConfigLayer::User {
            errors.insert(format!(
                "{}: {}: {}",
                self.path(layer)?.display(),
                problem.key.join("."),
                problem.message
            ));
        }
        Ok(())
    }

    pub fn source(&self, key: &str) -> Result<ConfigLayer> {
        self.sources()?.source(key)
    }

    /// Resolve many field sources without reparsing the layer documents per field.
    pub fn sources(&self) -> Result<ConfigSources> {
        let (user, workspace) = self.tables()?;
        let effective = self.config()?.effective_value()?;
        let raw = merge(user.clone(), workspace.clone());
        Ok(ConfigSources {
            user,
            workspace,
            effective,
            raw,
        })
    }
}

impl ConfigSources {
    pub fn source(&self, key: &str) -> Result<ConfigLayer> {
        let keys = toml_edit::Key::parse(key).with_context(|| format!("invalid key {key:?}"))?;
        let parts: Vec<_> = keys.iter().map(|k| k.get().to_owned()).collect();
        let Some(value) = lookup(&self.effective, &parts).or_else(|| lookup(&self.raw, &parts))
        else {
            return Ok(ConfigLayer::Default);
        };
        let mut sources = BTreeSet::new();
        collect_sources(
            value,
            &mut parts.clone(),
            &self.user,
            &self.workspace,
            &mut sources,
        );
        Ok(if sources.len() > 1 {
            ConfigLayer::Mixed
        } else {
            sources.into_iter().next().unwrap_or(ConfigLayer::Default)
        })
    }
}

pub fn personal_key(key: &str) -> bool {
    matches!(key, "profiles" | "harnesses" | "ui" | "defaults")
}

fn problem_key_len(value: &toml::Value, problem: &str, prefix: &str) -> usize {
    let Some(table) = value.as_table() else {
        return prefix.len();
    };
    table
        .iter()
        .filter_map(|(key, value)| {
            let key = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            let rest = problem.strip_prefix(&key)?;
            if rest.is_empty() || rest.starts_with(['.', ':', ' ', ']']) {
                Some(problem_key_len(value, problem, &key).max(key.len()))
            } else {
                None
            }
        })
        .max()
        .unwrap_or(prefix.len())
}

fn lookup<'a>(value: &'a toml::Value, parts: &[String]) -> Option<&'a toml::Value> {
    let mut value = value;
    for (index, part) in parts.iter().enumerate() {
        let table = value.as_table()?;
        value = table.get(part).or_else(|| {
            // The project's canonical field accepts this legacy spelling. Routing's
            // title/body aliases live inside arrays, whose provenance is atomic.
            match (parts.first().map(String::as_str), index, part.as_str()) {
                (Some("projects"), 2, "default_profile") => table.get("default_agent"),
                _ => None,
            }
        })?;
    }
    Some(value)
}

fn schema_path(value: &toml::Value, shown: &str) -> Option<Vec<String>> {
    for (key, value) in value.as_table()? {
        if shown == key {
            return Some(vec![key.clone()]);
        }
        if let Some(rest) = shown.strip_prefix(key).and_then(|s| s.strip_prefix('.')) {
            // Arrays are replaced atomically, not repaired into synthetic partial objects.
            if value.is_array() {
                return Some(vec![key.clone()]);
            }
            if let Some(mut parts) = schema_path(value, rest) {
                parts.insert(0, key.clone());
                return Some(parts);
            }
        }
    }
    None
}

fn remove_path(value: &mut toml::Value, parts: &[String]) -> Result<()> {
    let (last, parents) = parts
        .split_last()
        .context("schema error has an empty path")?;
    let mut table = value;
    for part in parents {
        table = table
            .get_mut(part)
            .context("schema error parent is absent")?;
    }
    table
        .as_table_mut()
        .context("schema error parent is not a table")?
        .remove(last);
    Ok(())
}

fn schema_key(error: &toml::de::Error) -> Result<String> {
    // TOML exposes the key path only in Display for a Value decode. Strip the
    // message first, since it may itself contain newlines or backticks.
    error
        .to_string()
        .strip_prefix(error.message())
        .and_then(|context| context.strip_prefix("\nin `"))
        .and_then(|context| context.strip_suffix("`\n"))
        .map(str::to_owned)
        .context("schema error is missing its key path")
}

fn collect_sources(
    value: &toml::Value,
    parts: &mut Vec<String>,
    user: &toml::Value,
    workspace: &toml::Value,
    sources: &mut BTreeSet<ConfigLayer>,
) {
    if let toml::Value::Table(table) = value
        && !table.is_empty()
    {
        for (key, value) in table {
            parts.push(key.clone());
            collect_sources(value, parts, user, workspace, sources);
            parts.pop();
        }
    } else {
        sources.insert(if lookup(workspace, parts).is_some() {
            ConfigLayer::Workspace
        } else if lookup(user, parts).is_some() {
            ConfigLayer::User
        } else {
            ConfigLayer::Default
        });
    }
}

fn merge(mut lower: toml::Value, upper: toml::Value) -> toml::Value {
    if let (Some(lower), toml::Value::Table(upper)) = (lower.as_table_mut(), &upper) {
        for (key, value) in upper {
            let value = match lower.remove(key) {
                Some(old) => merge(old, value.clone()),
                None => value.clone(),
            };
            lower.insert(key.clone(), value);
        }
    } else {
        lower = upper;
    }
    lower
}
