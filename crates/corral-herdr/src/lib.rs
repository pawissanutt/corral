//! Typed wrapper over the `herdr` CLI. Every call runs `herdr [--session <name>] <args...>` with argv
//! (no shell), parses the JSON line it prints ({"id":..,"result":{..}} or {"id":..,"error":{code,message}}),
//! and turns an error object or non-zero exit into an anyhow error "herdr <args>: <code>: <message>".
//! CLI shapes: docs/herdr-cli.md.

use std::path::{Path, PathBuf};

use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod config;
mod delivery;
mod engine;
mod recorded_delivery;
mod session;
pub mod work;
pub use config::config_path;
pub use delivery::{Delivery, DeliveryVia, ENTER_ATTEMPTS};
pub use engine::{ENGINE_DOWN, EngineProblem, INSTALL_HERDR, MIN_HERDR_VERSION, friendly_error};
pub use recorded_delivery::TextDeliveryState;

/// herdr's semantic agent state ("idle", "working", "blocked", "done", "unknown").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

impl AgentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentStatus::Idle => "idle",
            AgentStatus::Working => "working",
            AgentStatus::Blocked => "blocked",
            AgentStatus::Done => "done",
            AgentStatus::Unknown => "unknown",
        }
    }

    /// Unrecognized strings map to Unknown.
    pub fn parse(s: &str) -> AgentStatus {
        match s {
            "idle" => AgentStatus::Idle,
            "working" => AgentStatus::Working,
            "blocked" => AgentStatus::Blocked,
            "done" => AgentStatus::Done,
            _ => AgentStatus::Unknown,
        }
    }
}

/// A pane as reported by `pane list/get` or `agent list/get`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneInfo {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    /// Detected agent label ("pi", "claude", "omp", ...) when herdr detected one.
    pub agent: Option<String>,
    pub status: AgentStatus,
    /// `title` field, else `terminal_title_stripped`.
    pub title: Option<String>,
    pub cwd: Option<PathBuf>,
}

/// Result of creating a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub workspace_id: String,
    pub tab_id: String,
    pub pane_id: String,
}

/// `pane process-info` reports members of the terminal foreground process group.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ForegroundProcess {
    pub pid: u32,
    /// Herdr omits arguments when process metadata is unavailable.
    #[serde(default)]
    pub argv: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProcessInfo {
    pub shell_pid: u32,
    pub foreground_processes: Vec<ForegroundProcess>,
}

#[derive(Debug, Clone)]
pub struct Herdr {
    bin: PathBuf,
    session: Option<String>,
    poll_wait: fn(Duration),
}

