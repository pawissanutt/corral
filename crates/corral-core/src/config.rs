use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use jiff::SignedDuration;
use serde::{Deserialize, Serialize};

use crate::{Harness, Price, Profile, ProfileChoice, ReviewOverride, validate_glob};

/// Every problem [`Config::validate`] found; displays one per line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigErrors(pub Vec<String>);

impl fmt::Display for ConfigErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.0.len();
        write!(
            f,
            "invalid config ({n} problem{}):",
            if n == 1 { "" } else { "s" }
        )?;
        for p in &self.0 {
            write!(f, "\n  - {p}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigErrors {}

/// `.corral/corral.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Model ID -> USD per million tokens; omitted in existing configs unless explicitly added.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pricing: BTreeMap<String, Price>,
    #[serde(default)]
    pub limits: Limits,
    /// Project key -> project. Keys are what tasks reference in `project:`.
    #[serde(default)]
    pub projects: BTreeMap<String, Project>,
    #[serde(default, skip_serializing_if = "Defaults::is_empty")]
    pub defaults: Defaults,
    #[serde(default, skip_serializing_if = "crate::Routing::is_empty")]
    pub routing: crate::Routing,
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
    #[serde(default)]
    pub ui: UiCfg,
    /// `[viewers]`: file extension (no dot) -> command template that opens it (`{path}` absolute path, `{url}`
    /// file:// URL; split into argv without a shell), or "system" for the platform opener. Unlisted extensions
    /// use the built-in routing (`corral_ops::viewers`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub viewers: BTreeMap<String, String>,
}

/// `[ui]`: terminal UI preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiCfg {
    /// Capture the mouse (clicks, wheel, drag). Default true; `--no-mouse` and `CORRAL_MOUSE=0` also turn it off.
    #[serde(default = "yes")]
    pub mouse: bool,
    /// How far back the board's Done column reaches, as N h/d/w (default "7d"); older tasks are in history.
    #[serde(default = "default_done_window")]
    pub done_window: String,
    /// At most this many cards in the board's Done column, newest first (default 10).
    #[serde(default = "default_done_max")]
    pub done_max: usize,
}

fn yes() -> bool {
    true
}

fn default_done_window() -> String {
    "7d".to_string()
}

fn default_done_max() -> usize {
    10
}

impl Default for UiCfg {
    fn default() -> Self {
        UiCfg {
            mouse: true,
            done_window: default_done_window(),
            done_max: default_done_max(),
        }
    }
}

impl UiCfg {
    /// `done_window` as a duration; an unparseable value (reported by [`Config::validate`]) counts as "7d".
    pub fn done_window(&self) -> SignedDuration {
        parse_age(&self.done_window).unwrap_or(SignedDuration::from_hours(24 * 7))
    }
}

/// Parses an age such as "36h", "7d" or "2w": a positive whole number followed by one unit, no spaces.
pub fn parse_age(s: &str) -> Result<SignedDuration> {
    let s = s.trim();
    let split = s.char_indices().next_back().map_or(0, |(i, _)| i);
    let (n, unit) = s.split_at(split);
    let unit_secs: i64 = match unit {
        "h" => 3600,
        "d" => 86_400,
        "w" => 604_800,
        _ => bail!("{s:?} is not an age: use N h/d/w without spaces (e.g. 7d)"),
    };
    let n: i64 = n
        .parse()
        .with_context(|| format!("{s:?} is not an age: N must be a whole number"))?;
    if n <= 0 {
        bail!("{s:?} is not an age: N must be positive");
    }
    let secs = n
        .checked_mul(unit_secs)
        .with_context(|| format!("{s:?} is too large"))?;
    Ok(SignedDuration::from_secs(secs))
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
    /// Opt in to actionable event delivery to the live workspace PM pane.
    #[serde(default)]
    pub push_events: bool,
    /// Actionable event kinds delivered to the PM when push is enabled.
    #[serde(default = "default_push_kinds")]
    pub push_kinds: Vec<String>,
}

