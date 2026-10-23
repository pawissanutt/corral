//! Reviewer monitoring uses the existing accounting children, plus artifact references.
use std::path::PathBuf;

use anyhow::{Context, Result};
use corral_core::{Artifact, ArtifactKind, ReviewRecord, Run, Status, Task, Verdict};
use serde::{Deserialize, Serialize};

use super::ProgressState;
use crate::Ctx;

#[derive(Debug, Clone)]
pub struct ReviewerAttempt {
    pub run: String,
    pub round: u32,
    pub state: ProgressState,
    pub live_pane: Option<String>,
    /// Stable attempt key is accounting.id. Includes launch profile/model and usage/cost.
    pub accounting: Run,
    pub record: Option<ReviewRecord>,
    /// Transcript first, then screen. Notes are in record.notes, not this list.
    pub artifacts: Vec<Artifact>,
}

#[derive(Default, Serialize, Deserialize)]
struct Capture {
    round: u32,
    transcript: Option<String>,
    screen: Option<String>,
    #[serde(default)]
    interrupted_at: Option<jiff::Timestamp>,
    #[serde(default)]
    captured_at: Option<jiff::Timestamp>,
}

fn path(ctx: &Ctx, task: &str, child: &str) -> PathBuf {
    ctx.store
        .state_dir()
        .join("review-watch")
        .join(task)
        .join(format!("{child}.json"))
}

fn read(ctx: &Ctx, task: &str, child: &str) -> Result<Capture> {
    let path = path(ctx, task, child);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("parsing reviewer monitor"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Capture::default()),
        Err(e) => Err(e).context("reading reviewer monitor"),
    }
}

pub(crate) fn start_review(ctx: &Ctx, task: &Task, child: &Run) -> Result<()> {
    let capture = Capture {
        round: task.rounds,
        ..Default::default()
    };
    crate::artifact_storage::replace(
        &path(ctx, &task.id, &child.id),
        &serde_json::to_vec(&capture)?,
    )
}

/// Capture before closing/replacing a pane. Failures are reported, never verification inputs.
pub(crate) fn capture_review(ctx: &Ctx, task: &Task, run: &Run) {
    let Some(child) = run.review_runs.last() else {
        return;
    };
    if let Err(error) = capture_child(ctx, task, run, child) {
        crate::messages::report(Some(&task.id), "review capture", format!("{error:#}"));
    }
}

/// The capture of one named review child, current or not: true once it is durably recorded,
/// now or earlier; false while another capture holds its lock or this session does not own
/// the store's panes.
pub(crate) fn capture_review_exact(ctx: &Ctx, task: &Task, run: &str, child: &str) -> Result<bool> {
    let parent = task
        .runs
        .iter()
        .find(|r| r.id == run)
        .context("review parent run missing")?;
    let named = parent.review_runs.iter().find(|c| c.id == child);
    capture_child(ctx, task, parent, named.context("review child missing")?)
}

fn capture_child(ctx: &Ctx, task: &Task, run: &Run, child: &Run) -> Result<bool> {
    {
        let Some(_lock) = ctx
            .store
            .try_lock(&format!("review-capture-{}-{}", task.id, child.id))?
        else {
            return Ok(false);
        };
        let mut capture = read(ctx, &task.id, &child.id)?;
        if capture.captured_at.is_some() {
            return Ok(true);
        }
        if !crate::session_scope::owns_inventory(ctx)? {
            return Ok(false);
        }
        let mut window = child.clone();
        window.ended.get_or_insert_with(jiff::Timestamp::now);
        let notes = crate::review_notes_path(ctx, &task.id, &run.id);
        if let Ok(notes) = std::fs::read_to_string(notes)
            && !notes.trim().is_empty()
        {
            ctx.store.update(&task.id, |task| {
                if let Some(saved) = task
                    .runs
                    .iter_mut()
                    .find(|r| r.id == run.id)
                    .and_then(|r| r.review_runs.iter_mut().find(|r| r.id == child.id))
                    .and_then(|r| r.review.as_mut())
                    && saved.notes.is_none()
                {
                    saved.notes = Some(notes.clone());
                }
                Ok(())
            })?;
        }
        let transcript = if matches!(child.harness.as_deref(), Some("omo" | "omp" | "claude")) {
            crate::usage::transcript(&window, &ctx.store)
        } else {
            Ok(Vec::new())
        };
        match transcript {
            Ok(bytes) if !bytes.is_empty() => {
                capture.transcript = Some(snapshot(ctx, task, run, child, "transcript", &bytes)?);
            }
            Ok(_) => {}
            Err(error) => {
                crate::messages::report(Some(&task.id), "review transcript", format!("{error:#}"))
            }
        }
        let record = child.review.as_ref().or(run.review.as_ref());
        if let Some(pane) = record.and_then(|r| r.pane.as_deref()) {
            match ctx.herdr.pane_read_if_present(pane, 10_000) {
                Ok(Some(screen)) => {
                    capture.screen = Some(snapshot(
                        ctx,
                        task,
                        run,
                        child,
                        "screen",
                        screen.as_bytes(),
                    )?);
                }
                Ok(None) => {}
                Err(error) => {
                    crate::messages::report(Some(&task.id), "review screen", format!("{error:#}"))
                }
            }
        }
        capture.captured_at = Some(jiff::Timestamp::now());
        crate::artifact_storage::replace(
            &path(ctx, &task.id, &child.id),
            &serde_json::to_vec(&capture)?,
        )?;
        Ok(true)
    }
}

