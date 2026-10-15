//! Which harness session log belongs to one agent or reviewer run.
//!
//! Several sessions share a worktree directory (the implementer, each reviewer, old runs), so the
//! newest file is not the run's session. A session belongs to a run when it was created inside the
//! run's window (its header timestamp), and for OMP when its header cwd is the run's worktree.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use corral_core::Run;
use jiff::Timestamp;
use serde_json::Value;

use crate::{Ctx, omo};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Omo,
    Omp,
}

fn kind(run: &Run) -> Option<Kind> {
    match run.harness.as_deref().unwrap_or(&run.agent) {
        "omo" => Some(Kind::Omo),
        "omp" => Some(Kind::Omp),
        _ => None,
    }
}

struct Header {
    started: Timestamp,
    cwd: Option<PathBuf>,
}

fn header(path: &Path) -> Option<Header> {
    let lines = BufReader::new(File::open(path).ok()?).lines();
    for line in lines.take(4).map_while(Result::ok) {
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if entry["type"] == "session" {
            return Some(Header {
                started: entry["timestamp"].as_str()?.parse().ok()?,
                cwd: entry["cwd"].as_str().map(PathBuf::from),
            });
        }
    }
    None
}

fn session_dirs(ctx: &Ctx, run: &Run, kind: Kind) -> Vec<PathBuf> {
    match kind {
        Kind::Omo => {
            let agent = omo::agent_dir(ctx.store.config.sync.omo_agent_dir.as_deref());
            vec![agent.join("sessions").join(omo::encode_cwd(&run.worktree))]
        }
        Kind::Omp => {
            let agent = ctx.store.config.sync.omp_agent_dir.clone().or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".omp/agent"))
            });
            let Some(sessions) = agent.map(|dir| dir.join("sessions")) else {
                return Vec::new();
            };
            // omp names buckets relative to $HOME or the temp dir; the header cwd stays authoritative.
            let relative = |base: PathBuf, prefix: &str| {
                run.worktree.strip_prefix(base).ok().map(|rel| {
                    sessions.join(format!(
                        "{prefix}{}",
                        rel.to_string_lossy().replace(['/', '\\', ':'], "-")
                    ))
                })
            };
            let home = std::env::var_os("HOME").map(PathBuf::from);
            [
                home.and_then(|home| relative(home, "-")),
                relative(std::env::temp_dir(), "-tmp-"),
                Some(sessions.join(omo::encode_cwd(&run.worktree))),
            ]
            .into_iter()
            .flatten()
            .collect()
        }
    }
}

/// Which sessions a run may own: ones created inside its window, or (a reused PM pane) ones already
/// running that were written to since the review was requested.
#[derive(Clone, Copy)]
pub(crate) enum Scope {
    Created { until: Option<Timestamp> },
    Active,
}

fn launch_path(ctx: &Ctx, task: &str, run_id: &str) -> PathBuf {
    ctx.store
        .state_dir()
        .join("provider-stall")
        .join(format!("{task}-{run_id}.launch"))
}

/// Dispatch calls this with the run's pre-launch `started`: the harness may create its session while
/// the pane is still being launched, before the post-launch `started` that restarts the startup grace.
pub(crate) fn record_launch(ctx: &Ctx, task: &str, run_id: &str, at: Timestamp) {
    let path = launch_path(ctx, task, run_id);
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, at.to_string()));
    if let Err(error) = written {
        crate::messages::report(
            Some(task),
            "provider-stall",
            format!("recording launch time for {task} failed: {error}"),
        );
    }
}

/// Earliest instant a session or error can belong to this run.
pub(crate) fn boundary(ctx: &Ctx, task: &str, run: &Run) -> Timestamp {
    std::fs::read_to_string(launch_path(ctx, task, &run.id))
        .ok()
        .and_then(|text| text.trim().parse::<Timestamp>().ok())
        .map_or(run.started, |launch| launch.min(run.started))
}

/// The newest session this run owns in `scope`, if its harness has a session-log adapter.
pub(crate) fn discover(ctx: &Ctx, run: &Run, boundary: Timestamp, scope: Scope) -> Option<PathBuf> {
    let kind = kind(run)?;
    session_dirs(ctx, run, kind)
        .into_iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|entry| {
            let path = entry.path();
            let created = header(&path)?;
            let modified = Timestamp::try_from(entry.metadata().ok()?.modified().ok()?).ok()?;
            let in_scope = match scope {
                Scope::Created { until } => {
                    created.started >= boundary && until.is_none_or(|end| created.started < end)
                }
                Scope::Active => modified >= boundary,
            };
            let ours = kind == Kind::Omo || created.cwd.as_deref() == Some(run.worktree.as_path());
            (in_scope && ours).then_some((modified, path))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}
