//! Serialized config observation shared by independent board, sync and CLI processes.
//! Only redacted values and fingerprints are persisted; source text never enters the event log.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::store::atomic_write;
use crate::{Config, ConfigDoc, Store};

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSource {
    FileEdit,
    SettingsUi,
    Cli,
}

#[derive(Debug, Serialize, Deserialize)]
struct Leaf {
    value: Value,
    fingerprint: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Observation {
    hash: String,
    values: BTreeMap<String, Leaf>,
    #[serde(default)]
    invalid: bool,
}

impl Store {
    /// Records edits made before a short-lived sync process opened the workspace.
    pub(crate) fn load_initial_config(&mut self) -> Result<()> {
        let _lock = self.lock("config")?;
        let path = self.config_path();
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let parsed = if self.observation()?.is_some() {
            parse_config(&text, self.root())
        } else {
            parse_schema(&text)
        };
        let config = match parsed {
            Ok(config) => config,
            Err(error) => {
                self.observe_invalid_config(&text, &error)?;
                anyhow::bail!("{error}; keeping previous config");
            }
        };
        self.observe_config(&config, &text, ConfigSource::FileEdit)?;
        self.config = config;
        self.config_hash = content_hash(&text);
        Ok(())
    }

    /// Saves through the same validated, comment-preserving CLI write path.
    pub fn save_config(&self) -> Result<()> {
        self.edit_config(|doc| doc.sync_from(&self.config))
            .map(|_| ())
    }

    /// Validated CLI edit. The returned config is not installed in this Store until reload.
    pub fn edit_config<F>(&self, f: F) -> Result<Config>
    where
        F: FnOnce(&mut ConfigDoc) -> Result<()>,
    {
        self.edit_config_from(ConfigSource::Cli, f)
    }

    /// The single write path: observes pending file edits, validates, saves atomically and
    /// records the semantic change under the same cross-process config lock.
    pub fn edit_config_from<F>(&self, source: ConfigSource, f: F) -> Result<Config>
    where
        F: FnOnce(&mut ConfigDoc) -> Result<()>,
    {
        let _board = self.lock("dispatch")?;
        let _lock = self.lock("config")?;
        let path = self.config_path();
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let before = match parse_schema(&text) {
            Ok(config) => config,
            Err(error) => {
                self.observe_invalid_config(&text, &error)?;
                anyhow::bail!("{error}; keeping previous config");
            }
        };
        match before.validate(self.root()) {
            Ok(()) => self.observe_config(&before, &text, ConfigSource::FileEdit)?,
            Err(errors) => {
                let error = validation_error(errors);
                self.observe_invalid_config(&text, &error)?;
                crate::messages::report(
                    None,
                    "Config not reloaded",
                    format!("keeping previous config; {error}"),
                );
            }
        }
        let mut doc = ConfigDoc::parse(&text)?;
        f(&mut doc)?;
        let config = doc
            .config()
            .with_context(|| format!("refusing to save {}", path.display()))?;
        config
            .validate(self.root())
            .with_context(|| format!("refusing to save {}", path.display()))?;
        let text = doc.to_string();
        atomic_write(&path, &text)?;
        self.observe_config(&config, &text, source)?;
        Ok(config)
    }

    /// Invalid edits are logged once per content revision across processes and reported through
    /// the terminal owner's sink. The previous effective config remains installed.
    pub fn reload_config_if_changed(&mut self) -> Result<bool> {
        let _lock = self.lock("config")?;
        let path = self.config_path();
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let hash = content_hash(&text);
        if hash == self.config_hash {
            return Ok(false);
        }
        match parse_config(&text, self.root()) {
            Ok(config) => {
                self.observe_config(&config, &text, ConfigSource::FileEdit)?;
                self.config = config;
                self.config_hash = hash;
                Ok(true)
            }
            Err(error) => {
                self.observe_invalid_config(&text, &error)?;
                self.config_hash = hash;
                crate::messages::report(
                    None,
                    "Config not reloaded",
                    format!("keeping previous config; {error}"),
                );
                Ok(false)
            }
        }
    }

    /// Autopilot toggles are CLI changes; they bind at the next sync, not to running agents.
    pub fn set_autopilot(&mut self, on: bool) -> Result<()> {
        let value = if on { "true" } else { "false" };
        self.config = self.edit_config(|doc| doc.set_str("review.autopilot", value))?;
        Ok(())
    }

    fn observation(&self) -> Result<Option<Observation>> {
        let path = self.state_dir().join("config-observation.json");
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display()))
                .map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    fn save_observation(&self, observation: &Observation) -> Result<()> {
        fs::create_dir_all(self.state_dir()).context("creating config observation state")?;
        atomic_write(
            &self.state_dir().join("config-observation.json"),
            &serde_json::to_string(observation).context("serializing config observation")?,
        )
    }