impl Herdr {
    /// Binary: $HERDR_BIN_PATH if set, else "herdr" (PATH lookup). `session` adds
    /// `--session <name>` before every subcommand (the isolated test server uses "corral-test").
    pub fn new(session: Option<String>) -> Herdr {
        let bin = std::env::var_os("HERDR_BIN_PATH")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("herdr"));
        Herdr {
            bin,
            session,
            poll_wait: std::thread::sleep,
        }
    }

    /// Like `new` with an explicit binary (tests point this at a fake herdr script).
    pub fn with_bin(bin: PathBuf, session: Option<String>) -> Herdr {
        Herdr {
            bin,
            session,
            poll_wait: std::thread::sleep,
        }
    }

    /// Replaces delivery polling/backoff waits, allowing deterministic fake-herdr tests
    /// through the public API. Production defaults to `std::thread::sleep`.
    pub fn with_poll_wait(mut self, wait: fn(Duration)) -> Self {
        self.poll_wait = wait;
        self
    }

    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    /// The herdr executable this client runs.
    pub fn bin(&self) -> &Path {
        &self.bin
    }

    /// Retargets config-driven requests without replacing the selected executable.
    pub fn set_session(&mut self, session: Option<String>) {
        self.session = session;
    }

    /// True when `workspace list` succeeds.
    pub fn available(&self) -> bool {
        self.call(&["workspace", "list"]).is_ok()
    }

    /// `workspace create --cwd <cwd> --label <label>` (plus `--focus`/no-focus per docs/herdr-cli.md).
    pub fn workspace_create(&self, cwd: &Path, label: &str, focus: bool) -> Result<Created> {
        let cwd_s = cwd.to_string_lossy();
        let flag = if focus { "--focus" } else { "--no-focus" };
        let v = self.call(&[
            "workspace",
            "create",
            "--cwd",
            &cwd_s,
            "--label",
            label,
            flag,
        ])?;
        parse_created(&v)
    }

    /// `tab create --workspace <id> --cwd <cwd> --label <label>` (plus `--focus`/`--no-focus`); returns the new
    /// tab's root pane.
    pub fn tab_create(
        &self,
        workspace_id: &str,
        cwd: &Path,
        label: &str,
        focus: bool,
    ) -> Result<Created> {
        let cwd_s = cwd.to_string_lossy();
        let flag = if focus { "--focus" } else { "--no-focus" };
        let v = self.call(&[
            "tab",
            "create",
            "--workspace",
            workspace_id,
            "--cwd",
            &cwd_s,
            "--label",
            label,
            flag,
        ])?;
        parse_created(&v)
    }

    /// (workspace_id, label) for every workspace.
    pub fn workspace_list(&self) -> Result<Vec<(String, String)>> {
        let v = self.call(&["workspace", "list"])?;
        Ok(parse_workspaces(&v))
    }

    pub fn workspace_focus(&self, workspace_id: &str) -> Result<()> {
        self.call(&["workspace", "focus", workspace_id]).map(|_| ())
    }

    pub fn workspace_close(&self, workspace_id: &str) -> Result<()> {
        self.call(&["workspace", "close", workspace_id]).map(|_| ())
    }

    pub fn pane_list(&self) -> Result<Vec<PaneInfo>> {
        let v = self.call(&["pane", "list"])?;
        v.get("panes")
            .and_then(Value::as_array)
            .context("herdr pane list: missing panes inventory")?
            .iter()
            .map(|pane| parse_pane(pane).context("herdr pane list: pane has no pane_id"))
            .collect()
    }

    pub fn pane_process_info(&self, pane_id: &str) -> Result<ProcessInfo> {
        let result = self.call(&["pane", "process-info", "--pane", pane_id])?;
        serde_json::from_value(
            result
                .get("process_info")
                .cloned()
                .context("herdr process-info: missing process_info")?,
        )
        .context("parsing herdr process-info")
    }

    /// None when herdr reports the pane does not exist.
    pub fn pane_get(&self, pane_id: &str) -> Result<Option<PaneInfo>> {
        match self.call(&["pane", "get", pane_id]) {
            Ok(v) => Ok(v
                .get("pane")
                .and_then(parse_pane)
                .or_else(|| parse_pane(&v))),
            Err(e) if error_code(&e).is_some_and(|c| c.contains("not_found")) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Types `command` into the pane's shell and presses Enter (`pane run`).
    pub fn pane_run(&self, pane_id: &str, command: &str) -> Result<()> {
        self.call(&["pane", "run", pane_id, command]).map(|_| ())
    }

    /// Reuses matching pending input or pastes once, verifies it is visible, and optionally
    /// submits with bounded Enter retries. Inspect `Delivery::submitted`, not just `Result`.
    pub fn pane_send_text(&self, pane_id: &str, text: &str, submit: bool) -> Result<Delivery> {
        let mut wait = self.poll_wait;
        Ok(self.send_text_with_wait(pane_id, text, submit, &mut wait))
    }

    /// Last `lines` lines of output (source recent-unwrapped), as plain text.
    pub fn pane_read(&self, pane_id: &str, lines: usize) -> Result<String> {
        let n = lines.to_string();
        let out = self.raw(&[
            "pane",
            "read",
            pane_id,
            "--source",
            "recent-unwrapped",
            "--lines",
            &n,
        ])?;
        Ok(extract_read_text(&out))
    }

    /// Read a capture source that may have closed since its last observation.
    pub fn pane_read_if_present(&self, pane_id: &str, lines: usize) -> Result<Option<String>> {
        match self.pane_read(pane_id, lines) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error_code(&error) == Some("pane_not_found") => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Focuses the pane's workspace, then the pane.
    pub fn pane_focus(&self, pane_id: &str) -> Result<()> {
        let pane = self.pane_get(pane_id)?.ok_or_else(|| {
            anyhow!("herdr pane focus {pane_id}: pane_not_found: pane {pane_id} not found")
        })?;
        if pane.workspace_id.is_empty() {
            return self.call(&["agent", "focus", pane_id]).map(|_| ());
        }
        self.workspace_focus(&pane.workspace_id)?;
        // herdr has no plain `pane focus`; `agent focus` refuses panes it does not see as agents.
        // The workspace is already focused, and corral runs use one pane per workspace.
        let _ = self.call(&["agent", "focus", pane_id]);
        Ok(())
    }

    pub fn pane_close(&self, pane_id: &str) -> Result<()> {
        self.call(&["pane", "close", pane_id]).map(|_| ())
    }

    /// Display metadata: `title` plus named tokens (e.g. [("task","T-042")]) via `pane report-metadata`
    /// with source "custom:corral".
    pub fn pane_report_metadata(
        &self,
        pane_id: &str,
        title: &str,
        tokens: &[(&str, &str)],
    ) -> Result<()> {
        let mut args: Vec<String> = [
            "pane",
            "report-metadata",
            pane_id,
            "--source",
            "custom:corral",
            "--title",
            title,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for (k, v) in tokens {
            args.push("--token".into());
            args.push(format!("{k}={v}"));
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.call(&refs).map(|_| ())
    }

    /// Reports semantic agent state for a pane via `pane report-agent` (source "custom:<agent>").
    /// Used by fake agents in tests and by adapters for agents herdr cannot detect.
    pub fn pane_report_agent(
        &self,
        pane_id: &str,
        agent: &str,
        status: AgentStatus,
        message: Option<&str>,
    ) -> Result<()> {
        let state = match status {
            AgentStatus::Done => "idle",
            other => other.as_str(),
        };
        let source = format!("custom:{agent}");
        let mut args = vec![
            "pane",
            "report-agent",
            pane_id,
            "--source",
            &source,
            "--agent",
            agent,
            "--state",
            state,
        ];
        if let Some(m) = message {
            args.push("--message");
            args.push(m);
        }
        self.call(&args).map(|_| ())
    }

    /// Every pane herdr considers an agent, with its status.
    pub fn agent_list(&self) -> Result<Vec<PaneInfo>> {
        let v = self.call(&["agent", "list"])?;
        Ok(parse_panes(&v, "agents"))
    }

    /// Submits a prompt to the agent in the pane (`agent prompt`). When herdr refuses because no agent is
    /// detected there, falls back to `pane_send_text(pane, text, true)`.
    pub fn agent_prompt(&self, pane_id: &str, text: &str) -> Result<Delivery> {
        let mut wait = self.poll_wait;
        Ok(self.agent_prompt_with_wait(pane_id, text, &mut wait))
    }

    fn agent_prompt_with_wait(
        &self,
        pane_id: &str,
        text: &str,
        wait: &mut impl FnMut(Duration),
    ) -> Delivery {
        // A resend can arrive while a previous paste is still pending. A native
        // agent prompt may itself paste, so reuse known pending input before it.
        // This preflight is optional for the native transport; the fallback
        // performs its own mandatory read before any terminal write.
        if let Ok(screen) = self.pane_read(pane_id, delivery::READ_LINES)
            && (delivery::input_contains(&screen, text) || delivery::input_is_collapsed(&screen))
            && !text.trim().is_empty()
        {
            return self.send_text_with_wait(pane_id, text, true, wait);
        }
        match self.call(&["agent", "prompt", pane_id, text]) {
            Ok(_) => Delivery {
                via: DeliveryVia::AgentPrompt,
                submitted: true,
                attempts: 1,
                error: None,
            },
            Err(e) if error_code(&e).is_some_and(is_no_agent_code) => {
                self.send_text_with_wait(pane_id, text, true, wait)
            }
            Err(e) => Delivery {
                via: DeliveryVia::AgentPrompt,
                submitted: false,
                attempts: 1,
                error: Some(format!("{e:#}")),
            },
        }
    }

    /// `notification show <title> --body <body>` with `--sound request` when `request_sound`.
    /// A result with shown=false is not an error.
    pub fn notify(&self, title: &str, body: &str, request_sound: bool) -> Result<()> {
        let mut args = vec!["notification", "show", title, "--body", body];
        if request_sound {
            args.push("--sound");
            args.push("request");
        }
        self.call(&args).map(|_| ())
    }

    /// `plugin link <dir>`; returns herdr's JSON result text (includes any warnings).
    pub fn plugin_link(&self, dir: &Path) -> Result<String> {
        let d = dir.to_string_lossy();
        let v = self.call(&["plugin", "link", &d])?;
        Ok(v.to_string())
    }
}

#[derive(Debug)]
struct HerdrError {
    args: String,
    code: String,
    message: String,
}

impl std::fmt::Display for HerdrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "herdr {}: {}: {}", self.args, self.code, self.message)
    }
}

impl std::error::Error for HerdrError {}

fn error_code(e: &anyhow::Error) -> Option<&str> {
    e.downcast_ref::<HerdrError>().map(|h| h.code.as_str())
}

/// Codes meaning "this pane has no agent that can take a prompt" (including an already-blocked agent,
/// and agent_not_ready: OmO panes labelled by herdr-omo-agent-state are not active named agents).
fn is_no_agent_code(code: &str) -> bool {
    code.contains("agent_blocked")
        || code.contains("agent_not_ready")
        || code.contains("no_agent")
        || code.contains("not_an_agent")
        || code.contains("agent_not_found")
        || code.contains("not_agent")
}

fn find_error(text: &str) -> Option<(String, String)> {
    let v: Value = serde_json::from_str(text.trim()).ok()?;
    let e = v.get("error")?;
    let code = e
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("error")
        .to_string();
    let msg = e
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Some((code, msg))
}

fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn parse_pane(v: &Value) -> Option<PaneInfo> {
    let pane_id = s(v, "pane_id")?;
    Some(PaneInfo {
        pane_id,
        workspace_id: s(v, "workspace_id").unwrap_or_default(),
        tab_id: s(v, "tab_id").unwrap_or_default(),
        agent: s(v, "agent"),
        status: s(v, "agent_status")
            .map(|x| AgentStatus::parse(&x))
            .unwrap_or(AgentStatus::Unknown),
        title: s(v, "title").or_else(|| s(v, "terminal_title_stripped")),
        cwd: s(v, "cwd").map(PathBuf::from),
    })
}

fn parse_panes(result: &Value, key: &str) -> Vec<PaneInfo> {
    result
        .get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(parse_pane).collect())
        .unwrap_or_default()
}