fn snapshot(
    ctx: &Ctx,
    task: &Task,
    parent: &Run,
    child: &Run,
    kind: &str,
    bytes: &[u8],
) -> Result<String> {
    let file = path(ctx, &task.id, &child.id).with_extension(format!("{kind}.log"));
    crate::artifact_storage::replace(&file, bytes)?;
    let artifact = crate::artifacts::add_artifact_for_run(
        ctx,
        &task.id,
        &parent.id,
        &file,
        Some(ArtifactKind::Log),
        Some(&format!("Reviewer {kind} ({})", child.id)),
    )?;
    Ok(artifact.path)
}

pub(crate) fn load(ctx: &Ctx, task: &Task, owned: bool) -> Result<Vec<ReviewerAttempt>> {
    let mut attempts = Vec::new();
    for parent in &task.runs {
        for (index, child) in parent.review_runs.iter().enumerate() {
            let mut capture = read(ctx, &task.id, &child.id)?;
            let current = owned
                && !matches!(task.status, Status::Done | Status::Dropped)
                && task.current_run().is_some_and(|r| r.id == parent.id)
                && index + 1 == parent.review_runs.len()
                && child.ended.is_none()
                && child.outcome.is_none();
            let record = child.review.clone().or_else(|| {
                (index + 1 == parent.review_runs.len())
                    .then(|| parent.review.clone())
                    .flatten()
            });
            let mut available = true;
            if current
                && capture.interrupted_at.is_none()
                && capture.captured_at.is_none()
                && record.as_ref().is_some_and(|r| r.verdict.is_none())
                && record.as_ref().is_some_and(|r| !r.process_lost)
                && let Some(pane) = record.as_ref().and_then(|r| r.pane.as_deref())
            {
                match ctx.herdr.pane_get(pane) {
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        capture
                            .interrupted_at
                            .get_or_insert_with(jiff::Timestamp::now);
                        crate::artifact_storage::replace(
                            &path(ctx, &task.id, &child.id),
                            &serde_json::to_vec(&capture)?,
                        )?;
                    }
                    Err(error) => {
                        available = false;
                        crate::messages::report(
                            Some(&task.id),
                            "review live pane unavailable",
                            format!("{error:#}"),
                        );
                    }
                }
            }
            let state = match record.as_ref().and_then(|r| r.verdict) {
                Some(Verdict::Approve) => ProgressState::Passed,
                Some(Verdict::Changes | Verdict::Escalate) => ProgressState::Failed,
                None if capture.interrupted_at.is_some()
                    || capture.captured_at.is_some()
                    || matches!(task.status, Status::Done | Status::Dropped)
                    || child.ended.is_some()
                    || child.outcome.is_some()
                    || record.as_ref().is_some_and(|r| r.process_lost) =>
                {
                    ProgressState::Interrupted
                }
                None if record.as_ref().and_then(|r| r.pane.as_ref()).is_none() => {
                    ProgressState::Pending
                }
                None => ProgressState::Running,
            };
            let live_pane = current
                .then(|| record.as_ref().and_then(|r| r.pane.clone()))
                .flatten()
                .filter(|_| available && state == ProgressState::Running);
            let artifacts = [
                (capture.transcript, "transcript"),
                (capture.screen, "screen"),
            ]
            .into_iter()
            .filter_map(|(path, kind)| {
                let path = path?;
                let mut artifact = task.artifacts.iter().find(|a| a.path == path)?.clone();
                // Storage deduplicates equal bytes within a run. Give each attempt's
                // reference its own display title without duplicating the snapshot.
                artifact.title = format!("Reviewer {kind} ({})", child.id);
                Some(artifact)
            })
            .collect();
            let mut accounting = child.clone();
            if accounting.ended.is_none() {
                accounting.ended = capture.interrupted_at;
            }
            attempts.push(ReviewerAttempt {
                run: parent.id.clone(),
                round: capture.round,
                state,
                live_pane,
                accounting,
                record,
                artifacts,
            });
        }
    }
    // Durable run/child order records launch sequence even if the wall clock moves backwards.
    Ok(attempts)
}
