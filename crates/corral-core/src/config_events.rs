//! Serialized config observation shared by independent board, sync and CLI processes.
//! Only redacted values and fingerprints are persisted; source text never enters the event log.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::operating::digest;
use crate::store::atomic_write;
use crate::{
    ActionAuthorization, AuthorizationRevision, AuthorizedSubmission, OperatingActionIdentity,
    OperatingActionKind, OperatingOrigin, SubmissionDisposition,
};
use crate::{Config, ConfigDoc, ConfigLayer, ConfigLayers, Store};

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSource {
    FileEdit,
    SettingsUi,
    Cli,
}

enum ApprovalPublication {
    RevokeBeforeConfig,
    ConfirmAfterConfig,
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
    /// Persist origin before executor preparation. There is deliberately no default origin.
    pub fn authorize_operating_action(
        &self,
        action: OperatingActionIdentity,
        origin: OperatingOrigin,
    ) -> Result<ActionAuthorization> {
        ensure!(
            !action.run.is_empty() && !action.attempt.is_empty(),
            "action needs exact run and attempt"
        );
        ensure!(
            Self::normalize_id(&action.task)? == action.task,
            "action needs canonical task identity"
        );
        match action.kind {
            OperatingActionKind::Gate
            | OperatingActionKind::Review
            | OperatingActionKind::Repair
            | OperatingActionKind::Accept
            | OperatingActionKind::AcceptAggregate => ensure!(
                action.candidate.is_some(),
                "action needs an exact candidate identity"
            ),
            OperatingActionKind::PlanReview | OperatingActionKind::AcceptPlan => ensure!(
                action.plan.as_ref().is_some_and(|plan| !plan.is_empty()),
                "action needs an exact plan revision"
            ),
            OperatingActionKind::Plan => ensure!(
                action.round.is_some(),
                "planning action needs an exact revision round"
            ),
            OperatingActionKind::Dispatch => {}
        }
        let _board = self.lock("dispatch")?;
        let _task = self.lock(&action.task)?;
        let config = self.operating_config()?;
        let task = self.get(&action.task)?;
        let mut authorization = ActionAuthorization {
            id: String::new(),
            workspace: self.root().to_owned(),
            member: self.operating_member(&config, &task)?,
            policy_revision: self.policy_revision(&config)?,
            action,
            origin,
        };
        self.validate_operating_action(&config, &task, &authorization)?;
        // The journal belongs to the exact executor attempt, not its current grant.
        // A changed origin, grant or candidate must not create a second submission
        // slot for the same action after interruption or policy replacement.
        authorization.id = digest(&(
            self.root(),
            &authorization.action.task,
            &authorization.action.run,
            &authorization.action.attempt,
            authorization.action.kind,
            &authorization.action.plan,
            authorization.action.round,
        ))?;
        let path = self.submission_path(&authorization.id)?;
        if path.exists() {
            let mut record = self.authorized_submission(&authorization.id)?;
            if record.authorization != authorization {
                ensure!(
                    matches!(
                        record.disposition,
                        SubmissionDisposition::Prepared
                            | SubmissionDisposition::AuthorizationRefused { .. }
                    ),
                    "cannot refresh authority after a submission attempt"
                );
                ensure!(
                    matches!(
                        (&record.authorization.origin, &authorization.origin),
                        (
                            OperatingOrigin::Automatic { .. },
                            OperatingOrigin::Automatic { .. }
                        )
                    ),
                    "cannot change the action's authorization origin"
                );
                ensure!(
                    record.authorization.workspace == authorization.workspace
                        && record.authorization.member == authorization.member
                        && record.authorization.action == authorization.action,
                    "cannot refresh a changed prepared action identity"
                );
                record.authorization_history.push(AuthorizationRevision {
                    authorization: std::mem::replace(
                        &mut record.authorization,
                        authorization.clone(),
                    ),
                    disposition: std::mem::replace(
                        &mut record.disposition,
                        SubmissionDisposition::Prepared,
                    ),
                });
                self.save_authorized_submission(&record)?;
            }
        } else {
            self.save_authorized_submission(&AuthorizedSubmission {
                version: 1,
                authorization: authorization.clone(),
                disposition: SubmissionDisposition::Prepared,
                authorization_history: Vec::new(),
            })?;
        }
        Ok(authorization)
    }

