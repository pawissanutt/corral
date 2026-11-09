use super::super::publication::load_artifact;
use anyhow::{Context, Result, ensure};
use corral_core::{
    Store,
    agent::{ContainerRef, paths},
    assistant::AssistantContainer,
    assistant_workflow::*,
    blob_ref::BlobHash,
};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;

pub(super) fn file(store: &Store, session: &str, review: &LocalReview) -> PathBuf {
    paths::usage_session_file(
        store,
        &ContainerRef::Assistant {
            session: session.into(),
        },
        &review.invocation,
    )
    .with_extension("plan.md")
}

/// A regular read-tool input, derived only from the authenticated service identity. The blob
/// remains canonical/retained; this read-only materialization is never caller-selected.
pub(super) fn materialize(
    store: &Store,
    session: &str,
    container: &AssistantContainer,
    review: &LocalReview,
) -> Result<()> {
    let bytes = load_artifact(store, &review.plan.artifact)?;
    let path = file(store, session, review);
    write_exact(&path, &bytes)?;
    write_exact(
        &path.with_extension("context.json"),
        &load_artifact(store, &review.context)?,
    )?;
    container.validate_protocol()?;
    Ok(())
}

fn write_exact(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(path.parent().context("snapshot directory")?)
        .context("create review snapshot directory")?;
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut f) => {
            f.write_all(&bytes).context("write exact review snapshot")?;
            f.sync_all().context("sync review snapshot")?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            ensure!(
                fs::symlink_metadata(&path)?.file_type().is_file(),
                "review snapshot is not regular"
            );
            let mut existing = Vec::new();
            fs::File::open(&path)?
                .take(DOCUMENT_MAX as u64 + 1)
                .read_to_end(&mut existing)?;
            ensure!(
                existing == bytes,
                "review snapshot differs from canonical bytes"
            );
        }
        Err(error) => return Err(error).context("create exact review snapshot"),
    }
    Ok(())
}

pub(super) fn context(
    store: &Store,
    session: &str,
    container: &AssistantContainer,
    plan: &PlanRevision,
) -> Result<DesignArtifact> {
    let w = super::current(container)?;
    let mut decisions = Vec::new();
    for a in &w.decision_refs {
        decisions.push(String::from_utf8(load_artifact(store, a)?)?);
    }
    let decode =
        |a: &DesignArtifact| -> Result<String> { Ok(String::from_utf8(load_artifact(store, a)?)?) };
    let value = serde_json::json!({
        "original_request": decode(&w.original_request)?, "decisions_and_unanswered_choices": decisions,
        "execution_scope": plan.execution_scope.as_ref().map(decode).transpose()?,
        "source_assumptions": plan.assumptions.as_ref().map(decode).transpose()?,
        "stage": plan.stage, "revision": plan.revision, "digest": plan.artifact.blob.hash(),
        "advisory_only": true,
    });
    super::artifact(store, session, &w.id, &serde_json::to_vec(&value)?)
}

/// Observe bounded relevant files, including absence and dirty bytes, independently of plan
/// freshness. No shell, hooks, filters or index refresh. Drafts without scope are unobserved.
pub(super) fn observe(store: &Store, plan: &PlanRevision) -> Result<Option<BlobHash>> {
    let Some(a) = &plan.assumptions else {
        return Ok(None);
    };
    let assumptions: SourceAssumptions = serde_json::from_slice(&load_artifact(store, a)?)?;
    let Some(project) = store.config.projects.get(&assumptions.project) else {
        return Ok(None);
    };
    let root = std::path::Path::new(&project.path)
        .canonicalize()
        .context("review source project")?;
    let head = std::process::Command::new("git")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-C",
        ])
        .arg(&root)
        .args(["rev-parse", "--verify", "HEAD^{commit}"])
        .stdin(std::process::Stdio::null())
        .output()
        .context("observe review source HEAD")?;
    if !head.status.success() {
        return Ok(None);
    }
    let head = String::from_utf8(head.stdout)?;
    if !matches!(head.trim().len(), 40 | 64) {
        return Ok(None);
    }
    let mut facts = Vec::new();
    for source in &assumptions.files {
        let path = root.join(&source.path);
        let observation = match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_file() && meta.len() <= DOCUMENT_MAX as u64 => {
                let canonical = path.canonicalize()?;
                ensure!(
                    canonical.starts_with(&root),
                    "review source escapes registered project"
                );
                let mut bytes = Vec::new();
                fs::File::open(canonical)?
                    .take(DOCUMENT_MAX as u64 + 1)
                    .read_to_end(&mut bytes)?;
                ensure!(
                    bytes.len() <= DOCUMENT_MAX,
                    "review source exceeds observation bound"
                );
                Some(BlobHash::of(&bytes))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            _ => return Ok(None), // Unsupported/oversized source is unobserved, never current.
        };
        facts.push((&source.path, observation));
    }
    Ok(Some(BlobHash::of(&serde_json::to_vec(&(
        root, head, facts,
    ))?)))
}
