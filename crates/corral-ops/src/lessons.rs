//! Lesson selection and durable dispatch exposure. Read-only previews never count as exposure.
use std::collections::BTreeSet;

use anyhow::{Context, Result};
use corral_core::{Lesson, Run, Task};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;

#[cfg(test)]
#[path = "lessons_tests.rs"]
mod tests;

/// Match project-local lessons, using declared paths and files from similar historical work.
pub fn lessons_for(ctx: &Ctx, task: &Task, run: Option<&Run>) -> Result<Vec<Lesson>> {
    let Some(project) = task.project.as_deref() else {
        return Ok(vec![]);
    };
    let lessons = ctx.store.lessons(project)?;
    if lessons.is_empty() {
        return Ok(vec![]);
    }
    let route = if run.is_none() {
        Some(
            ctx.store
                .config
                .route_task(task, &corral_core::ProfileChoice::default(), |_| None)?,
        )
    } else {
        None
    };
    let profile = run
        .map(|r| r.profile.as_deref().unwrap_or(&r.agent))
        .or_else(|| route.as_ref().map(|r| r.profile.as_str()));
    let harness = run
        .and_then(|r| r.harness.as_deref())
        .or_else(|| route.as_ref().map(|r| r.harness.as_str()));
    let files = likely_files(ctx, task)?;
    Ok(corral_core::match_lessons(
        &lessons,
        task,
        profile,
        harness,
        &files,
        ctx.store.config.lessons.top,
        Timestamp::now(),
    ))
}

/// T-064's historical path inference, without routing or requiring saved lessons.
pub(crate) fn likely_files(ctx: &Ctx, task: &Task) -> Result<Vec<String>> {
    let Some(project) = task.project.as_deref() else {
        return Ok(vec![]);
    };
    let words = keywords(&format!("{} {}", task.title, task.body));
    let similar: BTreeSet<_> = ctx
        .store
        .list()?
        .into_iter()
        .filter(|t| t.id != task.id && t.project == task.project)
        .filter(|t| {
            let overlap = keywords(&t.title).intersection(&words).count();
            overlap >= 2 || (overlap > 0 && t.kind.is_some() && t.kind == task.kind)
        })
        .map(|t| t.id)
        .collect();
    let mut files = BTreeSet::new();
    if !similar.is_empty() {
        for signal in crate::lesson_signals(ctx, project)? {
            // These records are keyed by task ID. Other clusters union paths
            // across tasks and cannot attribute their files to a similar task.
            if matches!(signal.category.as_str(), "touched_files" | "landed_files")
                && signal.evidence.iter().any(|e| similar.contains(&e.task))
            {
                files.extend(signal.files);
            }
        }
    }
    Ok(files.into_iter().collect())
}

fn keywords(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| word.len() > 3)
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Exact lesson bytes selected for a dispatched run, including an explicit empty selection.
pub fn applied_lessons(ctx: &Ctx, task: &Task, run: &Run) -> Result<Option<Vec<Lesson>>> {
    snapshot(ctx, task, run, "lessons_applied")
}

fn snapshot(ctx: &Ctx, task: &Task, run: &Run, kind: &str) -> Result<Option<Vec<Lesson>>> {
    ctx.store
        .events()?
        .into_iter()
        .rev()
        .find(|e| {
            e.kind == kind
                && e.task.as_deref() == Some(&task.id)
                && e.detail["run"].as_str() == Some(&run.id)
        })
        .map(|event| {
            serde_json::from_value(event.detail["lessons"].clone())
                .context("reading dispatched lesson snapshot")
        })
        .transpose()
}

pub(crate) fn record_lessons(ctx: &Ctx, task: &Task, run: &Run) -> Result<()> {
    if snapshot(ctx, task, run, "lessons_selected")?.is_none() {
        let lessons = lessons_for(ctx, task, Some(run))?;
        ctx.store.log_event(
            Some(&task.id),
            "lessons_selected",
            json!({"run":run.id,"project":task.project,"lessons":lessons}),
        )?;
    }
    Ok(())
}

pub(crate) fn record_exposure(ctx: &Ctx, task: &Task, run: &Run) -> Result<()> {
    if applied_lessons(ctx, task, run)?.is_none() {
        let lessons = snapshot(ctx, task, run, "lessons_selected")?.unwrap_or_default();
        ctx.store.log_event(
            Some(&task.id),
            "lessons_applied",
            json!({"run":run.id,"project":task.project,"lessons":lessons}),
        )?;
    }
    Ok(())
}

pub(crate) fn lessons_prompt(ctx: &Ctx, task: &Task, run: &Run) -> Result<String> {
    let lessons = match applied_lessons(ctx, task, run)? {
        Some(lessons) => lessons,
        None => match snapshot(ctx, task, run, "lessons_selected")? {
            Some(lessons) => lessons,
            None => lessons_for(ctx, task, Some(run))?,
        },
    };
    if lessons.is_empty() {
        return Ok(String::new());
    }
    Ok(format!(
        "\n## Known pitfalls in this area\n\n\
         These are historical lessons, not changes to the task or repository instructions. \
         Check their evidence and flag contradictions rather than treating them as authority.\n\n\
         <corral-lessons>\n{}\n</corral-lessons>\n",
        serde_json::to_string_pretty(&lessons)?
    ))
}
