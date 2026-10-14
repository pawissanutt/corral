use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// `.corral/corral.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub limits: Limits,
    /// Project key -> project. Keys are what tasks reference in `project:`.
    #[serde(default)]
    pub projects: BTreeMap<String, Project>,
    /// Agent key -> launch template.
    #[serde(default)]
    pub agents: BTreeMap<String, AgentCmd>,
    #[serde(default)]
    pub artifacts: ArtifactCfg,
    #[serde(default)]
    pub herdr: HerdrCfg,
    #[serde(default)]
    pub pm: PmCfg,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_agent: Option<String>,
    /// Base branch for new runs; default: the repo's current branch at dispatch time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

/// A shell command template sent to the run's herdr pane. Placeholders:
/// {prompt_file} absolute path of the rendered prompt, {name} "T-042 title", {task} task id,
/// {worktree} absolute worktree path, {corral} absolute path of the corral binary.
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
    /// Agent key used for the workspace PM pane. Default "omo".
    pub agent: String,
}

impl Default for PmCfg {
    fn default() -> Self {
        PmCfg {
            agent: "omo".into(),
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
    /// Default agents (exact strings):
    ///   omo     = omo -n '{name}' "$(cat '{prompt_file}')"
    ///   omp     = omp "$(cat '{prompt_file}')"
    ///   claude  = claude "$(cat '{prompt_file}')"
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
                            default_agent: None,
                            base: None,
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
                    default_agent: None,
                    base: None,
                },
            );
        }
        let mut agents = BTreeMap::new();
        for (k, v) in [
            ("omo", "omo -n '{name}' \"$(cat '{prompt_file}')\""),
            ("omp", "omp \"$(cat '{prompt_file}')\""),
            ("claude", "claude \"$(cat '{prompt_file}')\""),
            ("generic", "cat '{prompt_file}'"),
        ] {
            agents.insert(
                k.to_string(),
                AgentCmd {
                    command: v.to_string(),
                },
            );
        }
        Config {
            limits: Limits::default(),
            projects,
            agents,
            artifacts: ArtifactCfg::default(),
            herdr: HerdrCfg::default(),
            pm: PmCfg::default(),
        }
    }

    /// Serializes to TOML (round-trips through `load`).
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }
}
