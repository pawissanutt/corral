//! Artifact viewers: the one routing table every "open" goes through (TUI ⏎, `corral open`, the PM).
//!
//! Routing by file extension: a `[viewers]` entry (extension -> command template, or "system") wins; otherwise
//! html/htm/svg open in a terminal-browser split beside the board when `terminal-browser` is on PATH and corral
//! runs inside herdr, and everything else goes to the system opener. The system opener is WSL-aware: `wslview`
//! when present, else `explorer.exe` with the path converted by `wslpath -w`; `xdg-open` on other Linux, `open`
//! on macOS. In-TUI previews (text, images) are `artifact_preview`'s job; this module only launches.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use corral_core::Artifact;

use crate::Ctx;

/// `[viewers]` value that forces the system opener for an extension.
pub const SYSTEM_VIEWER: &str = "system";

/// Built-in viewer for html/htm/svg inside herdr: the page opens in a split right of the board.
pub const TERMINAL_BROWSER: &str = "terminal-browser open --split right {path}";

/// How long `launch` waits for a quick failure before treating the viewer as started.
const LAUNCH_GRACE: Duration = Duration::from_millis(800);

/// Where an open goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// A command template (`{path}` absolute path, `{url}` file:// URL), split into argv without a shell.
    Template(String),
    /// The platform's default handler.
    System,
}

/// A launched viewer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub argv: Vec<String>,
}

impl Opened {
    /// The command as one line, for toasts and logs.
    pub fn command(&self) -> String {
        shown(&self.argv)
    }
}

/// Host facts and process control the router needs; [`SystemHost`] in production, a fake in tests.
pub trait Host {
    /// `std::env::consts::OS`.
    fn os(&self) -> &str;
    /// Contents of /proc/version (None when unreadable).
    fn proc_version(&self) -> Option<String>;
    fn on_path(&self, program: &str) -> bool;
    fn env(&self, key: &str) -> Option<String>;
    /// Runs `argv` to completion and returns its trimmed stdout; non-zero exit is an error.
    fn output(&self, argv: &[String]) -> Result<String>;
    /// Starts `argv` detached with null stdio. A process that fails within a short grace period is an error
    /// unless `check_status` is false; one still running after it is left to finish on its own.
    fn launch(&self, argv: &[String], check_status: bool) -> Result<()>;
}

/// The real machine.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemHost;

impl Host for SystemHost {
    fn os(&self) -> &str {
        std::env::consts::OS
    }

    fn proc_version(&self) -> Option<String> {
        std::fs::read_to_string("/proc/version").ok()
    }

    fn on_path(&self, program: &str) -> bool {
        let Some(paths) = std::env::var_os("PATH") else {
            return false;
        };
        std::env::split_paths(&paths).any(|dir| is_executable(&dir.join(program)))
    }

