//! Task-scoped conversations. Accounting attempts and callback authority are never resumed.
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use anyhow::{Context, Result};

use anyhow::{bail, ensure};
use corral_core::agent::ContainerRef;
use corral_core::{
    ResolvedProfile, RoleSession, Run, RunHead, SessionAction, SessionPolicy, Task, TaskFieldsRef,
    TaskRef,
};
use serde_json::Value;

use serde_json::json;

use crate::Ctx;

use crate::provider_session;

mod claude;
mod entropy;
pub use entropy::{FixtureRoleEntropyGuard, fixture_role_entropy};
#[cfg(test)]
mod heads_tests;
#[cfg(test)]
mod tests;

#[derive(Debug)]
pub(crate) struct SessionUnavailable(pub String);

impl std::fmt::Display for SessionUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SessionUnavailable {}

fn adapter<'a>(ctx: &'a Ctx, harness: &'a str) -> &'a str {
    ctx.store
        .config
        .harnesses
        .get(harness)
        .and_then(|h| h.flags.as_deref())
        .unwrap_or(harness)
}

pub(crate) fn fresh<T: TaskFieldsRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &Run,
    reviewer: bool,
) -> Result<RoleSession> {
    fresh_head(ctx, task, &RunHead::of(run)?, reviewer)
}

pub(crate) fn fresh_head<T: TaskFieldsRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
    reviewer: bool,
) -> Result<RoleSession> {
    let policy = ctx.store.config.task_review_policy(task);
    let nonce = entropy::nonce()?;
    let nonce: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let harness = run.harness.as_deref().unwrap_or(&run.agent);
    let selected_adapter = run
        .execution
        .as_ref()
        .and(run.session.as_ref())
        .and_then(|session| session.adapter.as_deref())
        .unwrap_or_else(|| adapter(ctx, harness));
    let mut session = RoleSession {
        policy: if reviewer {
            policy.reviewer_session
        } else {
            policy.worker_session
        },
        id: None,
        transcript: None,
        directory: corral_core::agent::paths::sessions_dir(&ctx.store)
            .join(task.id())
            .join(&run.id)
            .join(format!("role-{nonce}")),
        harness: harness.into(),
        adapter: Some(selected_adapter.into()),
        model: run.model.clone(),
        vendor: run.vendor.clone(),
        action: SessionAction::New,
        diagnostic: None,
    };
    if selected_adapter == "claude" {
        if let Some((id, directory)) = claude::location(&run.worktree, &session.directory) {
            session.transcript = Some(directory.join(format!("{id}.jsonl")));
            session.directory = directory;
            session.id = Some(id);
        } else {
            session.diagnostic = Some("Claude session directory unavailable: HOME is unset".into());
        }
    }
    Ok(session)
}

fn header(path: &Path, cwd: &Path) -> Result<String> {
    let meta = std::fs::symlink_metadata(path)
        .with_context(|| format!("session unavailable: {}", path.display()))?;
    ensure!(
        meta.file_type().is_file(),
        "session is not a regular file: {}",
        path.display()
    );
    let mut reader =
        BufReader::new(File::open(path).context("opening role session")?).take(64 * 1024);
    let mut line = Vec::new();
    for _ in 0..64 {
        line.clear();
        if reader
            .read_until(b'\n', &mut line)
            .context("reading role session header")?
            == 0
        {
            break;
        }
        let value: Value = serde_json::from_slice(&line).context("invalid session header")?;
        let id = if value["type"] == "session" {
            value["id"].as_str()
        } else {
            value["sessionId"].as_str()
        };
        if let Some(id) = id.filter(|id| !id.is_empty()) {
            ensure!(
                value["cwd"].as_str().map(Path::new) == Some(cwd),
                "session cwd is incompatible"
            );
            return Ok(id.to_owned());
        }
    }
    bail!("session header has no exact identity")
}

/// Capture only a saved binding or a unique transcript in this role's private directory.
pub(crate) fn capture(ctx: &Ctx, task: &Task, run: &Run, reviewer: bool) -> Result<RoleSession> {
    let native = run
        .agent_host
        .as_deref()
        .and_then(|host| host.current_generation())
        .and_then(|generation| generation.bound.as_ref())
        .map(|bound| (bound.session_id.clone(), bound.session_file.clone()));
    capture_metadata(ctx, task, &RunHead::of(run)?, reviewer, native)
}

