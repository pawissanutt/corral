//! Read-only legacy migration liveness adapter. Never opens a Store or reconciles owner data.
use anyhow::{Context, Result, ensure};
use corral_core::{Task, blob::ProcessIdentity, migrate::RuntimeProbe};
use corral_herdr::Herdr;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn migration_runtime_probe(root: &Path) -> Result<RuntimeProbe> {
    let config = corral_core::Config::load(&root.join(".corral/corral.toml"))?;
    let session = std::env::var("CORRAL_HERDR_SESSION")
        .ok()
        .filter(|s| !s.is_empty())
        .or(config.herdr.session)
        .or_else(|| {
            let inherited = std::env::var("HERDR_ENV").as_deref() == Ok("1")
                || ["HERDR_SOCKET_PATH", "HERDR_SESSION"]
                    .iter()
                    .any(|n| std::env::var_os(n).is_some_and(|v| !v.is_empty()));
            (!inherited).then(|| "corral".into())
        });
    let herdr = Herdr::new(session);
    let root = root.to_path_buf();
    Ok(Arc::new(move |task| {
        let selected = herdr.socket_path()?;
        let binding = match std::fs::read(root.join(".corral/herdr-session.json")) {
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes)?["socket"]
                .as_str()
                .map(PathBuf::from),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e).context("reading legacy session binding"),
        };
        let doc = serde_json::to_value(task)?;
        let mut handles = Handles::default();
        handles.collect(&doc, binding.as_deref());
        // A successful inventory from the *owning* socket is mandatory, even for a wait or
        // old aggregate gate that recorded no pane. Wrong/unavailable servers prove nothing.
        let panes: BTreeSet<_> = herdr.pane_list()?.into_iter().map(|p| p.pane_id).collect();
        if !handles.absent(&selected, binding.as_deref(), &panes)? {
            return Ok(false);
        }
        // Legacy gates had neither a PID nor a pane. Also catch detached descendants left
        // behind by a closed pane: argv task identity or a task-scoped working directory.
        processes_absent(Path::new("/proc"), task)
    }))
}

