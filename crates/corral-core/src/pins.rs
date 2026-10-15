//! Pins: `.corral/pins.toml`, an ordered list of tasks and artifacts kept one step away.
//! Pins live outside task files so pinning never rewrites a task.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::model::{Artifact, Status, Task};
use crate::store::{Store, atomic_write};

/// How an artifact pin picks its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Follow {
    /// The newest artifact of the same task with the same title or source path.
    Latest,
}

/// One entry of pins.toml.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    /// "T-042" or "T-042:R1:<artifact snapshot file name>".
    #[serde(rename = "ref")]
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow: Option<Follow>,
}

#[derive(Default, Serialize, Deserialize)]
struct PinsFile {
    #[serde(default, rename = "pin")]
    pins: Vec<Pin>,
}

/// A parsed pin ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinRef {
    Task(String),
    Artifact {
        task: String,
        run: String,
        file: String,
    },
}

impl PinRef {
    /// Parses "T-042" / "42" or "T-042:R1:<file>"; the task id is normalized, the run upper-cased.
    pub fn parse(s: &str) -> Result<PinRef> {
        let mut parts = s.trim().splitn(3, ':');
        let task = Store::normalize_id(parts.next().unwrap_or(""))?;
        match (parts.next(), parts.next()) {
            (None, _) => Ok(PinRef::Task(task)),
            (Some(run), Some(file)) if !run.trim().is_empty() && !file.trim().is_empty() => {
                Ok(PinRef::Artifact {
                    task,
                    run: run.trim().to_ascii_uppercase(),
                    file: file.trim().to_string(),
                })
            }
            _ => bail!("invalid pin ref {s:?} (expected T-042 or T-042:R1:<artifact-file>)"),
        }
    }

    pub fn task(&self) -> &str {
        match self {
            PinRef::Task(t) | PinRef::Artifact { task: t, .. } => t,
        }
    }

    /// The ref of an artifact: "T-042:R1:<snapshot file name>".
    pub fn of_artifact(task: &str, a: &Artifact) -> PinRef {
        PinRef::Artifact {
            task: task.to_string(),
            run: a.run.clone(),
            file: snapshot_name(a).to_string(),
        }
    }

    /// Whether this (user-typed) ref names the same target as `stored`: the file may omit the sha8 prefix.
    fn matches(&self, stored: &PinRef) -> bool {
        match (self, stored) {
            (PinRef::Task(a), PinRef::Task(b)) => a == b,
            (
                PinRef::Artifact { task, run, file },
                PinRef::Artifact {
                    task: t2,
                    run: r2,
                    file: f2,
                },
            ) => task == t2 && run == r2 && (file == f2 || strip_sha(f2) == file),
            _ => false,
        }
    }
}

impl fmt::Display for PinRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PinRef::Task(t) => write!(f, "{t}"),
            PinRef::Artifact { task, run, file } => write!(f, "{task}:{run}:{file}"),
        }
    }
}

/// What a pin points at right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinTarget {
    Task {
        id: String,
        title: String,
        status: Status,
        /// The task file.
        path: PathBuf,
    },
    Artifact {
        task: String,
        artifact: Artifact,
        /// Absolute snapshot path.
        path: PathBuf,
        /// True when follow-latest moved the pin past the artifact it was made on.
        followed: bool,
    },
    /// The task, the artifact record or the snapshot file is gone.
    Broken(String),
}

/// Snapshot file name of an artifact ("9f3c1a2b-report.html").
pub fn snapshot_name(a: &Artifact) -> &str {
    a.path.rsplit('/').next().unwrap_or(&a.path)
}

/// "9f3c1a2b-report.html" -> "report.html" (names without a sha8 prefix are returned as-is).
fn strip_sha(name: &str) -> &str {
    match name.split_once('-') {
        Some((sha, rest)) if sha.len() == 8 && sha.chars().all(|c| c.is_ascii_hexdigit()) => rest,
        _ => name,
    }
}