fn parse_workspaces(result: &Value) -> Vec<(String, String)> {
    result
        .get("workspaces")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|w| Some((s(w, "workspace_id")?, s(w, "label").unwrap_or_default())))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_created(result: &Value) -> Result<Created> {
    let pane = result
        .get("root_pane")
        .context("workspace create: missing root_pane")?;
    let workspace_id = s(result.get("workspace").unwrap_or(pane), "workspace_id")
        .or_else(|| s(pane, "workspace_id"))
        .context("workspace create: missing workspace_id")?;
    Ok(Created {
        workspace_id,
        tab_id: s(pane, "tab_id").context("workspace create: missing tab_id")?,
        pane_id: s(pane, "pane_id").context("workspace create: missing pane_id")?,
    })
}

/// `pane read` prints plain text; accept a JSON-wrapped result too.
fn extract_read_text(out: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(out.trim())
        && let Some(r) = v.get("result")
    {
        for key in ["text", "output", "content"] {
            if let Some(t) = r.get(key).and_then(Value::as_str) {
                return t.to_string();
            }
            if let Some(t) = r
                .get("read")
                .and_then(|x| x.get(key))
                .and_then(Value::as_str)
            {
                return t.to_string();
            }
        }
    }
    out.to_string()
}

impl Herdr {
    fn full_args<'a>(&'a self, args: &[&'a str]) -> Vec<&'a str> {
        let mut v = Vec::with_capacity(args.len() + 2);
        if let Some(sess) = &self.session {
            v.push("--session");
            v.push(sess.as_str());
        }
        v.extend_from_slice(args);
        v
    }

    /// Runs herdr, returns stdout on success; error objects / non-zero exit become anyhow errors.
    fn raw(&self, args: &[&str]) -> Result<String> {
        work::record();
        let label = args.join(" ");
        let out = Command::new(&self.bin)
            .args(self.full_args(args))
            .stdin(Stdio::null())
            .output()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    anyhow::Error::new(EngineProblem::Missing)
                } else {
                    anyhow::Error::new(error).context(format!("running herdr {label}"))
                }
            })?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let err = find_error(&stdout).or_else(|| find_error(&stderr));
        if let Some((code, message)) = err {
            return Err(HerdrError {
                args: label,
                code,
                message,
            }
            .into());
        }
        if !out.status.success() {
            let message = if stderr.trim().is_empty() {
                stdout.trim().to_string()
            } else {
                stderr.trim().to_string()
            };
            return Err(HerdrError {
                args: label,
                code: format!("exit_{}", out.status.code().unwrap_or(-1)),
                message,
            }
            .into());
        }
        Ok(stdout)
    }

    /// Runs herdr and returns the "result" object (Null when stdout is empty or not JSON).
    fn call(&self, args: &[&str]) -> Result<Value> {
        let out = self.raw(args)?;
        let v: Value = serde_json::from_str(out.trim()).unwrap_or(Value::Null);
        Ok(v.get("result").cloned().unwrap_or(v))
    }
}

#[cfg(test)]
mod tests;
