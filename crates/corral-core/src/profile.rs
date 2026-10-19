//! Harnesses (how to launch an agent CLI) and profiles (named harness + model + effort presets).
//!
//! A profile name resolves through `[profiles.<name>]`, then the legacy `[agents.<name>]` table (a legacy
//! agent is a profile with no model/effort whose harness is the agent itself).

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::Config;

/// Profile used when nothing else names one.
pub const FALLBACK_PROFILE: &str = "omo";

/// The semantic operation determines which supplied inputs a diagnostic requires.
#[derive(Clone, Copy)]
pub(crate) enum ProfileCheck {
    Independent,
    Harness,
    Model,
    Effort,
    Command,
}

impl ProfileCheck {
    pub fn capability_fields(self) -> &'static [&'static str] {
        match self {
            Self::Model => &["flags", "model_flag"],
            Self::Effort => &["flags", "effort_flag", "efforts"],
            Self::Independent | Self::Harness | Self::Command => &[],
        }
    }
}

/// Structured identity stays separate from diagnostic text, including literal dynamic names.
pub(crate) struct ProfileProblem {
    pub key: Vec<String>,
    pub message: String,
    pub profile: Option<String>,
    pub check: ProfileCheck,
}

/// How to launch an agent. `command` is a template with the `AgentCmd` placeholders plus `{model_args}`:
/// the rendered model/effort flags and the profile's extra args (empty when unset).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Harness {
    pub command: String,
    /// Built-in flag map: "omo", "omp", "claude" or "generic". Default: the harness key when it names a
    /// built-in, else "generic" (no model or effort).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flags: Option<String>,
    /// Overrides the flag map's model flag (e.g. "--model").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_flag: Option<String>,
    /// Overrides the flag map's effort flag (e.g. "--thinking").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_flag: Option<String>,
    /// Overrides the flag map's accepted effort levels (empty = keep the built-in list).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<String>,
    /// Dispatch prompt template for runs of this harness (relative to the workspace root, or absolute);
    /// overrides `.corral/prompts/run.md` and the built-in run contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

/// Capability decoding does not require a valid launch command or prompt.
#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct HarnessCapabilities {
    flags: Option<String>,
    model_flag: Option<String>,
    effort_flag: Option<String>,
    efforts: Vec<String>,
}

impl HarnessCapabilities {
    pub fn flag_map(&self, key: &str) -> Result<FlagMap> {
        Harness::configured_flag_map(
            key,
            self.flags.as_deref(),
            self.model_flag.as_deref(),
            self.effort_flag.as_deref(),
            &self.efforts,
        )
    }
}

/// A named preset: which harness, which model, which effort, plus extra argv.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// Absent on legacy/manual presets. The enclosing map key remains the stable identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<crate::ProfileMetadata>,
    pub harness: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Explicit vendor; otherwise inferred from the effective model's provider prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// Next profile in the finite escalation chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalate_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Extra arguments appended to `{model_args}`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

/// How a harness passes model and effort on its command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagMap {
    pub model_flag: Option<String>,
    pub effort_flag: Option<String>,
    /// Accepted effort levels; empty with an effort flag means any value is passed through.
    pub efforts: Vec<String>,
}

const PI_EFFORTS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
const OMP_EFFORTS: [&str; 8] = [
    "off", "minimal", "low", "medium", "high", "xhigh", "max", "auto",
];
const CLAUDE_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// Names of the built-in flag maps.
pub const BUILTIN_FLAG_MAPS: [&str; 4] = ["omo", "omp", "claude", "generic"];