    pub fn operating_state(&self) -> Result<crate::OperatingState> {
        let path = self.state_dir().join("automation.json");
        match fs::read_to_string(&path) {
            Ok(text) => {
                let state: crate::OperatingState = serde_json::from_str(&text)
                    .with_context(|| format!("parsing {}", path.display()))?;
                ensure!(
                    state.version == 1,
                    "unsupported automation state version {}",
                    state.version
                );
                Ok(state)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(crate::OperatingState::default())
            }
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub(crate) fn save_operating_state(&self, state: &crate::OperatingState) -> Result<()> {
        fs::create_dir_all(self.state_dir()).context("creating operating state")?;
        crate::store::atomic_write(
            &self.state_dir().join("automation.json"),
            &serde_json::to_string(state).context("encoding operating state")?,
        )
    }

    pub fn record_operating_presentation(
        &self,
        id: &str,
        receipt: crate::OperatingPresentation,
    ) -> Result<()> {
        ensure!(
            !id.is_empty() && !receipt.request_digest.is_empty() && !receipt.delivery.is_empty(),
            "presentation needs request and delivery identities"
        );
        let _board = self.lock("dispatch")?;
        let mut state = self.operating_state()?;
        if let Some(previous) = state.presentations.get(id) {
            if previous == &receipt {
                return Ok(());
            }
            ensure!(
                previous.request_digest == receipt.request_digest
                    && previous.delivery == receipt.delivery
                    && !previous.confirmed
                    && receipt.confirmed,
                "presentation identity already recorded"
            );
        }
        state.presentations.insert(id.to_owned(), receipt);
        self.save_operating_state(&state)
    }

    /// Records edits made before a short-lived sync process opened the workspace.
    pub(crate) fn load_initial_config(&mut self) -> Result<()> {
        let _lock = self.lock("config")?;
        let layers = self.config_layers()?;
        let text = layers.identity();
        let parsed = parse_layers(&layers, self.root(), self.observation()?.is_some());
        let config = match parsed {
            Ok(config) => config,
            Err(error) => {
                self.observe_invalid_config(&text, &error)?;
                anyhow::bail!("{error}; keeping previous config");
            }
        };
        self.observe_config(&config, &text, ConfigSource::FileEdit)?;
        self.warn_deprecated_max_diff_lines(&config);
        self.config = config;
        self.config_hash = content_hash(&text);
        Ok(())
    }

    /// `max_diff_lines` is accepted and ignored (T-214). Warns once per workspace, across processes,
    /// via a marker in the state dir; a marker that cannot be written only means the warning repeats.
    fn warn_deprecated_max_diff_lines(&self, config: &Config) {
        let layers = config.deprecated_max_diff_lines_layers();
        if layers.is_empty() {
            return;
        }
        let marker = self.state_dir().join("max-diff-lines-warned");
        if marker.exists() {
            return;
        }
        crate::messages::report(
            None,
            "Config",
            format!(
                "max_diff_lines in {} is deprecated and ignored: diffs of any size go through normal review; remove the key",
                layers.join(", ")
            ),
        );
        if fs::create_dir_all(self.state_dir()).is_ok() {
            let _ = fs::write(&marker, "");
        }
    }

    /// Saves through the same validated, comment-preserving CLI write path.
    pub fn save_config(&self) -> Result<()> {
        let base = self.config_layers()?.config()?;
        self.edit_config(|doc| doc.apply_config_changes(&base, &self.config))
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
        self.edit_config_layer_from(ConfigLayer::Workspace, source, f)
    }

    /// Edit only the selected raw document, validating the resulting effective configuration.
    pub fn edit_config_layer_from<F>(
        &self,
        layer: ConfigLayer,
        source: ConfigSource,
        f: F,
    ) -> Result<Config>
    where
        F: FnOnce(&mut ConfigDoc) -> Result<()>,
    {
        let _board = self.lock("dispatch")?;
        self.edit_config_layer_locked(layer, source, ApprovalPublication::RevokeBeforeConfig, f)
    }

    /// Caller owns the board boundary; do not reacquire it during grant publication.
    fn edit_config_layer_locked<F>(
        &self,
        layer: ConfigLayer,
        source: ConfigSource,
        publication: ApprovalPublication,
        f: F,
    ) -> Result<Config>
    where
        F: FnOnce(&mut ConfigDoc) -> Result<()>,
    {
        let _lock = self.lock("config")?;
        let _user = match layer {
            ConfigLayer::User => Some(self.lock_user_config()?),
            ConfigLayer::Workspace => None,
            ConfigLayer::Default | ConfigLayer::Mixed => anyhow::bail!("{layer} is not writable"),
        };
        let mut layers = self.config_layers()?;
        let path = layers.path(layer)?;
        let text = layers.identity();
        let before = match parse_layers(&layers, self.root(), false) {
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
        let mut doc = layers.doc(layer)?;
        f(&mut doc)?;
        layers.replace(layer, &doc)?;
        let config = layers
            .config()
            .with_context(|| format!("refusing to save {}", path.display()))?;
        config
            .validate(self.root())
            .with_context(|| format!("refusing to save {}", path.display()))?;
        layers
            .validate(self.root())
            .with_context(|| format!("refusing to save {}", path.display()))?;
        // A generic edit may withdraw or change a grant, but can never restore its
        // old approval by copying a stale configuration snapshot back later.
        match publication {
            ApprovalPublication::RevokeBeforeConfig => {
                if before.automation.grant_digest()? != config.automation.grant_digest()? {
                    let mut state = self.operating_state()?;
                    state.approval = None;
                    self.save_operating_state(&state)?;
                }
            }
            // Only a fresh preview confirmed below may publish a new identity
            // before its final matching approval, under the same board lock.
            ApprovalPublication::ConfirmAfterConfig => {}
        }
        atomic_write(&path, &doc.to_string())?;
        self.observe_config(&config, &layers.identity(), source)?;
        Ok(config)
    }

    /// Recompute the exact preview under the board boundary before publishing authority.
    /// Configuration is written first; a missing/failed final receipt cannot activate it.
    pub fn confirm_operating_policy(
        &self,
        preview: &crate::OperatingPreview,
        confirmed_by: &str,
        confirmed_at: jiff::Timestamp,
    ) -> Result<Config> {
        anyhow::ensure!(
            !confirmed_by.trim().is_empty(),
            "owner confirmation identity is required"
        );
        let _board = self.lock("dispatch")?;
        let before = self.operating_config()?;
        let current = self.operating_preview(&before, &preview.selection)?;
        anyhow::ensure!(
            &current == preview,
            "operating preview is stale; review a fresh preview"
        );
        let mut state = self.operating_state()?;
        let mut draft = before.clone();
        draft.automation = current.policy;
        let config = self.edit_config_layer_locked(
            ConfigLayer::Workspace,
            ConfigSource::Cli,
            ApprovalPublication::ConfirmAfterConfig,
            |doc| doc.apply_config_changes(&before, &draft),
        )?;
        state.approval = Some(crate::OperatingApproval {
            authorization: preview.id.clone(),
            revision: config.automation.revision,
            workspace: self.root().to_owned(),
            digest: config.automation.grant_digest()?,
            confirmed_by: confirmed_by.to_owned(),
            confirmed_at,
        });
        self.save_operating_state(&state)?;
        Ok(config)
    }

    /// Withdraw automatic authority without interrupting running processes.
    pub fn assist_operating_policy(&self) -> Result<Config> {
        self.edit_config(|doc| {
            doc.set_str("automation.mode", "assist")?;
            doc.unset("automation.authorization")?;
            Ok(())
        })
    }

    /// Pause is a narrowing control; resume requires the original matching grant.
    pub fn pause_operating_policy(&self, paused: bool) -> Result<Config> {
        let _board = self.lock("dispatch")?;
        if !paused {
            let config = self.operating_config()?;
            anyhow::ensure!(
                self.resolve_operating_policy(&config)?.mode == crate::OperatingMode::Autopilot,
                "cannot resume without a matching workspace grant"
            );
        }
        self.edit_config_layer_locked(
            ConfigLayer::Workspace,
            ConfigSource::Cli,
            ApprovalPublication::RevokeBeforeConfig,
            |doc| doc.set_str("automation.paused", if paused { "true" } else { "false" }),
        )
    }

    /// Turbo widens only an existing matching grant: it drops owner-only acceptance and round limits.
    pub fn turbo_operating_policy(&self, on: bool) -> Result<Config> {
        let _board = self.lock("dispatch")?;
        self.edit_config_layer_locked(
            ConfigLayer::Workspace,
            ConfigSource::Cli,
            ApprovalPublication::RevokeBeforeConfig,
            |doc| doc.set_str("automation.turbo", if on { "true" } else { "false" }),
        )
    }

    /// Invalid edits are logged once per content revision across processes and reported through
    /// the terminal owner's sink. The previous effective config remains installed.
    pub fn reload_config_if_changed(&mut self) -> Result<bool> {
        let _lock = self.lock("config")?;
        let layers = self.config_layers()?;
        let text = layers.identity();
        let hash = content_hash(&text);
        if hash == self.config_hash {
            return Ok(false);
        }
        match parse_layers(&layers, self.root(), true) {
            Ok(config) => {
                self.observe_config(&config, &text, ConfigSource::FileEdit)?;
                self.warn_deprecated_max_diff_lines(&config);
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

fn validation_error(errors: crate::ConfigErrors) -> String {
    format!(
        "configuration validation failed ({} problems; source values omitted)",
        errors.0.len()
    )
}

/// Parser and validation Display may echo secret values. Persist only category and file paths.
pub(crate) fn parse_layers(
    layers: &ConfigLayers,
    root: &std::path::Path,
    validate: bool,
) -> Result<Config, String> {
    let paths = [ConfigLayer::User, ConfigLayer::Workspace]
        .into_iter()
        .filter_map(|layer| layers.path(layer).ok())
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let config = layers
        .config()
        .map_err(|_| format!("TOML parse/schema error in {paths} (source values omitted)"))?;
    if validate {
        layers.validate(root).map_err(|_| {
            format!("configuration validation failed in {paths} (source values omitted)")
        })?;
    }
    Ok(config)
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
        "automation.",
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