fn default_push_kinds() -> Vec<String> {
    ["done", "needs_me", "stuck", "blocked", "escalated"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

impl Default for PmCfg {
    fn default() -> Self {
        PmCfg {
            agent: "omo".into(),
            push_events: false,
            push_kinds: default_push_kinds(),
        }
    }
}

/// `[sync]`: how sync reads agent activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncCfg {
    /// Seconds of raw herdr idle and foreign process lifetime before the stuck warning. Default 300.
    #[serde(default = "default_stuck_after_secs")]
    pub stuck_after_secs: u64,
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

fn default_stuck_after_secs() -> u64 {
    300
}

impl Default for SyncCfg {
    fn default() -> Self {
        SyncCfg {
            stuck_after_secs: default_stuck_after_secs(),
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
                    prompt: None,
                },
            );
            profiles.insert(
                k.to_string(),
                Profile {
                    harness: k.to_string(),
                    model: None,
                    vendor: None,
                    escalate_to: None,
                    effort: None,
                    args: Vec::new(),
                },
            );
        }
        Config {
            pricing: BTreeMap::new(),
            limits: Limits::default(),
            projects,
            defaults: Defaults::default(),
            routing: crate::Routing::default(),
            harnesses,
            profiles,
            agents: BTreeMap::new(),
            artifacts: ArtifactCfg::default(),
            herdr: HerdrCfg::default(),
            pm: PmCfg::default(),
            review: ReviewOverride::default(),
            sync: SyncCfg::default(),
            ui: UiCfg::default(),
            viewers: BTreeMap::new(),
        }
    }

    /// Serializes to TOML (round-trips through `load`).
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// Checks the config against the workspace at `root` and reports every problem: limits must be > 0,
    /// per-project limits must name projects, project paths must be directories, profile/harness/reviewer
    /// references must resolve, profile model/effort must suit the harness, globs must parse.
    pub fn validate(&self, root: &Path) -> std::result::Result<(), ConfigErrors> {
        let problems = self.problems(root);
        if problems.is_empty() {
            Ok(())
        } else {
            Err(ConfigErrors(problems))
        }
    }

    fn problems(&self, root: &Path) -> Vec<String> {
        let mut out = Vec::new();
        if self.limits.running == 0 {
            out.push("limits.running must be > 0".to_string());
        }
        for (k, n) in &self.limits.per_project {
            if !self.projects.contains_key(k) {
                out.push(format!("limits.per_project.{k}: unknown project '{k}'"));
            }
            if *n == 0 {
                out.push(format!("limits.per_project.{k} must be > 0"));
            }
        }
        let profiles = self.profile_names();
        let check_profile = |out: &mut Vec<String>, key: String, name: &str| {
            if !profiles.iter().any(|p| p == name) {
                out.push(format!(
                    "{key}: unknown profile '{name}' (configured: {})",
                    profiles.join(", ")
                ));
            }
        };
        for (k, p) in &self.projects {
            let path = Path::new(&p.path);
            if p.path.trim().is_empty() {
                out.push(format!("projects.{k}.path is empty"));
            } else if !root.join(path).is_dir() {
                out.push(format!(
                    "projects.{k}.path: {} is not a directory",
                    root.join(path).display()
                ));
            }
            if let Some(name) = &p.default_profile {
                check_profile(&mut out, format!("projects.{k}.default_profile"), name);
            }
        }
        if let Some(name) = &self.defaults.profile {
            check_profile(&mut out, "defaults.profile".into(), name);
        }
        check_profile(&mut out, "pm.agent".into(), &self.pm.agent);
        for kind in &self.pm.push_kinds {
            if !matches!(
                kind.as_str(),
                "done" | "needs_me" | "stuck" | "blocked" | "escalated"
            ) {
                out.push(format!(
                    "pm.push_kinds: unknown actionable kind {kind:?}; use done, needs_me, stuck, blocked, escalated"
                ));
            }
        }
        for (k, h) in &self.harnesses {
            if h.command.trim().is_empty() {
                out.push(format!("harnesses.{k}.command is empty"));
            }
            if let Err(e) = h.flag_map(k) {
                out.push(format!("harnesses.{k}: {e:#}"));
            }
        }
        for (k, a) in &self.agents {
            if a.command.trim().is_empty() {
                out.push(format!("agents.{k}.command is empty"));
            }
        }
        for name in self.profiles.keys() {
            if let Some(target) = &self.profiles[name].escalate_to {
                check_profile(&mut out, format!("profiles.{name}.escalate_to"), target);
            }
            let mut seen = std::collections::BTreeSet::new();
            let mut current = name.as_str();
            loop {
                if !seen.insert(current) {
                    out.push(format!(
                        "profiles.{name}.escalate_to: escalation cycle at '{current}'"
                    ));
                    break;
                }
                match self
                    .profiles
                    .get(current)
                    .and_then(|p| p.escalate_to.as_deref())
                {
                    Some(next) => current = next,
                    None => break,
                }
            }
            let choice = ProfileChoice {
                profile: Some(name.clone()),
                ..ProfileChoice::default()
            };
            if let Err(e) = self.resolve_profile(&choice, None, None) {
                out.push(format!("profiles.{name}: {e:#}"));
            }
        }
        for glob in &self.artifacts.globs {
            if let Err(e) = validate_glob(glob) {
                out.push(format!("artifacts.globs: {e:#}"));
            }
        }
        if self.artifacts.max_bytes == 0 {
            out.push("artifacts.max_bytes must be > 0".to_string());
        }
        if let Err(e) = parse_age(&self.ui.done_window) {
            out.push(format!("ui.done_window: {e:#}"));
        }
        out.extend(self.review_problems());
        out.extend(self.pricing_problems());
        out
    }
}
