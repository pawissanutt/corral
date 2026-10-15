//! Harnesses (how to launch an agent CLI) and profiles (named harness + model + effort presets).
//!
//! A profile name resolves through `[profiles.<name>]`, then the legacy `[agents.<name>]` table (a legacy
//! agent is a profile with no model/effort whose harness is the agent itself).

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::Config;

/// Profile used when nothing else names one.
pub const FALLBACK_PROFILE: &str = "omo";

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

/// A named preset: which harness, which model, which effort, plus extra argv.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
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
    /// The effective flag map for this harness under config key `key`.
    pub fn flag_map(&self, key: &str) -> Result<FlagMap> {
        let base = match &self.flags {
            Some(name) => FlagMap::builtin(name).ok_or_else(|| {
                anyhow!(
                    "harness '{key}': unknown flags '{name}' (built-in: {})",
                    BUILTIN_FLAG_MAPS.join(", ")
                )
            })?,
            None => FlagMap::builtin(key)
                .or_else(|| FlagMap::builtin("generic"))
                .ok_or_else(|| anyhow!("generic flag map missing"))?,
        };
        Ok(FlagMap {
            model_flag: self.model_flag.clone().or(base.model_flag),
            effort_flag: self.effort_flag.clone().or(base.effort_flag),
            efforts: if self.efforts.is_empty() {
                base.efforts
            } else {
                self.efforts.iter().map(|e| normalize_effort(e)).collect()
            },
        })
    }
}

/// Per-run overrides (dispatch/retry `--profile`, `--model`, `--effort`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileChoice {
    pub profile: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// A profile resolved for one run.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Infers a vendor from a provider-qualified model (`provider/model` or `provider:model`).
pub fn infer_vendor(model: &str) -> Option<String> {
    let (provider, model) = model.split_once(['/', ':'])?;
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

impl Config {
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
