//! When and how the old board closed each task, read from `corral export events --json`.

use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

use anyhow::{Context, Result};
use corral_engine::{Actor, Status};
use jiff::Timestamp;
use serde::Deserialize;

/// The old log's last record of a task reaching Done or Dropped.
pub(super) struct Closure {
    pub(super) at: Timestamp,
    pub(super) actor: Actor,
    /// The status it left, in the new model's words.
    pub(super) from: Option<String>,
    /// Present when a commit landed, absent when the task was moved to Done by hand.
    pub(super) sha: Option<String>,
    pub(super) base: Option<String>,
}

#[derive(Default)]
pub(super) struct History {
    done: BTreeMap<String, Closure>,
    dropped: BTreeMap<String, Closure>,
}

/// Fields are optional so a line of another shape (`--with-conditions`) is skipped, not fatal.
#[derive(Deserialize)]
struct Line {
    ts: Option<Timestamp>,
    task: Option<String>,
    kind: Option<String>,
    actor: Option<String>,
}

#[derive(Deserialize)]
struct Detailed {
    detail: Detail,
}

#[derive(Deserialize)]
struct Detail {
    from: Option<String>,
    to: Option<String>,
    sha: Option<String>,
    base: Option<String>,
}

impl History {
    /// Reads the log one line at a time: it holds every event the old board ever stored.
    pub(super) fn read(path: &Path) -> Result<Self> {
        let file =
            File::open(path).with_context(|| format!("open event log {}", path.display()))?;
        let mut history = Self::default();
        for (index, line) in BufReader::new(file).lines().enumerate() {
            let place = || format!("{} line {}", path.display(), index + 1);
            let line = line.with_context(|| format!("read {}", place()))?;
            if line.trim().is_empty() {
                continue;
            }
            let head: Line =
                serde_json::from_str(&line).with_context(|| format!("parse {}", place()))?;
            let (Some(at), Some(task), Some(kind)) = (head.ts, head.task, head.kind) else {
                continue;
            };
            let landed = kind == "approved";
            if !landed && kind != "moved" {
                continue;
            }
            // Only these two kinds have a detail of this shape; others reuse the field names.
            let Detailed { detail } = serde_json::from_str(&line)
                .with_context(|| format!("parse the detail of {}", place()))?;
            let closed = match detail.to.as_deref() {
                _ if landed => &mut history.done,
                Some("done") => &mut history.done,
                Some("dropped") => &mut history.dropped,
                _ => continue,
            };
            // The log is in ascending order, so the last record of each task wins.
            closed.insert(
                task,
                Closure {
                    at,
                    actor: actor(head.actor.as_deref()),
                    from: detail.from.map(|status| match status.as_str() {
                        "needs_me" => "blocked".to_owned(),
                        _ => status,
                    }),
                    sha: detail.sha.filter(|_| landed),
                    base: detail.base.filter(|_| landed),
                },
            );
        }
        Ok(history)
    }

    pub(super) fn closure(&self, id: &str, status: Status) -> Option<&Closure> {
        match status {
            Status::Done => self.done.get(id),
            Status::Dropped => self.dropped.get(id),
            _ => None,
        }
    }
}

/// The new model's actor for one the old board named; an unnamed one was the owner's command.
pub(super) fn actor(name: Option<&str>) -> Actor {
    match name {
        Some("pm" | "assistant") => Actor::Assistant,
        Some("agent") => Actor::Agent,
        Some("reviewer") => Actor::Reviewer,
        Some("autopilot" | "system") => Actor::Auto,
        _ => Actor::Owner,
    }
}