impl FlagMap {
    /// Built-in flag map by name: omo `--model X --thinking Y` (off..max), omp the same plus "auto",
    /// claude `--model X --effort Y` (low..max), generic none.
    pub fn builtin(name: &str) -> Option<FlagMap> {
        let (model, effort, levels): (Option<&str>, Option<&str>, &[&str]) = match name {
            "omo" => (Some("--model"), Some("--thinking"), &PI_EFFORTS),
            "omp" => (Some("--model"), Some("--thinking"), &OMP_EFFORTS),
            "claude" => (Some("--model"), Some("--effort"), &CLAUDE_EFFORTS),
            "generic" => (None, None, &[]),
            _ => return None,
        };
        Some(FlagMap {
            model_flag: model.map(str::to_string),
            effort_flag: effort.map(str::to_string),
            efforts: levels.iter().map(|s| s.to_string()).collect(),
        })
    }

    /// Renders argv tokens for `model` then `effort` (effort is matched case-insensitively). Errors when the
    /// harness has no flag for a requested value, or when the effort is not one of its levels (the error lists
    /// the valid levels).
    pub fn render(
        &self,
        harness: &str,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> Result<Vec<String>> {
        let mut out = Vec::new();
        if let Some(model) = model {
            let flag = self.model_flag.as_ref().ok_or_else(|| {
                anyhow!("harness '{harness}' takes no model flag; cannot set model '{model}'")
            })?;
            out.push(flag.clone());
            out.push(model.to_string());
        }
        if let Some(effort) = effort {
            let flag = self.effort_flag.as_ref().ok_or_else(|| {
                anyhow!("harness '{harness}' takes no effort flag; cannot set effort '{effort}'")
            })?;
            let level = normalize_effort(effort);
            if !self.efforts.is_empty() && !self.efforts.contains(&level) {
                bail!(
                    "unknown effort '{effort}' for harness '{harness}' (valid: {})",
                    self.efforts.join(", ")
                );
            }
            out.push(flag.clone());
            out.push(level);
        }
        Ok(out)
    }
}

fn normalize_effort(effort: &str) -> String {
    effort.trim().to_ascii_lowercase()
}

impl Harness {
    /// Flag validity does not depend on a decodable command template.
    pub(crate) fn base_flag_map(key: &str, flags: Option<&str>) -> Result<FlagMap> {
        match flags {
            Some(name) => FlagMap::builtin(name).ok_or_else(|| {
                anyhow!(
                    "harness '{key}': unknown flags '{name}' (built-in: {})",
                    BUILTIN_FLAG_MAPS.join(", ")
                )
            }),
            None => FlagMap::builtin(key)
                .or_else(|| FlagMap::builtin("generic"))
                .ok_or_else(|| anyhow!("generic flag map missing")),
        }
    }

    /// The effective flag map for this harness under config key `key`.
    pub fn flag_map(&self, key: &str) -> Result<FlagMap> {
        Self::configured_flag_map(
            key,
            self.flags.as_deref(),
            self.model_flag.as_deref(),
            self.effort_flag.as_deref(),
            &self.efforts,
        )
    }

    fn configured_flag_map(
        key: &str,
        flags: Option<&str>,
        model_flag: Option<&str>,
        effort_flag: Option<&str>,
        efforts: &[String],
    ) -> Result<FlagMap> {
        let base = Self::base_flag_map(key, flags)?;
        Ok(FlagMap {
            model_flag: model_flag.map(str::to_owned).or(base.model_flag),
            effort_flag: effort_flag.map(str::to_owned).or(base.effort_flag),
            efforts: if efforts.is_empty() {
                base.efforts
            } else {
                efforts.iter().map(|e| normalize_effort(e)).collect()
            },
        })
    }
}

/// Per-run overrides (dispatch/retry `--profile`, `--model`, `--effort`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProfileChoice {
    pub profile: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// A profile resolved for one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedProfile {
    pub profile: String,
    pub harness: String,
    /// The harness command template.
    pub command: String,
    pub model: Option<String>,
    pub vendor: Option<String>,
    /// Normalized (lowercase) effort level.
    pub effort: Option<String>,
    /// Argv rendered into `{model_args}`: model/effort flags, then the profile's extra args.
    pub model_args: Vec<String>,
}