    fn observe_config(&self, config: &Config, text: &str, source: ConfigSource) -> Result<()> {
        let mut values = BTreeMap::new();
        flatten(
            "",
            &serde_json::to_value(config).context("serializing config")?,
            &mut values,
        )?;
        let current = Observation {
            hash: content_hash(text),
            values,
            invalid: false,
        };
        if let Some(previous) = self.observation()? {
            if previous.hash == current.hash && !previous.invalid {
                return Ok(());
            }
            let keys: BTreeSet<_> = previous
                .values
                .keys()
                .chain(current.values.keys())
                .collect();
            let mut changes = Vec::new();
            for key in keys {
                let old = previous.values.get(key);
                let new = current.values.get(key);
                if old.map(|v| &v.fingerprint) == new.map(|v| &v.fingerprint) {
                    continue;
                }
                changes.push(json!({
                    "key": key,
                    "old": old.map(|v| &v.value),
                    "new": new.map(|v| &v.value),
                    "pm_relevant": pm_relevant(key),
                    "applies": applies(key),
                }));
            }
            if !changes.is_empty() {
                self.log_event(None, "config_changed", json!({
                    "source": source,
                    "hash": current.hash,
                    "pm_relevant": changes.iter().any(|v| v["pm_relevant"] == true),
                    "changes": changes,
                    "message": "Running agents are not interrupted; changes apply to new dispatches, the next gate, or the next sync as marked.",
                }))?;
            }
        }
        self.save_observation(&current)
    }

    pub(crate) fn observe_invalid_config(&self, text: &str, error: &str) -> Result<()> {
        let mut previous = self.observation()?.unwrap_or_default();
        let hash = content_hash(text);
        if previous.hash != hash || !previous.invalid {
            self.log_event(
                None,
                "config_invalid",
                json!({
                    "source": ConfigSource::FileEdit,
                    "hash": hash,
                    "error": error,
                    "message": "keeping previous config",
                }),
            )?;
            previous.hash = hash;
            previous.invalid = true;
            self.save_observation(&previous)?;
        }
        Ok(())
    }
}

pub(crate) fn content_hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// Parser Display includes source lines and schema errors may echo secret values.
/// Keep the error category and exact location, never those untrusted excerpts.
pub(crate) fn parse_config(text: &str, root: &std::path::Path) -> Result<Config, String> {
    let config = parse_schema(text)?;
    config.validate(root).map_err(validation_error)?;
    Ok(config)
}

fn validation_error(errors: crate::ConfigErrors) -> String {
    format!(
        "configuration validation failed ({} problems; source values omitted)",
        errors.0.len()
    )
}

pub(crate) fn parse_schema(text: &str) -> Result<Config, String> {
    toml::from_str(text).map_err(|error: toml::de::Error| {
        let location = error
            .span()
            .map(|span| {
                let prefix = &text[..span.start.min(text.len())];
                let line = prefix.bytes().filter(|b| *b == b'\n').count() + 1;
                let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
                format!(" at line {line}, column {column}")
            })
            .unwrap_or_default();
        format!("TOML parse/schema error{location} (source values omitted)")
    })
}

fn flatten(key: &str, value: &Value, out: &mut BTreeMap<String, Leaf>) -> Result<()> {
    if let Value::Object(map) = value {
        for (part, value) in map {
            let part = if part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                part.clone()
            } else {
                serde_json::to_string(part).context("encoding config key")?
            };
            let path = if key.is_empty() {
                part
            } else {
                format!("{key}.{part}")
            };
            flatten(&path, value, out)?;
        }
    } else {
        let fingerprint =
            content_hash(&serde_json::to_string(value).context("encoding config value")?);
        out.insert(
            key.into(),
            Leaf {
                value: redacted(key, value),
                fingerprint,
            },
        );
    }
    Ok(())
}

fn redacted(key: &str, value: &Value) -> Value {
    if opaque(key) && !value.is_null() {
        return json!("[redacted]");
    }
    match value {
        Value::Array(items) => Value::Array(items.iter().map(|v| redacted(key, v)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(part, v)| (part.clone(), redacted(&format!("{key}.{part}"), v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn opaque(key: &str) -> bool {
    // Executable text, paths and arbitrary argv may contain credentials, including positional ones.
    let leaf = key.rsplit('.').next().unwrap_or(key);
    key.starts_with("viewers.")
        || key.contains(".post_land")
        || key.starts_with("review.env.")
        || key.contains(".review.env.")
        || matches!(
            leaf,
            "command"
                | "args"
                | "gate"
                | "prompt"
                | "path"
                | "model_flag"
                | "effort_flag"
                | "omo_agent_dir"
                | "omp_agent_dir"
                | "title_regex"
                | "body_regex"
        )
        || ["secret", "password", "token", "credential", "api_key"]
            .iter()
            .any(|s| key.to_ascii_lowercase().contains(s))
}

fn pm_relevant(key: &str) -> bool {
    [
        "limits.",
        "resources.",
        "routing.",
        "review.",
        "profiles.",
        "harnesses.",
        "agents.",
        "projects.",
        "pm.",
    ]
    .iter()
    .any(|prefix| key.starts_with(prefix))
        || key == "defaults.profile"
}

fn applies(key: &str) -> &'static str {
    if key.ends_with(".autopilot") || key.starts_with("limits.") || key.starts_with("pm.") {
        "next_sync"
    } else if key.starts_with("review.") || key.contains(".review.") {
        "next_gate"
    } else if [
        "routing.",
        "profiles.",
        "harnesses.",
        "agents.",
        "projects.",
        "defaults.",
    ]
    .iter()
    .any(|prefix| key.starts_with(prefix))
    {
        "new_dispatches"
    } else {
        "next_sync"
    }
}
