//! `knowledge` command executors.
use anyhow::{Context, Result, ensure};
use corral_core::memory::{MemoryEntry, MemoryRole, MemoryScope, MemorySelector};
use corral_protocol::cmd::knowledge::KnowledgeCommand;

use super::{Exec, Verb};
use crate::memory::{MemoryActor, authorize};

impl Verb for KnowledgeCommand {
    fn execute(self, cx: &mut Exec<'_>) -> Result<()> {
        match self {
            Self::Distill {
                project,
                profile,
                json,
            } => {
                let project = project_for(cx, project.as_deref())?;
                let task = crate::lessons_distill(&cx.ctx, &project, profile.as_deref())?;
                if json {
                    let mut value = serde_json::to_value(&task)?;
                    value["body"] = serde_json::Value::String(task.body.clone());
                    value["runs"] = serde_json::to_value(&task.runs)?;
                    cx.out.json_pretty(&value)?;
                } else {
                    cx.out.line(format_args!(
                        "{}: {} (filed, not dispatched)",
                        task.id, task.title
                    ));
                }
                Ok(())
            }
            Self::Apply { project, file } => {
                let project = project_for(cx, project.as_deref())?;
                let proposal =
                    serde_json::from_str(&file.text).context("parsing lesson proposal")?;
                crate::lessons_apply(&cx.ctx, &project, &proposal)?;
                cx.out.line(format_args!("Applied lessons for {project}"));
                Ok(())
            }
            Self::Pin { project, id, unpin } => {
                let project = project_for(cx, project.as_deref())?;
                cx.ctx.store.pin_lesson(&project, &id, !unpin)?;
                cx.out.line(format_args!(
                    "{} {id}",
                    if unpin { "Unpinned" } else { "Pinned" }
                ));
                Ok(())
            }
            Self::Delete { project, id } => {
                let project = project_for(cx, project.as_deref())?;
                cx.ctx.store.delete_lesson(&project, &id)?;
                cx.out.line(format_args!("Deleted {id}"));
                Ok(())
            }
            Self::Put {
                scope,
                key,
                id,
                file,
                expected_hash,
                ..
            } => {
                let (scope, author) = memory_scope(cx, &scope, key)?;
                let value: MemoryEntry =
                    serde_json::from_str(&file.text).context("parsing MemoryEntry JSON")?;
                let result = cx.ctx.store.memory_put(
                    &MemorySelector { scope, id },
                    value,
                    expected_hash.as_deref(),
                    author,
                    jiff::Timestamp::now(),
                )?;
                cx.out.json_pretty(&serde_json::to_value(result)?)
            }
            Self::Remove {
                scope,
                key,
                id,
                expected_hash,
            } => {
                let (scope, _) = memory_scope(cx, &scope, key)?;
                cx.ctx
                    .store
                    .memory_remove(&MemorySelector { scope, id }, &expected_hash)?;
                cx.out.json_pretty(&serde_json::json!({"removed": true}))
            }
            Self::Other => Err(super::unknown_command("other")),
        }
    }
}

fn project_for(cx: &Exec<'_>, selected: Option<&str>) -> Result<String> {
    selected
        .map(str::to_string)
        .or_else(|| {
            cx.invoker
                .cwd()
                .ok()
                .and_then(|cwd| crate::infer_project(&cx.ctx, &cwd))
        })
        .or_else(|| {
            (cx.ctx.store.config.projects.len() == 1)
                .then(|| cx.ctx.store.config.projects.keys().next().cloned())
                .flatten()
        })
        .context("select a project with --project")
}

fn memory_scope(
    cx: &Exec<'_>,
    scope: &str,
    key: Option<String>,
) -> Result<(MemoryScope, &'static str)> {
    let author = cx.invoker.decision_author(&cx.ctx.store)?;
    let env = &cx.invoker.caller.env;
    ensure!(
        author == "owner"
            && [
                &env.corral_assistant,
                &env.corral_pm,
                &env.corral_agent_bootstrap,
                &env.corral_task,
            ]
            .iter()
            .all(|value| value.is_none()),
        "bound agents must use authenticated memory operations"
    );
    let scope = match (scope, key) {
        ("workspace", None) => MemoryScope::Workspace,
        ("project", Some(key)) => MemoryScope::Project(key),
        ("role", Some(key)) => MemoryScope::Role(match key.as_str() {
            "pm" => MemoryRole::Pm,
            "assistant" => MemoryRole::Assistant,
            "worker" => MemoryRole::Worker,
            _ => anyhow::bail!("role must be pm, assistant or worker"),
        }),
        _ => anyhow::bail!("workspace has no --key; project and role require --key"),
    };
    authorize(&cx.ctx.store, &MemoryActor::Operator, &scope, true)?;
    Ok((scope, author))
}