/// The artifact of `task` named by `run` + `file` (exact snapshot name, else original file name; newest wins).
fn find_artifact<'a>(task: &'a Task, run: &str, file: &str) -> Option<&'a Artifact> {
    let in_run = || task.artifacts.iter().filter(move |a| a.run == run);
    in_run().find(|a| snapshot_name(a) == file).or_else(|| {
        in_run()
            .filter(|a| strip_sha(snapshot_name(a)) == file)
            .max_by_key(|a| a.added)
    })
}

/// Resolves `pin` against `tasks`; `dir` is the `.corral/` dir holding the snapshots.
pub fn resolve_pin(pin: &Pin, tasks: &[Task], dir: &Path) -> PinTarget {
    let r = match PinRef::parse(&pin.target) {
        Ok(r) => r,
        Err(e) => return PinTarget::Broken(format!("{e:#}")),
    };
    let Some(task) = tasks.iter().find(|t| t.id == r.task()) else {
        return PinTarget::Broken(format!("task {} not found", r.task()));
    };
    let PinRef::Artifact { run, file, .. } = &r else {
        return PinTarget::Task {
            id: task.id.clone(),
            title: task.title.clone(),
            status: task.status,
            path: task.path.clone(),
        };
    };
    let Some(anchor) = find_artifact(task, run, file) else {
        return PinTarget::Broken(format!("artifact {run}:{file} not found on {}", task.id));
    };
    let chosen = match pin.follow {
        Some(Follow::Latest) => task
            .artifacts
            .iter()
            .filter(|a| a.title == anchor.title || a.source == anchor.source)
            .max_by_key(|a| a.added)
            .unwrap_or(anchor),
        None => anchor,
    };
    let path = dir.join(&chosen.path);
    if !path.is_file() {
        return PinTarget::Broken(format!("snapshot {} is missing", chosen.path));
    }
    PinTarget::Artifact {
        task: task.id.clone(),
        artifact: chosen.clone(),
        path,
        followed: chosen.path != anchor.path,
    }
}

/// 0-based position of the pin named by `query`: a 1-based pin number, or a ref (artifact file may omit
/// the sha8 prefix). Errors when nothing or more than one pin matches.
pub fn pin_position(pins: &[Pin], query: &str) -> Result<usize> {
    let q = query.trim();
    if !q.is_empty() && q.chars().all(|c| c.is_ascii_digit()) {
        let n: usize = q
            .parse()
            .with_context(|| format!("invalid pin number {q:?}"))?;
        if n == 0 || n > pins.len() {
            bail!("no pin #{n} ({} pinned)", pins.len());
        }
        return Ok(n - 1);
    }
    let want = PinRef::parse(q)?;
    let hits: Vec<usize> = pins
        .iter()
        .enumerate()
        .filter(|(_, p)| PinRef::parse(&p.target).is_ok_and(|s| want.matches(&s)))
        .map(|(i, _)| i)
        .collect();
    match hits.as_slice() {
        [i] => Ok(*i),
        [] => bail!("{want} is not pinned"),
        _ => bail!("{want} matches several pins; use the full ref or the pin number"),
    }
}

impl Store {
    fn pins_path(&self) -> PathBuf {
        self.dir().join("pins.toml")
    }

    /// All pins in the user's order (missing pins.toml -> empty).
    pub fn pins(&self) -> Result<Vec<Pin>> {
        let path = self.pins_path();
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let file: PinsFile =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(file.pins)
    }

    /// Replaces pins.toml with `pins` (in order).
    pub fn save_pins(&self, pins: &[Pin]) -> Result<()> {
        let file = PinsFile {
            pins: pins.to_vec(),
        };
        let text = toml::to_string(&file).context("serializing pins")?;
        atomic_write(&self.pins_path(), &text)
    }