/// Execution settings captured once when a run is admitted, never re-bound on resume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionSnapshot {
    pub resolved: ResolvedProfile,
    pub revision: Option<String>,
    pub evidence: Option<String>,
    pub instructions: String,
    /// Contents of the implementation role template, not a mutable file reference.
    pub prompt_template: Option<String>,
}

impl ExecutionSnapshot {
    pub fn new(config: &Config, resolved: ResolvedProfile) -> Self {
        let metadata = config
            .profiles
            .get(&resolved.profile)
            .and_then(|profile| profile.metadata.as_ref());
        Self {
            revision: metadata.map(|metadata| metadata.revision.clone()),
            evidence: metadata.and_then(|metadata| metadata.evidence.clone()),
            instructions: metadata
                .map_or_else(String::new, |metadata| metadata.instructions.clone()),
            resolved,
            prompt_template: None,
        }
    }
}

/// Checks the launch slot before a future adapter supplies delivery argv.
///
/// Legacy launches pass `required = false` and retain their existing templates. Enabled adapters
/// requiring delivery flags must pass `true` before launch: every `{delivery_args}` occurrence must
/// be an unquoted, unescaped, whitespace-delimited word, and at least one must exist.
/// This checks the slot's spelling, not the semantics of arbitrary shell programs.
pub fn validate_delivery_template(command: &str, required: bool) -> Result<()> {
    if !required {
        return Ok(());
    }
    const SLOT: &str = "{delivery_args}";
    let mut quote = None;
    let mut escaped = false;
    let mut comment = false;
    let mut word_start = true;
    let mut found = false;
    for (offset, ch) in command.char_indices() {
        if command[offset..].starts_with(SLOT) {
            let after = command[offset + SLOT.len()..].chars().next();
            if quote.is_some()
                || escaped
                || comment
                || !word_start
                || after.is_some_and(|c| !c.is_whitespace())
            {
                bail!("delivery requires a standalone unquoted {{delivery_args}} slot");
            }
            found = true;
        }
        if comment {
            comment = ch != '\n';
        } else if escaped {
            escaped = false;
            if ch != '\n' {
                word_start = false;
            }
        } else if ch == '\\' && quote != Some('\'') {
            escaped = true;
        } else if let Some(active) = quote {
            if ch == active {
                quote = None;
            }
            word_start = false;
        } else if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
            word_start = false;
        } else if ch == '#' && word_start {
            comment = true;
        } else {
            word_start = ch.is_whitespace();
        }
    }
    if !found {
        bail!("delivery requires a standalone unquoted {{delivery_args}} slot");
    }
    Ok(())
}

/// Infers a vendor from a provider-qualified model or a known bare model family.
pub fn infer_vendor(model: &str) -> Option<String> {
    let Some((provider, model)) = model.split_once(['/', ':']) else {
        return if model.starts_with("claude-") {
            Some("anthropic".to_string())
        } else if model.starts_with("gpt-")
            || model
                .strip_prefix('o')
                .and_then(|suffix| suffix.as_bytes().first())
                .is_some_and(u8::is_ascii_digit)
        {
            Some("openai".to_string())
        } else {
            None
        };
    };
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    Some(
        match provider {
            "anthropic-subscription" => "anthropic",
            "chatgpt-subscription" | "openai-codex" => "openai",
            other => other,
        }
        .to_string(),
    )
}

/// The implementer vendor is unknown; callers may downcast an `anyhow::Error` to this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedRunVendor {
    pub run: String,
    pub profile: String,
}

impl std::fmt::Display for UnresolvedRunVendor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cross-vendor review cannot resolve vendor for run {} (profile '{}'); set vendor or a recognizable model on its profile",
            self.run, self.profile
        )
    }
}

impl std::error::Error for UnresolvedRunVendor {}

