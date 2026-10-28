//! Harness detection and model discovery: which configured harnesses are installed (binary on PATH +
//! `--version`), which models each can use, and which effort levels its flag map accepts.
//!
//! Probes run as argv subprocesses (never a shell) with a timeout. Results are cached in
//! `.corral/state/harnesses.json`; `harness_report(ctx, true)` re-detects. Model listing:
//! omo `--list-models` (table), omp `models --json`, claude a built-in alias list, pi-native its
//! installed host's offline catalog for logged-in providers (`pi_native`). A failed or
//! unparseable listing degrades to free-text model entry with a note, never an error.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use corral_core::{Config, PI_NATIVE_HARNESS, is_pi_native};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Ctx;

mod availability;
mod pi_native;
pub(crate) use availability::preferred_omo_models;
pub(crate) use pi_native::detect as detect_pi_native;
pub use pi_native::models_for_profile;
pub(crate) use pi_native::{catalog_selector, foreign_selector};

#[cfg(test)]
mod tests;

/// Cache file name under `.corral/state/`.
pub const HARNESS_CACHE_FILE: &str = "harnesses.json";
/// Model aliases the claude CLI accepts for `--model`.
pub const CLAUDE_MODEL_ALIASES: [&str; 3] = ["opus", "sonnet", "haiku"];

type ModelParser = fn(&str) -> Result<Vec<String>>;

const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const MODELS_TIMEOUT: Duration = Duration::from_secs(30);

/// Where a harness's model list came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    /// Listed by the harness itself; free text is still accepted.
    Listed,
    /// Built-in alias list (claude); free text is still accepted.
    Builtin,
    /// No list available: the model is typed as free text (see the harness notes for why).
    FreeText,
    /// The harness takes no model flag.
    None,
}

/// Detection result for one configured harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessInfo {
    /// Config key (`[harnesses.<key>]` or legacy `[agents.<key>]`).
    pub key: String,
    /// Program the command template launches, when it names one.
    pub program: Option<String>,
    /// Resolved executable; `None` means not installed.
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub models: Vec<String>,
    pub model_source: ModelSource,
    /// Accepted effort levels from the harness's flag map (empty with `takes_effort` = any value).
    pub efforts: Vec<String>,
    pub takes_effort: bool,
    /// Why a probe degraded (version or model listing failed), for display.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl HarnessInfo {
    pub fn installed(&self) -> bool {
        self.path.is_some()
    }
}

/// Every configured harness, as detected at `detected_at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessReport {
    pub detected_at: Timestamp,
    pub harnesses: Vec<HarnessInfo>,
}

impl HarnessReport {
    pub fn get(&self, key: &str) -> Option<&HarnessInfo> {
        self.harnesses.iter().find(|h| h.key == key)
    }
}

/// The cached report, or a fresh detection when `refresh`, when no usable cache exists, or when the
/// configured harness programs changed since it was written. Effort levels always come from the current
/// config. Errors only when the cache cannot be written.
pub fn harness_report(ctx: &Ctx, refresh: bool) -> Result<HarnessReport> {
    let config = &ctx.store.config;
    if !refresh && let Some(report) = cached_harness_report(ctx) {
        return Ok(report);
    }
    let path = ctx.store.state_dir().join(HARNESS_CACHE_FILE);
    let report = HarnessReport {
        detected_at: Timestamp::now(),
        harnesses: detect_harnesses(config),
    };
    write_cache(&path, &report)?;
    Ok(report)
}

/// The cached report when it still matches the configured harnesses (a file read, no probes), with effort
/// levels from the current config; `None` when detection has to run.
pub fn cached_harness_report(ctx: &Ctx) -> Option<HarnessReport> {
    let config = &ctx.store.config;
    let mut report = read_cache(&ctx.store.state_dir().join(HARNESS_CACHE_FILE), config)?;
    for h in &mut report.harnesses {
        apply_efforts(h, config);
    }
    Some(report)
}

/// Detects every configured harness now (in parallel; each probe has its own timeout). No caching.
pub fn detect_harnesses(config: &Config) -> Vec<HarnessInfo> {
    let keys = harness_keys(config);
    std::thread::scope(|s| {
        let handles: Vec<_> = keys
            .iter()
            .map(|k| s.spawn(move || detect_one(config, k)))
            .collect();
        handles
            .into_iter()
            .zip(&keys)
            .map(|(h, k)| {
                h.join().unwrap_or_else(|_| HarnessInfo {
                    key: k.clone(),
                    program: None,
                    path: None,
                    version: None,
                    models: Vec::new(),
                    model_source: ModelSource::FreeText,
                    efforts: Vec::new(),
                    takes_effort: false,
                    notes: vec!["detection panicked".into()],
                })
            })
            .collect()
    })
}