pub(crate) fn capture_head<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
    container: &ContainerRef,
) -> Result<RoleSession> {
    let native = match run.agent_host.as_deref() {
        Some(host) => task
            .agent_generation(container, host.binding.generation)?
            .and_then(|generation| generation.bound)
            .map(|bound| (bound.session_id, bound.session_file)),
        None => None,
    };
    capture_metadata(
        ctx,
        task,
        run,
        matches!(container, ContainerRef::Reviewer { .. }),
        native,
    )
}

fn capture_metadata<T: TaskFieldsRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
    reviewer: bool,
    native: Option<(String, std::path::PathBuf)>,
) -> Result<RoleSession> {
    let mut session = match &run.session {
        Some(session) => session.clone(),
        None => fresh_head(ctx, task, run, reviewer)?,
    };
    if session.adapter.is_none() {
        session.adapter = Some(adapter(ctx, &session.harness).into());
    }
    ensure!(
        session.harness == run.harness.as_deref().unwrap_or(&run.agent)
            && session.model == run.model
            && session.vendor == run.vendor,
        "session harness/model/vendor is incompatible"
    );
    let path = if let Some((_, path)) = &native {
        Some(path.clone())
    } else if let Some(path) = &session.transcript {
        Some(path.clone())
    } else if let Some(path) = run
        .inbox
        .binding
        .as_ref()
        .and_then(|b| b.transcript.clone())
    {
        Some(path)
    } else if run.session.is_some() {
        let entries = match std::fs::read_dir(&session.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                bail!(
                    "session unavailable: no transcript in {}",
                    session.directory.display()
                )
            }
            Err(error) => return Err(error).context("listing role session directory"),
        };
        let files: Vec<_> = entries
            .collect::<std::io::Result<Vec<_>>>()
            .context("reading role session directory entries")?
            .into_iter()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .collect();
        ensure!(
            files.len() == 1,
            "session unavailable or ambiguous: {} transcripts in {}",
            files.len(),
            session.directory.display()
        );
        files.into_iter().next()
    } else {
        provider_session::discover_head(
            ctx,
            run,
            provider_session::boundary_head(ctx, task.id(), &run.id, run.started),
            provider_session::Scope::Created {
                until: run.ended.or_else(|| run.review.as_ref().map(|r| r.at)),
            },
        )
    }
    .context("session unavailable: no recorded conversation")?;
    let id = header(&path, &run.worktree)?;
    if let Some(expected) = session.id.as_deref() {
        ensure!(
            id == expected,
            "session identity changed: expected {expected}, found {id}"
        );
    }
    if let Some((session_id, _)) = native {
        ensure!(
            id == session_id,
            "native session identity does not match its binding"
        );
    }
    session.id = Some(id);
    session.directory = path
        .parent()
        .context("session has no directory")?
        .to_path_buf();
    session.transcript = Some(path);
    Ok(session)
}

/// One explicit fresh replacement on incompatibility; never label a lost conversation reused.
#[cfg(test)]
pub(crate) fn reviewer(ctx: &Ctx, task: &Task, parent: &Run, child: &Run) -> Result<RoleSession> {
    reviewer_head(ctx, task, &RunHead::of(parent)?, child)
}

pub(crate) fn reviewer_head<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    parent: &RunHead,
    child: &Run,
) -> Result<RoleSession> {
    let mut next = fresh(ctx, task, child, true)?;
    if next.policy == SessionPolicy::Fresh {
        return Ok(next);
    }
    let Some(previous) = parent.current_child.as_deref() else {
        return Ok(next);
    };
    let reused = (|| {
        ensure!(
            previous.harness == child.harness
                && previous.model == child.model
                && previous.vendor == child.vendor
                && previous.worktree == child.worktree,
            "reviewer session is incompatible with the selected harness/model/vendor/worktree"
        );
        let container = ContainerRef::Reviewer {
            task: task.id().clone(),
            run: parent.id.clone(),
            review: previous.id.clone(),
        };
        let mut session = capture_head(ctx, task, previous, &container)?;
        ensure!(
            session.adapter == next.adapter,
            "reviewer session adapter is incompatible"
        );
        session.policy = next.policy;
        session.action = SessionAction::Resumed;
        Ok::<_, anyhow::Error>(session)
    })();
    match reused {
        Ok(session) => Ok(session),
        Err(error) => {
            next.diagnostic = Some(format!(
                "Reviewer context not reused; starting one fresh attempt: {error:#}"
            ));
            Ok(next)
        }
    }
}

