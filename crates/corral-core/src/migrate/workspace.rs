//! Workspace records added after the original rehearsal, plus legacy runtime relocation.
use anyhow::{Context, Result, ensure};
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};

pub(super) fn markdown<T: DeserializeOwned>(path: &Path) -> Result<(T, String)> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let (yaml, body) = text
        .strip_prefix("---\n")
        .and_then(|s| s.split_once("\n---\n"))
        .with_context(|| format!("{} has no frontmatter", path.display()))?;
    Ok((
        serde_yaml_ng::from_str(yaml).with_context(|| format!("parsing {}", path.display()))?,
        body.to_owned(),
    ))
}

pub(super) fn assistants(corral: &Path) -> Result<Vec<crate::assistant::AssistantSession>> {
    let dir = corral.join("assistants");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut sessions = Vec::new();
    for entry in std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if super::classify::classify(&format!(
            "assistants/{}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ))
        .is_none()
        {
            continue;
        }
        let (mut session, body): (crate::assistant::AssistantSession, _) = markdown(&path)?;
        crate::assistant::validate_id(&session.id)?;
        ensure!(
            path.file_stem().is_some_and(|s| s == session.id.as_str())
                && session.version == crate::assistant::ASSISTANT_VERSION,
            "assistant identity mismatch in {}",
            path.display()
        );
        session.body = body;
        sessions.push(session);
    }
    sessions.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(sessions)
}

fn relocate_path(corral: &Path, path: &mut PathBuf) {
    if let Ok(rest) = path.strip_prefix(corral.join("state/role-sessions"))
        && rest
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
    {
        *path = corral.join("agent/sessions/legacy-role").join(rest);
    }
}

fn host(corral: &Path, state: &mut crate::agent::AgentRunState) {
    for generation in &mut state.generations {
        if let Some(bound) = &mut generation.bound {
            relocate_path(corral, &mut bound.session_file);
        }
    }
    for session in &mut state.usage_sessions {
        relocate_path(corral, &mut session.session_file);
    }
}

fn inbox(corral: &Path, inbox: &mut crate::inbox::InboxState) {
    if let Some(binding) = &mut inbox.binding
        && let Some(transcript) = &mut binding.transcript
    {
        relocate_path(corral, transcript);
    }
}

fn run(corral: &Path, run: &mut crate::Run) {
    if let Some(session) = &mut run.session {
        relocate_path(corral, &mut session.directory);
        if let Some(transcript) = &mut session.transcript {
            relocate_path(corral, transcript);
        }
    }
    inbox(corral, &mut run.inbox);
    if let Some(state) = &mut run.agent_host {
        host(corral, state);
    }
    for child in run.review_runs.iter_mut() {
        self::run(corral, child);
    }
}

/// Only typed runtime paths move. Owner bodies, tool results and other historical JSON do not.
pub(super) fn relocate_task(corral: &Path, task: &mut crate::Task) {
    for coding in &mut task.runs {
        run(corral, coding);
    }
    for planning in &mut task.plan_runs {
        inbox(corral, &mut planning.inbox);
        if let Some(state) = &mut planning.agent_host {
            host(corral, state);
        }
    }
    for plan in &mut task.plans {
        for review in plan.reviews.iter_mut() {
            inbox(corral, &mut review.inbox);
            if let Some(state) = &mut review.agent_host {
                host(corral, state);
            }
        }
    }
}

/// Workspace host documents remain opaque except for their declared runtime path fields.
pub(super) fn relocate_workspace_host(corral: &Path, doc: &mut serde_json::Value) {
    if let Some(generations) = doc
        .get_mut("state")
        .and_then(|v| v.get_mut("generations"))
        .and_then(serde_json::Value::as_array_mut)
    {
        for generation in generations {
            if let Some(file) = generation["bound"]["session_file"].as_str() {
                let mut path = PathBuf::from(file);
                relocate_path(corral, &mut path);
                generation["bound"]["session_file"] = serde_json::json!(path);
            }
        }
    }
    if let Some(sessions) = doc
        .get_mut("state")
        .and_then(|v| v.get_mut("usage_sessions"))
        .and_then(serde_json::Value::as_array_mut)
    {
        for session in sessions {
            if let Some(file) = session["session_file"].as_str() {
                let mut path = PathBuf::from(file);
                relocate_path(corral, &mut path);
                session["session_file"] = serde_json::json!(path);
            }
        }
    }
}

pub(super) fn runtime_path(corral: &Path, legacy: &str) -> Result<PathBuf> {
    let rest = legacy
        .strip_prefix("state/role-sessions/")
        .context("legacy role session path")?;
    ensure!(
        Path::new(rest)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_))),
        "invalid role session path"
    );
    Ok(corral.join("agent/sessions/legacy-role").join(rest))
}

/// Copy without overwriting existing runtime data; a resumed import verifies the existing copy.
pub(super) fn copy_runtime(corral: &Path, legacy: &str) -> Result<()> {
    let source = corral.join(legacy);
    ensure!(
        std::fs::symlink_metadata(&source)?.is_file(),
        "runtime source is not a regular file: {legacy}"
    );
    let destination = runtime_path(corral, legacy)?;
    let relative = destination.strip_prefix(corral)?;
    let mut parent = corral.to_path_buf();
    for part in relative.parent().context("runtime parent")?.components() {
        parent.push(part);
        match super::durable::seen(&parent)? {
            super::durable::Seen::Absent => {
                std::fs::create_dir(&parent)?;
                super::durable::sync_dir(parent.parent().context("runtime parent")?)?;
            }
            super::durable::Seen::Dir => {}
            _ => anyhow::bail!(
                "runtime directory is not a real directory: {}",
                parent.display()
            ),
        }
    }
    match super::durable::seen(&destination)? {
        super::durable::Seen::Absent => {
            use std::io::Write;
            let mut input = std::fs::File::open(&source)?;
            let parent = destination.parent().context("runtime parent")?;
            let mut output =
                tempfile::NamedTempFile::new_in(parent).context("staging legacy runtime copy")?;
            std::io::copy(&mut input, &mut output)?;
            output.flush()?;
            output.as_file().sync_all()?;
            output
                .persist_noclobber(&destination)
                .with_context(|| format!("publishing runtime copy {}", destination.display()))?;
            super::durable::sync_dir(parent)?;
        }
        super::durable::Seen::Other if std::fs::symlink_metadata(&destination)?.is_file() => {
            ensure!(
                super::scan::sha256_file(&source)? == super::scan::sha256_file(&destination)?,
                "existing runtime copy differs: {}",
                destination.display()
            );
        }
        _ => anyhow::bail!(
            "runtime copy is not a regular file: {}",
            destination.display()
        ),
    }
    Ok(())
}