impl Config {
    pub(crate) fn profile_problems(&self) -> Vec<ProfileProblem> {
        let mut out = Vec::new();
        for (name, profile) in &self.profiles {
            if let Some(metadata) = &profile.metadata
                && let Err(error) = metadata.validate(self, profile)
            {
                out.push(ProfileProblem {
                    key: vec!["profiles".into(), name.clone(), "metadata".into()],
                    message: format!("{error:#}"),
                    profile: None,
                    check: ProfileCheck::Independent,
                });
            }
            let harness_section = if self.harnesses.contains_key(&profile.harness) {
                "harnesses"
            } else {
                "agents"
            };
            let mut add = |section: &str, entry: &str, field: &str, message: String, check| {
                out.push(ProfileProblem {
                    key: vec![section.into(), entry.into(), field.into()],
                    message,
                    profile: Some(name.clone()),
                    check,
                });
            };
            let (command, flags) = match self.harness(&profile.harness) {
                Ok(harness) => harness,
                Err(error) => {
                    // An invalid flag map is already diagnosed on its harness.
                    if !self.harnesses.contains_key(&profile.harness) {
                        add(
                            "profiles",
                            name,
                            "harness",
                            error.to_string(),
                            ProfileCheck::Harness,
                        );
                    }
                    continue;
                }
            };
            for (field, value) in [
                ("model", profile.model.as_deref()),
                ("effort", profile.effort.as_deref()),
            ] {
                let Some(value) = value else { continue };
                let rendered = if field == "model" {
                    flags.render(&profile.harness, Some(value), None)
                } else {
                    flags.render(&profile.harness, None, Some(value))
                };
                match rendered {
                    Err(error) => add(
                        "profiles",
                        name,
                        field,
                        error.to_string(),
                        if field == "model" {
                            ProfileCheck::Model
                        } else {
                            ProfileCheck::Effort
                        },
                    ),
                    Ok(_) if !command.contains("{model_args}") => {
                        let message = format!(
                            "harness '{}' command has no {{model_args}} placeholder; {field} cannot be passed",
                            profile.harness
                        );
                        add(
                            "profiles",
                            name,
                            field,
                            message.clone(),
                            ProfileCheck::Command,
                        );
                        add(
                            harness_section,
                            &profile.harness,
                            "command",
                            message,
                            ProfileCheck::Command,
                        );
                    }
                    Ok(_) => {}
                }
            }
            if !profile.args.is_empty() && !command.contains("{model_args}") {
                let message = format!(
                    "harness '{}' command has no {{model_args}} placeholder; args cannot be passed",
                    profile.harness
                );
                add(
                    "profiles",
                    name,
                    "args",
                    message.clone(),
                    ProfileCheck::Command,
                );
                add(
                    harness_section,
                    &profile.harness,
                    "command",
                    message,
                    ProfileCheck::Command,
                );
            }
        }
        out
    }

    /// Resolves historical run metadata before falling back to its current profile.
    ///
    /// # Errors
    /// Returns `UnresolvedRunVendor` when neither the run nor its profile identifies a vendor.
    pub fn run_vendor(&self, run: &crate::Run) -> Result<String, UnresolvedRunVendor> {
        if let Some(snapshot) = &run.execution {
            return snapshot
                .resolved
                .vendor
                .clone()
                .or_else(|| snapshot.resolved.model.as_deref().and_then(infer_vendor))
                .ok_or_else(|| UnresolvedRunVendor {
                    run: run.id.clone(),
                    profile: snapshot.resolved.profile.clone(),
                });
        }
        run.vendor
            .clone()
            .or_else(|| run.model.as_deref().and_then(infer_vendor))
            .or_else(|| {
                self.profiles
                    .get(run.profile.as_deref().unwrap_or(&run.agent))
                    .and_then(|profile| {
                        profile
                            .vendor
                            .clone()
                            .or_else(|| profile.model.as_deref().and_then(infer_vendor))
                    })
            })
            .ok_or_else(|| UnresolvedRunVendor {
                run: run.id.clone(),
                profile: run.profile.as_ref().unwrap_or(&run.agent).clone(),
            })
    }