/// Add concrete adapter arguments before the current prompt, not a directory-wide `continue`.
pub(crate) fn command(ctx: &Ctx, run: &Run, resolved: &ResolvedProfile) -> Result<ResolvedProfile> {
    command_head(ctx, &RunHead::of(run)?, resolved)
}

pub(crate) fn command_head(
    ctx: &Ctx,
    run: &RunHead,
    resolved: &ResolvedProfile,
) -> Result<ResolvedProfile> {
    if corral_core::is_corral_pi(&resolved.harness) {
        return Ok(resolved.clone());
    }
    let Some(session) = &run.session else {
        return Ok(resolved.clone());
    };
    if matches!(
        session.action,
        SessionAction::Reused | SessionAction::Resumed
    ) {
        ensure!(
            session.harness == resolved.harness
                && session.model == resolved.model
                && session.vendor == resolved.vendor,
            "recorded session is incompatible with the launch profile's harness/model/vendor"
        );
    }
    let adapter = session
        .adapter
        .as_deref()
        .unwrap_or_else(|| adapter(ctx, &resolved.harness));
    let quote = |path: &Path| {
        format!(
            "'{}'",
            crate::prompt::sh_escape(&crate::prompt::shell_path(path))
        )
    };
    let args = match adapter {
        "omo" | "omp" => {
            std::fs::create_dir_all(&session.directory)
                .context("creating role session directory")?;
            let mut args = format!("--session-dir {}", quote(&session.directory));
            if matches!(
                session.action,
                SessionAction::Resumed | SessionAction::Reused
            ) {
                let path = session
                    .transcript
                    .as_deref()
                    .context("resume has no exact transcript")?;
                let id = header(path, &run.worktree)?;
                ensure!(
                    session.id.as_deref() == Some(id.as_str()),
                    "resume session identity changed"
                );
                args.push_str(&format!(
                    " {} {}",
                    if adapter == "omo" {
                        "--session"
                    } else {
                        "--resume"
                    },
                    quote(path)
                ));
            }
            args
        }
        "claude" => {
            let id = session
                .id
                .as_deref()
                .context("Claude session has no exact UUID")?;
            match session.action {
                SessionAction::New => format!("--session-id '{}'", crate::prompt::sh_escape(id)),
                SessionAction::Reused | SessionAction::Resumed => {
                    let path = session
                        .transcript
                        .as_deref()
                        .context("Claude resume has no transcript")?;
                    ensure!(
                        header(path, &run.worktree)? == id,
                        "Claude resume session identity changed"
                    );
                    format!("--resume '{}'", crate::prompt::sh_escape(id))
                }
            }
        }
        _ => {
            ensure!(
                session.action == SessionAction::New,
                "session resume unsupported for harness {}; choose fresh mode explicitly",
                resolved.harness
            );
            return Ok(resolved.clone());
        }
    };
    ensure!(
        !resolved.command.split_whitespace().any(|word| matches!(
            word.trim_matches(['\'', '"'])
                .split('=')
                .next()
                .unwrap_or(""),
            "--session"
                | "--session-id"
                | "--session-dir"
                | "--resume"
                | "--continue"
                | "--fork"
                | "-c"
                | "-r"
        )),
        "configured session selector conflicts with corral's task-scoped session adapter"
    );
    let mut resolved = resolved.clone();
    if resolved.command.contains("{session_args}") {
        resolved.command = resolved.command.replace("{session_args}", &args);
    } else if resolved.command.contains("{model_args}") {
        resolved.command = resolved
            .command
            .replace("{model_args}", &format!("{{model_args}} {args}"));
    } else if resolved.command.contains("{prompt_ref}") {
        resolved.command = resolved
            .command
            .replace("{prompt_ref}", &format!("{args} {{prompt_ref}}"));
    } else {
        resolved.command.push_str(&format!(" {args}"));
    }
    Ok(resolved)
}

