//! User preferences underneath workspace policy. Merge raw TOML before applying schema defaults.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

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
        let mut errors = Vec::new();
        let mut parse = |text: &str, layer| -> Result<toml::Value> {
            let path = self.path(layer)?;
            match toml::from_str::<toml::Table>(text) {
                Ok(table) => Ok(toml::Value::Table(table)),
                Err(e) => {
                    errors.push(format!("{}: {e}", path.display()));
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
        if let Some(table) = user.as_table() {
            for key in table.keys().filter(|key| !personal_key(key)) {
                errors.push(format!(
                    "{}: {key} is not allowed in the user layer; only profiles, harnesses, ui, defaults are personal",
                    self.path(ConfigLayer::User)?.display()
                ));
            }
        }
        if !errors.is_empty() {
            bail!("{}", errors.join("\n"));
        }
        Ok((user, workspace))
    }

    /// Schema-check both layers, including user values hidden by a workspace override.
    pub fn config(&self) -> Result<Config> {
        let (user, workspace) = self.tables()?;
        let mut errors = BTreeSet::new();
        let mut decode = |value: toml::Value, user_wins| -> Result<Option<Config>> {
            match value.try_into::<Config>() {
                Ok(config) => Ok(Some(config)),
                Err(e) => {
                    let diagnostic = e.to_string();
                    let key = schema_key(&e)?;
                    // Paths use literal dynamic names, just like semantic diagnostics.
                    let user_key = problem_key_len(&user, &key, "");
                    let workspace_key = problem_key_len(&workspace, &key, "");
                    let layer = if user_key > workspace_key
                        || user_wins && user_key > 0 && user_key == workspace_key
                    {
                        ConfigLayer::User
                    } else {
                        ConfigLayer::Workspace
                    };
                    errors.insert(format!("{}: {diagnostic}", self.path(layer)?.display()));
                    Ok(None)
                }
            }
        };
        if !self.user.is_empty() {
            decode(merge(workspace.clone(), user.clone()), true)?;
        }
        let config = decode(merge(user.clone(), workspace.clone()), false)?;
        if !errors.is_empty() {
            // A merged decode stops at its first error, which can hide another input's
            // unrelated bad value in both precedence orders. Probe each supplied value
            // independently. A missing sibling is real only when neither input supplies
            // it; otherwise the fragment is merely a legal partial table.
            for (layer, input) in [
                (ConfigLayer::User, &user),
                (ConfigLayer::Workspace, &workspace),
            ] {
                for fragment in schema_fragments(input) {
                    if let Err(error) = fragment.try_into::<Config>() {
                        if let Some(missing) = error
                            .message()
                            .strip_prefix("missing field `")
                            .and_then(|name| name.strip_suffix('`'))
                        {
                            let key = format!("{}.{missing}", schema_key(&error)?);
                            if problem_key_len(&user, &key, "") == key.len()
                                || problem_key_len(&workspace, &key, "") == key.len()
                            {
                                continue;
                            }
                        }
                        errors.insert(format!("{}: {error}", self.path(layer)?.display()));
                    }
                }
            }
            bail!("{}", errors.into_iter().collect::<Vec<_>>().join("\n"));
        }
        config.context("missing effective configuration")
    }

    /// Validate effective policy and masked personal values, with a file on every diagnostic.
    pub fn validate(&self, root: &Path) -> Result<()> {
        let config = self.config()?;
        let (user, workspace) = self.tables()?;
        let mut errors = BTreeSet::new();
        let mut add = |config: &Config, user_wins: bool| -> Result<()> {
            if let Err(problems) = config.validate(root) {
                for problem in problems.0 {
                    // Validators display dynamic names literally, not as quoted TOML paths.
                    // Match known keys directly so names containing dots or spaces keep ownership.
                    let shown = problem.trim_start_matches('[');
                    let user_key = problem_key_len(&user, shown, "");
                    let workspace_key = problem_key_len(&workspace, shown, "");
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
            }
            Ok(())
        };
        add(&config, false)?;
        if !self.user.is_empty() {
            let personal: Config = merge(workspace.clone(), user.clone())
                .try_into()
                .context("decoding personal configuration")?;
            add(&personal, true)?;
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ConfigErrors(errors.into_iter().collect()).into())
        }
    }

    pub fn source(&self, key: &str) -> Result<ConfigLayer> {
        let keys = toml_edit::Key::parse(key).with_context(|| format!("invalid key {key:?}"))?;
        let parts: Vec<_> = keys.iter().map(|k| k.get().to_owned()).collect();
        let (user, workspace) = self.tables()?;
        let effective = self.config()?.effective_value()?;
        let raw = merge(user.clone(), workspace.clone());
        let Some(value) = lookup(&effective, &parts).or_else(|| lookup(&raw, &parts)) else {
            return Ok(ConfigLayer::Default);
        };
        let mut sources = BTreeSet::new();
        collect_sources(value, &mut parts.clone(), &user, &workspace, &mut sources);
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

fn schema_fragments(value: &toml::Value) -> Vec<toml::Value> {
    match value {
        toml::Value::Table(table) if !table.is_empty() => table
            .iter()
            .flat_map(|(key, value)| {
                schema_fragments(value)
                    .into_iter()
                    .map(|value| toml::Value::Table(toml::Table::from_iter([(key.clone(), value)])))
            })
            .collect(),
        // Arrays replace as a unit; probing their individual elements would turn
        // incomplete synthetic objects into errors in otherwise valid input.
        _ => vec![value.clone()],
    }
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
