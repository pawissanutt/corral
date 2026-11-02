//! Read-only legacy migration liveness adapter. Never opens a Store or reconciles owner data.
use anyhow::{Context, Result, ensure};
use corral_core::{Task, blob::ProcessIdentity, migrate::RuntimeProbe};
use corral_herdr::Herdr;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn migration_runtime_probe(root: &Path) -> Result<RuntimeProbe> {
    let config = corral_core::Config::load(&root.join(".corral/corral.toml"))?;
    let owner_dirs = launch_owner_dirs(root, &config)?;
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
        // Descendants can outlive a pane and recorded parent, and can change cwd/argv.
        // Both legacy and v2 issuers persist the inherited token beside launch receipts.
        let owners = LaunchOwners::read_all(&owner_dirs)?;
        processes_absent(Path::new("/proc"), task, &owners)
    }))
}

/// The same scratch layout as Store::scratch_path, resolved without opening a Store or
/// creating directories. Migration also needs the old issuer's retained markers.
fn launch_owner_dirs(root: &Path, config: &corral_core::Config) -> Result<[PathBuf; 2]> {
    let layout = corral_core::StorageLayout::resolve(
        root,
        &config.storage,
        &config.projects,
        &corral_core::StorageEnv::from_process(),
    )
    .map_err(|errors| {
        anyhow::anyhow!(
            "{}",
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        )
    })?;
    Ok([
        corral_core::migrate::Migrate::new(layout.workspace())
            .corral_dir()
            .join("state/launch-exits"),
        layout.scratch_root().join("tmp").join("launch-exits"),
    ])
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

/// Legacy launch markers cover coding/review/planning and gate descendants alike. A retained
/// marker alone is history; only a live process carrying it proves associated work is present.
#[derive(Default)]
struct LaunchOwners(BTreeMap<Vec<u8>, String>);

impl LaunchOwners {
    fn read_all(dirs: &[PathBuf]) -> Result<Self> {
        let mut owners = Self::default();
        for dir in dirs {
            for (marker, task) in Self::read(dir)?.0 {
                if let Some(previous) = owners.0.insert(marker, task.clone()) {
                    ensure!(previous == task, "launch owner belongs to multiple tasks");
                }
            }
        }
        Ok(owners)
    }

    fn read(dir: &Path) -> Result<Self> {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error).with_context(|| format!("reading {}", dir.display())),
        };
        let mut owners = Self::default();
        for entry in entries {
            let entry = entry.context("reading legacy process owner entry")?;
            let name = entry.file_name();
            let Some(stem) = name.to_str().and_then(|s| s.strip_suffix(".owner")) else {
                continue;
            };
            // PM shells use the same issuer with PM-<pane>. They are known workspace
            // owners, not descendants of any task, so don't confuse them with unknown tokens.
            let task = if stem
                .strip_prefix("PM-")
                .is_some_and(|pane| !pane.is_empty())
            {
                "PM".to_owned()
            } else {
                let (number, attempt) = stem
                    .strip_prefix("T-")
                    .and_then(|s| s.split_once('-'))
                    .with_context(|| {
                        format!(
                            "invalid legacy process owner filename: {}",
                            entry.path().display()
                        )
                    })?;
                ensure!(
                    !number.is_empty()
                        && number.bytes().all(|b| b.is_ascii_digit())
                        && !attempt.is_empty(),
                    "invalid legacy process owner filename"
                );
                format!("T-{number}")
            };
            let meta = std::fs::symlink_metadata(entry.path())
                .context("inspecting legacy process owner")?;
            ensure!(
                meta.is_file() && meta.len() == 64,
                "invalid legacy process owner file: {}",
                entry.path().display()
            );
            let mut marker = Vec::new();
            std::fs::File::open(entry.path())
                .context("opening legacy process owner")?
                .take(65)
                .read_to_end(&mut marker)
                .context("reading legacy process owner")?;
            ensure!(
                marker.len() == 64 && marker.iter().all(u8::is_ascii_hexdigit),
                "invalid legacy process owner marker"
            );
            if let Some(previous) = owners.0.insert(marker, task.clone()) {
                ensure!(
                    previous == task,
                    "legacy process owner belongs to multiple tasks"
                );
            }
        }
        Ok(owners)
    }

    fn owns_environment(&self, env: &[u8], task: &str) -> Result<bool> {
        let mut owned = false;
        for value in env.split(|byte| *byte == 0) {
            if let Some(id) = value.strip_prefix(b"CORRAL_SANDBOX_TASK=") {
                owned |= id == task.as_bytes();
            }
            if let Some(marker) = value.strip_prefix(b"CORRAL_PROCESS_OWNER=") {
                let owner = self
                    .0
                    .get(marker)
                    .context("live process has unconfirmed launch ownership")?;
                owned |= owner == task;
            }
        }
        Ok(owned)
    }
}

#[cfg(target_os = "linux")]
fn processes_absent(proc: &Path, task: &Task, owners: &LaunchOwners) -> Result<bool> {
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
            // Inspect inherited ownership before the birth-time/cwd heuristics: an exec can
            // retain an older PID start time, and a descendant may have left the task tree.
            let mut environment = Vec::new();
            std::fs::File::open(path.join("environ"))?
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut environment)?;
            ensure!(
                environment.len() <= 4 * 1024 * 1024,
                "process environment exceeds inspection cap"
            );
            if owners.owns_environment(&environment, &task.id)? {
                return Ok(false);
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
fn processes_absent(_proc: &Path, _task: &Task, _owners: &LaunchOwners) -> Result<bool> {
    Ok(false)
}

#[cfg(test)]
#[path = "migration_liveness_tests.rs"]
mod tests;