    fn env(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|v| !v.is_empty())
    }

    fn output(&self, argv: &[String]) -> Result<String> {
        let (prog, args) = argv.split_first().context("empty command")?;
        let out = Command::new(prog)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("running {}", shown(argv)))?;
        if !out.status.success() {
            bail!(
                "{} exited with {}: {}",
                shown(argv),
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn launch(&self, argv: &[String], check_status: bool) -> Result<()> {
        let (prog, args) = argv.split_first().context("empty command")?;
        let mut child = Command::new(prog)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("starting {}", shown(argv)))?;
        let deadline = Instant::now() + LAUNCH_GRACE;
        while Instant::now() < deadline {
            if let Some(status) = child
                .try_wait()
                .with_context(|| format!("waiting for {}", shown(argv)))?
            {
                if check_status && !status.success() {
                    bail!("{} exited with {status}", shown(argv));
                }
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // Still running (e.g. terminal-browser setting up its split): reap it in the background.
        std::thread::spawn(move || child.wait());
        Ok(())
    }
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// Lowercased extension without the dot ("" when none).
pub fn extension(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// The routing table: `viewers` override, then the built-in defaults.
pub fn route(viewers: &BTreeMap<String, String>, path: &Path, host: &dyn Host) -> Route {
    let ext = extension(path);
    let configured = viewers
        .iter()
        .find(|(k, _)| k.trim_start_matches('.').eq_ignore_ascii_case(&ext));
    if let Some((_, template)) = configured {
        return if template.trim().eq_ignore_ascii_case(SYSTEM_VIEWER) {
            Route::System
        } else {
            Route::Template(template.clone())
        };
    }
    if matches!(ext.as_str(), "html" | "htm" | "svg")
        && host.on_path("terminal-browser")
        && host.env("HERDR_PANE_ID").is_some()
    {
        return Route::Template(TERMINAL_BROWSER.to_string());
    }
    Route::System
}

/// True when /proc/version says this is WSL.
pub fn is_wsl(proc_version: &str) -> bool {
    proc_version.to_ascii_lowercase().contains("microsoft")
}

/// argv for the platform's default handler.
pub fn system_opener(path: &Path, host: &dyn Host) -> Result<Vec<String>> {
    let p = path.to_string_lossy().into_owned();
    if host.os() == "macos" {
        return Ok(vec!["open".into(), p]);
    }
    if host.proc_version().is_some_and(|v| is_wsl(&v)) {
        if host.on_path("wslview") {
            return Ok(vec!["wslview".into(), p]);
        }
        let win = host.output(&["wslpath".into(), "-w".into(), p])?;
        return Ok(vec!["explorer.exe".into(), win]);
    }
    Ok(vec!["xdg-open".into(), p])
}

/// Splits `template` into words (whitespace-separated; '...' literal, "..." with \" and \\ escapes) and
/// substitutes `{path}` and `{url}` inside each word, so a path with spaces stays one argument.
pub fn render_template(template: &str, path: &Path) -> Result<Vec<String>> {
    let p = path.to_string_lossy();
    let url = file_url(path);
    let argv: Vec<String> = split_words(template)?
        .into_iter()
        .map(|w| w.replace("{path}", &p).replace("{url}", &url))
        .collect();
    if argv.is_empty() {
        bail!("viewer template is empty");
    }
    Ok(argv)
}

fn split_words(s: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => cur.push(ch),
                        None => bail!("unterminated ' in viewer template: {s}"),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(e @ ('"' | '\\')) => cur.push(e),
                            Some(other) => {
                                cur.push('\\');
                                cur.push(other);
                            }
                            None => bail!("unterminated \" in viewer template: {s}"),
                        },
                        Some(ch) => cur.push(ch),
                        None => bail!("unterminated \" in viewer template: {s}"),
                    }
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

/// file:// URL for an absolute path, percent-encoding everything but unreserved characters and '/'.
pub fn file_url(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Routes and launches `path` against an explicit viewer table and host. Errors name the command that failed.
pub fn open_with(
    viewers: &BTreeMap<String, String>,
    path: &Path,
    host: &dyn Host,
) -> Result<Opened> {
    let argv = match route(viewers, path, host) {
        Route::Template(t) => render_template(&t, path)?,
        Route::System => system_opener(path, host)?,
    };
    // explorer.exe exits 1 even after it opened the file, so its status says nothing.
    let check = argv.first().is_none_or(|p| p != "explorer.exe");
    host.launch(&argv, check)
        .with_context(|| format!("could not open with `{}`", shown(&argv)))?;
    Ok(Opened { argv })
}

/// Opens `path` with the workspace's `[viewers]` routing on this machine.
pub fn open_path(ctx: &Ctx, path: &Path) -> Result<Opened> {
    open_with(&ctx.store.config.viewers, path, &SystemHost)
}

/// Opens an artifact's snapshot (see [`open_path`]).
pub fn open_artifact(ctx: &Ctx, artifact: &Artifact) -> Result<Opened> {
    open_path(ctx, &ctx.store.dir().join(&artifact.path))
}

fn shown(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if a.is_empty()
                || a.chars()
                    .any(|c| c.is_whitespace() || "'\"\\$`".contains(c))
            {
                format!("'{}'", a.replace('\'', r"'\''"))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
#[path = "viewers_tests.rs"]
mod tests;