/// `[harnesses]` keys plus legacy `[agents]` keys, plus the built-in pi-native harness when the
/// configuration names it, sorted and deduplicated.
fn harness_keys(config: &Config) -> Vec<String> {
    let mut keys: Vec<String> = config
        .harnesses
        .keys()
        .chain(config.agents.keys())
        .cloned()
        .collect();
    if crate::agent::hooks::configured(config) {
        keys.push(PI_NATIVE_HARNESS.into());
    }
    keys.sort();
    keys.dedup();
    keys
}

fn command_template(config: &Config, key: &str) -> Option<String> {
    config
        .harnesses
        .get(key)
        .map(|h| h.command.clone())
        .or_else(|| config.agents.get(key).map(|a| a.command.clone()))
}

/// Built-in flag-map name driving model discovery: explicit `flags`, else the key itself.
fn flag_map_name(config: &Config, key: &str) -> String {
    config
        .harnesses
        .get(key)
        .and_then(|h| h.flags.clone())
        .unwrap_or_else(|| key.to_string())
}

fn apply_efforts(h: &mut HarnessInfo, config: &Config) {
    match config.harness(&h.key) {
        Ok((_, flags)) => {
            h.takes_effort = flags.effort_flag.is_some();
            h.efforts = if h.takes_effort {
                flags.efforts
            } else {
                Vec::new()
            };
        }
        Err(_) => {
            h.takes_effort = false;
            h.efforts = Vec::new();
        }
    }
}

fn detect_one(config: &Config, key: &str) -> HarnessInfo {
    if is_pi_native(key) {
        return pi_native::detect(config);
    }
    let program = command_template(config, key).and_then(|c| program_of(&c));
    let path = program.as_deref().and_then(find_executable);
    let mut info = HarnessInfo {
        key: key.to_string(),
        program,
        path,
        version: None,
        models: Vec::new(),
        model_source: ModelSource::FreeText,
        efforts: Vec::new(),
        takes_effort: false,
        notes: Vec::new(),
    };
    apply_efforts(&mut info, config);
    let takes_model = config
        .harness(key)
        .map(|(_, f)| f.model_flag.is_some())
        .unwrap_or(false);
    if let Some(exe) = info.path.clone() {
        match run_capture(&exe, &["--version"], VERSION_TIMEOUT) {
            Ok(out) => info.version = parse_version(&out),
            Err(e) => info.notes.push(format!("--version: {e:#}")),
        }
    }
    if !takes_model {
        info.model_source = ModelSource::None;
        return info;
    }
    let listing: Option<(&[&str], ModelParser)> = match flag_map_name(config, key).as_str() {
        "omo" => Some((&["--list-models"], parse_omo_models)),
        "omp" => Some((&["models", "--json"], parse_omp_models)),
        "claude" => {
            info.models = CLAUDE_MODEL_ALIASES.iter().map(|s| s.to_string()).collect();
            info.model_source = ModelSource::Builtin;
            None
        }
        _ => None,
    };
    if let (Some((args, parse)), Some(exe)) = (listing, info.path.clone()) {
        let shown = args.join(" ");
        match run_capture(&exe, args, MODELS_TIMEOUT).and_then(|out| parse(&out)) {
            Ok(models) if !models.is_empty() => {
                info.models = models;
                info.model_source = ModelSource::Listed;
            }
            Ok(_) => info.notes.push(format!("{shown}: no models listed")),
            Err(e) => info.notes.push(format!("{shown}: {e:#}")),
        }
    }
    info
}

/// The program a command template launches: its first word after any `VAR=value` assignments, unquoted.
/// `None` when that word is a placeholder or the template is empty.
pub(crate) fn program_of(command: &str) -> Option<String> {
    let word = command
        .split_whitespace()
        .find(|w| !w.contains('=') || w.starts_with(['\'', '"', '{']))?;
    let word = word.trim_matches(['\'', '"']);
    (!word.is_empty() && !word.contains('{')).then(|| word.to_string())
}

/// Resolves `program` like a shell would: a path containing `/` as-is, else the first executable match in $PATH.
pub(crate) fn find_executable(program: &str) -> Option<PathBuf> {
    if program.contains('/') {
        let p = PathBuf::from(program);
        return is_executable(&p).then_some(p);
    }
    if hermetic_blocks(program) {
        return None;
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(program))
        .find(|p| is_executable(p))
}

/// Test builds (this crate's unit tests, and every test target that enables the `hermetic-harness`
/// feature through a dev-dependency) never resolve the real agent harness binaries from `$PATH`, so model
/// discovery cannot call the owner's omp/omo/claude. Release builds are unaffected. Fixtures that need a
/// harness configure an absolute path.
const REAL_HARNESS_BINARIES: [&str; 3] = ["omp", "omo", "claude"];