    /// Pins a task ("T-042") or artifact ("T-042:R1:<file>", file may omit the sha8 prefix). The target must
    /// exist; artifact refs are stored with the snapshot file name. `follow_latest` needs an artifact.
    /// Re-pinning an already pinned target updates its label/follow in place. Returns (1-based number, pin).
    pub fn pin(
        &self,
        target: &str,
        label: Option<&str>,
        follow_latest: bool,
    ) -> Result<(usize, Pin)> {
        let r = PinRef::parse(target)?;
        let task = self.get(r.task())?;
        let canonical = match &r {
            PinRef::Task(_) if follow_latest => {
                bail!("--follow-latest needs an artifact ref (T-042:R1:<file>)")
            }
            PinRef::Task(_) => r.clone(),
            PinRef::Artifact { run, file, .. } => {
                let a = find_artifact(&task, run, file)
                    .with_context(|| format!("artifact {run}:{file} not found on {}", task.id))?;
                PinRef::of_artifact(&task.id, a)
            }
        };
        let _lock = self.lock("pins")?;
        let mut pins = self.pins()?;
        let follow = follow_latest.then_some(Follow::Latest);
        let key = canonical.to_string();
        let idx = match pins.iter().position(|p| p.target == key) {
            Some(i) => {
                if let Some(l) = label {
                    pins[i].label = Some(l.to_string());
                }
                pins[i].follow = follow;
                i
            }
            None => {
                pins.push(Pin {
                    target: key,
                    label: label.map(str::to_string),
                    follow,
                });
                pins.len() - 1
            }
        };
        self.save_pins(&pins)?;
        let pin = pins[idx].clone();
        self.log_event(
            Some(&task.id),
            "pin",
            serde_json::json!({ "ref": pin.target, "label": pin.label, "follow": pin.follow }),
        )?;
        Ok((idx + 1, pin))
    }

    /// Removes the pin named by `query` (pin number or ref, see `pin_position`) and returns it.
    pub fn unpin(&self, query: &str) -> Result<Pin> {
        let _lock = self.lock("pins")?;
        let mut pins = self.pins()?;
        let pin = pins.remove(pin_position(&pins, query)?);
        self.save_pins(&pins)?;
        let task = PinRef::parse(&pin.target)
            .ok()
            .map(|r| r.task().to_string());
        self.log_event(
            task.as_deref(),
            "unpin",
            serde_json::json!({ "ref": pin.target }),
        )?;
        Ok(pin)
    }

    /// Moves the pin at 0-based `from` to position `to` (clamped to the list); returns its new position.
    pub fn move_pin(&self, from: usize, to: usize) -> Result<usize> {
        let _lock = self.lock("pins")?;
        let mut pins = self.pins()?;
        if from >= pins.len() {
            bail!("no pin #{} ({} pinned)", from + 1, pins.len());
        }
        let to = to.min(pins.len() - 1);
        let pin = pins.remove(from);
        let target = pin.target.clone();
        pins.insert(to, pin);
        self.save_pins(&pins)?;
        let task = PinRef::parse(&target).ok().map(|r| r.task().to_string());
        self.log_event(
            task.as_deref(),
            "pin_move",
            serde_json::json!({ "ref": target, "from": from + 1, "to": to + 1 }),
        )?;
        Ok(to)
    }

    /// Sets the label of the pin at 0-based `index`; `None` or a blank label clears it (the target's title shows).
    pub fn label_pin(&self, index: usize, label: Option<&str>) -> Result<Pin> {
        let _lock = self.lock("pins")?;
        let mut pins = self.pins()?;
        let n = pins.len();
        let pin = pins
            .get_mut(index)
            .with_context(|| format!("no pin #{} ({n} pinned)", index + 1))?;
        pin.label = label
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string);
        let pin = pin.clone();
        self.save_pins(&pins)?;
        let task = PinRef::parse(&pin.target)
            .ok()
            .map(|r| r.task().to_string());
        self.log_event(
            task.as_deref(),
            "pin",
            serde_json::json!({ "ref": pin.target, "label": pin.label, "follow": pin.follow }),
        )?;
        Ok(pin)
    }

    /// Every pin with its current target, in order.
    pub fn resolved_pins(&self) -> Result<Vec<(Pin, PinTarget)>> {
        let pins = self.pins()?;
        let tasks = self.list()?;
        Ok(pins
            .into_iter()
            .map(|p| {
                let t = resolve_pin(&p, &tasks, self.dir());
                (p, t)
            })
            .collect())
    }
}
