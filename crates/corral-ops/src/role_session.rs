//! Task-scoped conversations. Accounting attempts and callback authority are never resumed.
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use anyhow::{Context, Result};

use anyhow::{bail, ensure};
use corral_core::{ResolvedProfile, RoleSession, Run, SessionAction, SessionPolicy, Task};
use serde_json::Value;

use serde_json::json;

use crate::Ctx;

use crate::provider_session;

mod claude;
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

pub(crate) fn fresh(ctx: &Ctx, task: &Task, run: &Run, reviewer: bool) -> Result<RoleSession> {
    let policy = ctx.store.config.task_review_policy(task);
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce)
        .map_err(|error| anyhow::anyhow!("allocating role session identity: {error}"))?;
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
        directory: ctx
            .store
            .state_dir()
            .join("role-sessions")
            .join(&task.id)
            .join(&run.id)
            .join(nonce),
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
    let mut session = match &run.session {
        Some(session) => session.clone(),
        None => fresh(ctx, task, run, reviewer)?,
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
    let native = run
        .agent_host
        .as_ref()
        .and_then(|host| host.current_generation())
        .and_then(|generation| generation.bound.as_ref());
    let path = if let Some(bound) = native {
        Some(bound.session_file.clone())
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
        provider_session::discover(
            ctx,
            run,
            provider_session::boundary(ctx, &task.id, run),
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
    if let Some(bound) = native {
        ensure!(
            id == bound.session_id,
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
pub(crate) fn reviewer(ctx: &Ctx, task: &Task, parent: &Run, child: &Run) -> Result<RoleSession> {
    let mut next = fresh(ctx, task, child, true)?;
    if next.policy == SessionPolicy::Fresh {
        return Ok(next);
    }
    let Some(previous) = parent.review_runs.last() else {
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
        let mut session = capture(ctx, task, previous, true)?;
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
    if corral_core::is_pi_native(&resolved.harness) {
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

pub(crate) fn log(ctx: &Ctx, task: &Task, run: &Run) -> Result<()> {
    let current = ctx.store.get(&task.id)?;
    let saved = current.current_run().and_then(|parent| {
        if parent.id == run.id {
            Some(parent)
        } else {
            parent.review_runs.iter().find(|child| child.id == run.id)
        }
    });
    if let Some(session) = saved.and_then(|run| run.session.as_ref()) {
        ctx.store.log_event(
            Some(&task.id),
            "role_session",
            json!({
                "run":run.id, "session":session
            }),
        )?;
        if let Some(reason) = &session.diagnostic {
            crate::messages::report(Some(&task.id), "session", reason.clone());
        }
    }
    Ok(())
}