fn hermetic_blocks(program: &str) -> bool {
    cfg!(any(test, feature = "hermetic-harness")) && REAL_HARNESS_BINARIES.contains(&program)
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// Runs `exe args` (argv, no shell, stdin closed, stderr discarded) and returns stdout. Kills the child and
/// errors after `timeout`; errors on a non-zero exit.
pub(crate) fn run_capture(exe: &Path, args: &[&str], timeout: Duration) -> Result<String> {
    let (status, out) = run_raw(exe, args, timeout)?;
    if !status.success() {
        bail!("exited with {status}");
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Like `run_capture`, but a non-zero exit is a result: `(exit code, stdout)`.
pub(crate) fn run_status(
    exe: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<(Option<i32>, String)> {
    let (status, out) = run_raw(exe, args, timeout)?;
    Ok((status.code(), String::from_utf8_lossy(&out).into_owned()))
}

fn run_raw(
    exe: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<(std::process::ExitStatus, Vec<u8>)> {
    let deadline = Instant::now() + timeout;
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("no stdout pipe for {}", exe.display()))?;
    let (tx, rx) = mpsc::channel();
    // Reads until EOF; detached on timeout (a lingering grandchild may hold the pipe open).
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let res = stdout.read_to_end(&mut buf).map(|_| buf);
        let _ = tx.send(res);
    });
    let out = match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(res) => res.with_context(|| format!("reading output of {}", exe.display()))?,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            bail!("timed out after {}s", timeout.as_secs());
        }
    };
    let status = loop {
        if let Some(status) = child.try_wait().context("waiting for the probe")? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("timed out after {}s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    Ok((status, out))
}

/// Version from `--version` output: the first word that starts with a digit and contains a dot (split on
/// whitespace and `/`, leading `v` dropped), else the first non-empty line. `None` for empty output.
/// "omo 5.1.33 (engine: senpi ...)" -> "5.1.33", "omp/18.4.0" -> "18.4.0", "2.1.299 (Claude Code)" -> "2.1.299".
pub(crate) fn parse_version(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let version = line
        .split(|c: char| c.is_whitespace() || c == '/')
        .map(|w| w.strip_prefix('v').unwrap_or(w))
        .map(|w| w.trim_end_matches(|c: char| !c.is_ascii_alphanumeric()))
        .find(|w| w.starts_with(|c: char| c.is_ascii_digit()) && w.contains('.'));
    Some(version.unwrap_or(line).to_string())
}

/// `omo --list-models`: a whitespace-aligned table whose header starts `provider model`; each row becomes
/// "provider/model" (the form `--model` accepts). Errors when the header is missing.
pub(crate) fn parse_omo_models(text: &str) -> Result<Vec<String>> {
    let mut lines = text.lines().skip_while(|l| {
        let mut w = l.split_whitespace();
        !(w.next() == Some("provider") && w.next() == Some("model"))
    });
    if lines.next().is_none() {
        bail!("unrecognized output (no 'provider model' header)");
    }
    Ok(lines
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            Some(format!("{}/{}", w.next()?, w.next()?))
        })
        .collect())
}

#[derive(Deserialize)]
struct OmpModels {
    models: Vec<OmpModel>,
}

#[derive(Deserialize)]
struct OmpModel {
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    id: Option<String>,
}

/// `omp models --json`: `{"models": [{"selector": "provider/id", ...}]}`; falls back to "provider/id" when a
/// model has no selector. Errors on JSON that does not have that shape.
pub(crate) fn parse_omp_models(text: &str) -> Result<Vec<String>> {
    let parsed: OmpModels = serde_json::from_str(text.trim()).context("parsing models JSON")?;
    Ok(parsed
        .models
        .into_iter()
        .filter_map(|m| match (m.selector, m.provider, m.id) {
            (Some(s), _, _) if !s.is_empty() => Some(s),
            (_, Some(p), Some(i)) => Some(format!("{p}/{i}")),
            (_, None, Some(i)) => Some(i),
            _ => None,
        })
        .collect())
}

/// The cached report when it parses and still covers exactly the configured harnesses and programs.
fn read_cache(path: &Path, config: &Config) -> Option<HarnessReport> {
    let text = std::fs::read_to_string(path).ok()?;
    let report: HarnessReport = serde_json::from_str(&text).ok()?;
    let cached: Vec<(&str, Option<String>)> = report
        .harnesses
        .iter()
        .map(|h| (h.key.as_str(), h.program.clone()))
        .collect();
    let keys = harness_keys(config);
    let current: Vec<(&str, Option<String>)> = keys
        .iter()
        .map(|k| {
            (
                k.as_str(),
                command_template(config, k).and_then(|c| program_of(&c)),
            )
        })
        .collect();
    (cached == current).then_some(report)
}

fn write_cache(path: &Path, report: &HarnessReport) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("cache path {} has no parent", path.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let json = serde_json::to_string_pretty(report).context("serializing the harness report")?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json + "\n").with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}
