use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{Harness, Profile, ReviewOverride};

/// `.corral/corral.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub limits: Limits,
    /// Project key -> project. Keys are what tasks reference in `project:`.
    #[serde(default)]
    pub projects: BTreeMap<String, Project>,
    #[serde(default, skip_serializing_if = "Defaults::is_empty")]
    pub defaults: Defaults,
    /// Harness key -> how to launch it (command template + model/effort flag map).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub harnesses: BTreeMap<String, Harness>,
    /// Profile key -> named harness + model + effort preset.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, Profile>,
    /// Legacy agent key -> launch template; each acts as a profile (and harness) with no model/effort.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, AgentCmd>,
    #[serde(default)]
    pub artifacts: ArtifactCfg,
    #[serde(default)]
    pub herdr: HerdrCfg,
    #[serde(default)]
    pub pm: PmCfg,
    /// `[review]`: workspace-wide review policy (gate, reviewer, acceptance).
    #[serde(default, skip_serializing_if = "ReviewOverride::is_empty")]
    pub review: ReviewOverride,
    #[serde(default)]
    pub sync: SyncCfg,
}

/// `[defaults]`: workspace-wide fallbacks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Defaults {
    /// Profile for runs whose task and project name none. Unset: "omo".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

impl Defaults {
    fn is_empty(&self) -> bool {
        self.profile.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// Global run slots (Running + NeedsMe). Default 3.
    pub running: usize,
    /// Optional per-project caps.
    #[serde(default)]
    pub per_project: BTreeMap<String, usize>,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            running: 3,
            per_project: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// Path relative to the workspace root (the dir containing `.corral/`), or absolute.
    pub path: String,
    /// Profile for this project's runs (legacy key: `default_agent`).
    #[serde(
        default,
        alias = "default_agent",
        skip_serializing_if = "Option::is_none"
    )]
    pub default_profile: Option<String>,
    /// Base branch for new runs; default: the repo's current branch at dispatch time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// `[projects.<p>.review]`: overrides of the global `[review]` policy.
    #[serde(default, skip_serializing_if = "ReviewOverride::is_empty")]
    pub review: ReviewOverride,
}

/// A shell command template sent to the run's herdr pane. Placeholders:
/// {prompt_file} absolute path of the rendered prompt, {name} "T-042 title", {task} task id,
/// {worktree} absolute worktree path, {corral} absolute path of the corral binary,
/// {model_args} rendered model/effort flags and profile args (empty when unset).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCmd {
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactCfg {
    /// Globs (relative to the run worktree) scanned at `corral done` for new/changed files.
    pub globs: Vec<String>,
    /// Files larger than this are skipped by the scan and refused by `artifact add`.
    pub max_bytes: u64,
}

impl Default for ArtifactCfg {
    fn default() -> Self {
        ArtifactCfg {
            globs: vec![
                ".omo/evidence/**".into(),
                ".omo/plans/*.md".into(),
                "docs/**/records/*".into(),
                "**/*report*.html".into(),
                "**/*report*.md".into(),
            ],
            max_bytes: 20_000_000,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrCfg {
    /// Named herdr session to target (`herdr --session <name>`). None = caller's environment.
    /// The env var CORRAL_HERDR_SESSION overrides this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PmCfg {
    /// Profile key used for the workspace PM pane. Default "omo".
    pub agent: String,
}

impl Default for PmCfg {
    fn default() -> Self {
        PmCfg {
            agent: "omo".into(),
        }
    }
}

/// `[sync]`: how sync reads agent activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncCfg {
    /// Seconds a run must look idle (without `corral done`) before it gets the `idle_no_report`
    /// badge, counted from the first sync that saw it idle. Default 120.
    #[serde(default = "default_idle_after_secs")]
    pub idle_after_secs: u64,
    /// OmO agent dir holding `sessions/<cwd-slug>/`. Unset: $OMO_CODING_AGENT_DIR,
    /// $SENPI_CODING_AGENT_DIR, else ~/.omo/agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omo_agent_dir: Option<PathBuf>,
}

fn default_idle_after_secs() -> u64 {
    120
}

impl Default for SyncCfg {
    fn default() -> Self {
        SyncCfg {
            idle_after_secs: default_idle_after_secs(),
            omo_agent_dir: None,
        }
    }
}

impl Config {
    /// Reads and parses a corral.toml.
    pub fn load(path: &Path) -> Result<Config> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Default config for a workspace root: every immediate subdirectory of `root` that contains a
    /// `.git` entry becomes a project (key = dir name, path = dir name); if `root` itself is a git
    /// repo with no such children, it becomes project "<root dir name>" with path ".".
    /// Default harnesses (exact strings), each with a same-named profile (no model/effort):
    ///   omo     = omo -n '{name}' {model_args} "$(cat '{prompt_file}')"
    ///   omp     = omp {model_args} "$(cat '{prompt_file}')"
    ///   claude  = claude {model_args} "$(cat '{prompt_file}')"
    ///   generic = cat '{prompt_file}'   (shows the prompt; a human or any agent takes it from there)
    pub fn default_for(root: &Path) -> Config {
        let mut projects = BTreeMap::new();
        if let Ok(rd) = std::fs::read_dir(root) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_dir() && p.join(".git").exists() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') {
                        continue;
                    }
                    projects.insert(
                        name.clone(),
                        Project {
                            path: name,
                            default_profile: None,
                            base: None,
                            review: ReviewOverride::default(),
                        },
                    );
                }
            }
        }
        if projects.is_empty() && root.join(".git").exists() {
            let name = root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "root".to_string());
            projects.insert(
                name,
                Project {
                    path: ".".into(),
                    default_profile: None,
                    base: None,
                    review: ReviewOverride::default(),
                },
            );
        }
        let mut harnesses = BTreeMap::new();
        let mut profiles = BTreeMap::new();
        for (k, v) in [
            (
                "omo",
                "omo -n '{name}' {model_args} \"$(cat '{prompt_file}')\"",
            ),
            ("omp", "omp {model_args} \"$(cat '{prompt_file}')\""),
            ("claude", "claude {model_args} \"$(cat '{prompt_file}')\""),
            ("generic", "cat '{prompt_file}'"),
        ] {
            harnesses.insert(
                k.to_string(),
                Harness {
                    command: v.to_string(),
                    flags: None,
                    model_flag: None,
                    effort_flag: None,
                    efforts: Vec::new(),
                },
            );
            profiles.insert(
                k.to_string(),
                Profile {
                    harness: k.to_string(),
                    model: None,
                    effort: None,
                    args: Vec::new(),
                },
            );
        }
        Config {
            limits: Limits::default(),
            projects,
            defaults: Defaults::default(),
            harnesses,
            profiles,
            agents: BTreeMap::new(),
            artifacts: ArtifactCfg::default(),
            herdr: HerdrCfg::default(),
            pm: PmCfg::default(),
            review: ReviewOverride::default(),
            sync: SyncCfg::default(),
        }
    }

    /// Serializes to TOML (round-trips through `load`).
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }
}