    /// Every name `resolve_profile` accepts: `[profiles]` keys plus legacy `[agents]` keys, sorted.
    pub fn profile_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .profiles
            .keys()
            .chain(self.agents.keys())
            .cloned()
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// The profile a run uses without a `--profile` override: task frontmatter `profile:` > project
    /// `default_profile` > `[defaults] profile` > "omo".
    pub fn default_profile(&self, task_profile: Option<&str>, project: Option<&str>) -> String {
        task_profile
            .map(str::to_string)
            .or_else(|| {
                project
                    .and_then(|p| self.projects.get(p))
                    .and_then(|p| p.default_profile.clone())
            })
            .or_else(|| self.defaults.profile.clone())
            .unwrap_or_else(|| FALLBACK_PROFILE.to_string())
    }

    /// Command template and flag map of a harness: `[harnesses.<key>]`, else legacy `[agents.<key>]`.
    pub fn harness(&self, key: &str) -> Result<(String, FlagMap)> {
        if let Some(h) = self.harnesses.get(key) {
            return Ok((h.command.clone(), h.flag_map(key)?));
        }
        if let Some(a) = self.agents.get(key) {
            let flags = FlagMap::builtin(key)
                .or_else(|| FlagMap::builtin("generic"))
                .ok_or_else(|| anyhow!("generic flag map missing"))?;
            return Ok((a.command.clone(), flags));
        }
        let known: Vec<&str> = self
            .harnesses
            .keys()
            .chain(self.agents.keys())
            .map(String::as_str)
            .collect();
        bail!("unknown harness '{key}' (configured: {})", known.join(", "))
    }

    /// Resolves the profile for a run. The profile is `choice.profile`, else `default_profile(..)`; model and
    /// effort are the choice's overrides, else the profile's. Errors: unknown profile (lists the configured
    /// ones), unknown harness, a value the harness cannot take, an effort outside the harness's levels, and
    /// model args for a command template without `{model_args}`.
    pub fn resolve_profile(
        &self,
        choice: &ProfileChoice,
        task_profile: Option<&str>,
        project: Option<&str>,
    ) -> Result<ResolvedProfile> {
        let name = choice
            .profile
            .clone()
            .unwrap_or_else(|| self.default_profile(task_profile, project));
        let (harness, model, effort, args) = if let Some(p) = self.profiles.get(&name) {
            (
                p.harness.clone(),
                p.model.clone(),
                p.effort.clone(),
                p.args.clone(),
            )
        } else if self.agents.contains_key(&name) {
            (name.clone(), None, None, Vec::new())
        } else {
            bail!(
                "unknown profile '{name}' (configured: {})",
                self.profile_names().join(", ")
            );
        };
        let (command, flags) = self
            .harness(&harness)
            .map_err(|e| anyhow!("profile '{name}': {e}"))?;
        let model = choice.model.clone().or(model);
        let vendor = self
            .profiles
            .get(&name)
            .and_then(|p| p.vendor.clone())
            .or_else(|| model.as_deref().and_then(infer_vendor));
        let effort = choice.effort.clone().or(effort);
        let mut model_args = flags
            .render(&harness, model.as_deref(), effort.as_deref())
            .map_err(|e| anyhow!("profile '{name}': {e}"))?;
        model_args.extend(args);
        if !model_args.is_empty() && !command.contains("{model_args}") {
            bail!(
                "profile '{name}': harness '{harness}' command has no {{model_args}} placeholder, so \
                 model/effort/args cannot be passed (command: {command})"
            );
        }
        Ok(ResolvedProfile {
            profile: name,
            harness,
            command,
            model,
            vendor,
            effort: effort.as_deref().map(normalize_effort),
            model_args,
        })
    }
}