pub(crate) fn observe(ctx: &Ctx, task: &Task, run: &Run, reviewer: bool) {
    let captured = capture(ctx, task, run, reviewer);
    if let Ok(session) = captured {
        let result = ctx.store.update(&task.id, |task| {
            let parent = task
                .current_run_mut()
                .context("session parent disappeared")?;
            let current = if reviewer {
                parent
                    .review_runs
                    .iter_mut()
                    .find(|child| child.id == run.id)
                    .context("session reviewer disappeared")?
            } else {
                ensure!(parent.id == run.id, "session worker changed");
                parent
            };
            current.session = Some(session.clone());
            Ok(())
        });
        if let Err(error) = result {
            crate::messages::report(Some(&task.id), "session", format!("{error:#}"));
        }
    }
}

/// Observe the actual current worker/reviewer header and keyed native generation.
pub(crate) fn observe_head<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
    container: &ContainerRef,
) {
    if let Ok(session) = capture_head(ctx, task, run, container) {
        let result = ctx.store.update_head(task.id(), |current| {
            match container {
                ContainerRef::Worker { run, .. } => {
                    current
                        .current_run_mut()
                        .filter(|current| current.id == *run)
                        .context("session worker changed")?
                        .session = Some(session.clone());
                }
                ContainerRef::Reviewer { run, review, .. } => {
                    ensure!(
                        current
                            .head()
                            .current_run
                            .as_ref()
                            .is_some_and(|parent| parent.id == *run),
                        "session parent disappeared"
                    );
                    current.edit_review_child(run, review, |child| {
                        child.session = Some(session.clone());
                        Ok(())
                    })?;
                }
                _ => bail!("not a coding role session"),
            }
            Ok(())
        });
        if let Err(error) = result {
            crate::messages::report(Some(task.id()), "session", format!("{error:#}"));
        }
    }
}

/// Capture and publish the current reviewer's actual session in its owning task unit.
/// The caller then appends review_started, preserving the incoming-main audit order.
pub(crate) fn publish_reviewer(
    ctx: &Ctx,
    task: &mut corral_core::TaskHeadMut,
    parent: &str,
) -> Result<()> {
    let Some(child) = task
        .head()
        .current_run
        .as_deref()
        .filter(|run| run.id == parent)
        .and_then(|run| run.current_child.as_deref())
        .cloned()
    else {
        return Ok(());
    };
    let container = ContainerRef::Reviewer {
        task: task.id.clone(),
        run: parent.into(),
        review: child.id.clone(),
    };
    let session = capture_head(ctx, &*task, &child, &container)
        .ok()
        .or(child.session);
    if let Some(session) = session {
        task.edit_review_child(parent, &child.id, |current| {
            current.session = Some(session.clone());
            Ok(())
        })?;
        log_value(ctx, &task.id, &child.id, &session)?;
    }
    Ok(())
}

/// Refresh identities that an asynchronously launched harness has since written.
pub fn refresh_role_sessions(ctx: &Ctx, id: &str) -> Result<()> {
    let task = ctx.store.get(id)?;
    if let Some(run) = task.current_run() {
        observe(ctx, &task, run, false);
        for child in &run.review_runs {
            observe(ctx, &task, child, true);
        }
    }
    Ok(())
}

pub(crate) fn log(ctx: &Ctx, task: &str, run: &str) -> Result<()> {
    let current = ctx.store.head(task)?;
    let saved = match current.current_run.as_deref() {
        Some(parent) if parent.id == run => parent.session.clone(),
        Some(parent) => match parent
            .current_child
            .as_deref()
            .filter(|child| child.id == run)
        {
            Some(child) => child.session.clone(),
            None => current
                .review_child(&parent.id, run)?
                .and_then(|child| child.session),
        },
        None => None,
    };
    if let Some(session) = saved {
        log_value(ctx, task, run, &session)?;
    }
    Ok(())
}

fn log_value(ctx: &Ctx, task: &str, run: &str, session: &RoleSession) -> Result<()> {
    ctx.store.log_event(
        Some(task),
        "role_session",
        json!({
            "run":run, "session":session
        }),
    )?;
    if let Some(reason) = &session.diagnostic {
        crate::messages::report(Some(task), "session", reason.clone());
    }
    Ok(())
}