#[derive(Default)]
struct Handles {
    sockets: BTreeSet<PathBuf>,
    panes: BTreeSet<String>,
    processes: Vec<(u32, String)>,
}
impl Handles {
    fn absent(
        &self,
        selected: &Path,
        binding: Option<&Path>,
        panes: &BTreeSet<String>,
    ) -> Result<bool> {
        ensure!(
            self.sockets.iter().all(|s| s == selected),
            "foreign runtime socket"
        );
        ensure!(
            binding == Some(selected) || !self.sockets.is_empty(),
            "runtime socket unknown"
        );
        if !self.panes.is_disjoint(panes) {
            return Ok(false);
        }
        for (pid, start) in &self.processes {
            if let Some(process) = ProcessIdentity::of(*pid)?
                && (start.is_empty()
                    || process.start_time.is_empty()
                    || process.start_time == *start)
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn collect(&mut self, doc: &Value, fallback: Option<&Path>) {
        match doc {
            Value::Object(fields) => {
                let socket = fields
                    .get("socket")
                    .or_else(|| fields.get("herdr_socket"))
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(Path::new)
                    .or(fallback);
                for key in ["pane", "pane_id"] {
                    if let Some(pane) = fields
                        .get(key)
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                    {
                        self.panes.insert(pane.to_owned());
                        if let Some(socket) = socket {
                            self.sockets.insert(socket.to_path_buf());
                        }
                    }
                }
                if let Some(pid) = fields.get("pid").and_then(Value::as_u64)
                    && let Ok(pid) = u32::try_from(pid)
                {
                    self.processes.push((
                        pid,
                        fields
                            .get("start_time")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                    ));
                }
                for value in fields.values() {
                    self.collect(value, socket);
                }
            }
            Value::Array(items) => {
                for value in items {
                    self.collect(value, fallback);
                }
            }
            _ => {}
        }
    }
}

fn task_command(cmdline: &[u8], task: &str) -> bool {
    let args: Vec<_> = cmdline
        .split(|b| *b == 0)
        .filter_map(|s| std::str::from_utf8(s).ok())
        .collect();
    let corral = args
        .first()
        .and_then(|s| Path::new(s).file_name())
        .is_some_and(|s| s.to_string_lossy().starts_with("corral"));
    // A PM's grep/jq/show monitor is not a task executor. Only executable corral actions or
    // named gate scripts count here; shells' arbitrary embedded query text does not.
    let action = args
        .iter()
        .any(|s| matches!(*s, "gate" | "verify" | "dispatch" | "retry" | "resume"));
    (corral && action && args.contains(&task))
        || args.iter().any(|s| {
            !s.contains(char::is_whitespace)
                && (s.contains("/state/gates/") || s.contains("/state/plan-gates/"))
                && task_token(s, task)
        })
}

fn task_token(text: &str, task: &str) -> bool {
    text.match_indices(task).any(|(at, _)| {
        let end = at + task.len();
        (at == 0 || !text.as_bytes()[at - 1].is_ascii_alphanumeric())
            && (end == text.len() || !text.as_bytes()[end].is_ascii_alphanumeric())
    })
}

#[cfg(target_os = "linux")]
fn processes_absent(proc: &Path, task: &Task) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let uid = std::fs::metadata(proc.join("self"))?.uid();
    let stat = std::fs::read_to_string(proc.join("stat"))?;
    let boot: f64 = stat
        .lines()
        .find_map(|l| l.strip_prefix("btime "))
        .context("process boot time")?
        .parse()?;
    let ticks = rustix::param::clock_ticks_per_second();
    ensure!(ticks > 0, "process clock frequency unknown");
    for entry in std::fs::read_dir(proc).context("reading process inventory")? {
        let path = entry?.path();
        if path
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u32>().ok())
            .is_none()
        {
            continue;
        }
        let inspect = || -> Result<bool> {
            if std::fs::metadata(&path)?.uid() != uid {
                return Ok(true);
            }
            let cmdline = std::fs::read(path.join("cmdline"))?;
            if cmdline.is_empty() {
                return Ok(true);
            } // exited/zombie
            if task_command(&cmdline, &task.id) {
                return Ok(false);
            }
            // These shared user-session supervisors have ptrace-protected cwd links. They
            // pre-exist task execution and are not task commands; their children are still
            // inspected individually. No other permission failure is evidence of absence.
            let executable = cmdline.split(|b| *b == 0).next().unwrap_or_default();
            if matches!(
                executable,
                b"/usr/lib/systemd/systemd" | b"/lib/systemd/systemd" | b"(sd-pam)"
            ) {
                return Ok(true);
            }
            let stat = std::fs::read_to_string(path.join("stat"))?;
            let start: f64 = stat
                .rsplit_once(')')
                .context("process stat")?
                .1
                .split_whitespace()
                .nth(19)
                .context("process start")?
                .parse()?;
            // A pre-existing supervisor cannot be a task's detached command. Checking this
            // before cwd also avoids requiring ptrace access to the user's systemd session.
            if boot + start / ticks as f64 + 1.0 < task.created.as_second() as f64 {
                return Ok(true);
            }
            let cwd = std::fs::read_link(path.join("cwd"))?;
            Ok(!task_token(&cwd.to_string_lossy(), &task.id))
        };
        match inspect() {
            Ok(true) => {}
            Ok(false) => return Ok(false),
            Err(_) if !path.exists() => {} // process exited during the read
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", path.display()));
            }
        }
    }
    Ok(true)
}
#[cfg(not(target_os = "linux"))]
fn processes_absent(_proc: &Path, _task: &Task) -> Result<bool> {
    Ok(false)
}

#[cfg(test)]
#[path = "migration_liveness_tests.rs"]
mod tests;
