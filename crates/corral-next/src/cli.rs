use std::io::Write;

use anyhow::{Context, Result};
use corral_engine::Engine;
use serde_json::json;

use crate::output::{Output, PendingVerdictView, TaskView};

pub(crate) fn show(engine: &Engine, out: &mut Output<'_>, id: &str) -> Result<()> {
    let view = engine.store.read_snapshot(|| {
        let task = engine.store.require_task(id)?;
        let pending_review_verdict = engine
            .pending_review_verdict(&task)?
            .map(|pending| {
                let root = engine
                    .root
                    .to_str()
                    .context("workspace root is not UTF-8")?;
                let retry = shlex::try_join([
                    out.program,
                    "--root",
                    root,
                    "review-verdict",
                    &pending.task,
                    &pending.run,
                    &pending.request,
                ])
                .context("format verdict retry command")?;
                Ok::<_, anyhow::Error>(PendingVerdictView { pending, retry })
            })
            .transpose()?;
        Ok(TaskView {
            references: engine.artifact_references(&task.body),
            task,
            runs: engine.store.list_runs(id)?,
            checks: engine.store.list_latest_checks(id)?,
            artifacts: engine.list_artifacts(Some(id))?,
            pending_review_verdict,
        })
    })?;
    out.show(&view)
}

pub(crate) fn review_verdict(
    engine: &Engine,
    out: &mut Output<'_>,
    task: &str,
    run: &str,
    request: &str,
) -> Result<()> {
    engine.record_review_verdict(task, run, request)?;
    out.value(&json!({"recorded": true}), |out, _| {
        writeln!(out, "Verdict recorded for {task} {run}")
    })
}
