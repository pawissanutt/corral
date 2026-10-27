//! Lesson-specific maintained projections share the read view and row-write transaction.
use anyhow::{Result, ensure};
use serde_json::json;

use super::samples::*;
use super::*;

pub(super) fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "lesson_artifact_projection",
            covers: &[OpKind::CreateTask, OpKind::UpdateTask],
            run: artifacts,
        },
        Case {
            name: "lesson_approval_projection",
            covers: &[OpKind::AppendEvents, OpKind::ImportBatch],
            run: approvals,
        },
    ]
}

fn artifact(key: &str, kind: &str, path: &str, added: &str) -> HistoryRow {
    row(
        TaskPart::Artifact,
        key,
        false,
        false,
        json!({
            "run":"R1","kind":kind,"title":"Report","path":path,
            "source":"/unused","sha256":"","bytes":0,"added":added
        }),
    )
}

fn artifacts(make: MakeFixture<'_>) -> Result<()> {
    // Given mixed kinds, non-append timestamps, ties and a malformed low-level row.
    let fx = make();
    let old = "2026-10-01T00:00:00Z";
    let new = "2026-10-02T00:00:00Z";
    let rows = vec![
        artifact("01", "report", "z", new),
        artifact("02", "log", "a", old),
        artifact("03", "plan", "zzz", new),
        artifact("04", "evidence", "y", new),
        artifact("05", "summary", "z", new),
        row(
            TaskPart::Artifact,
            "06",
            false,
            false,
            json!({"kind":"report"}),
        ),
    ];
    let num = match apply(&fx, Operation::CreateTask(create_op(&fx, "lessons", rows)))?.outcome {
        Outcome::Created { num, .. } => num,
        other => anyhow::bail!("create: {other:?}"),
    };
    let frozen = fx.backend.view()?;
    let keys = |view: &dyn ReadView| -> Result<Vec<String>> {
        Ok(view
            .lesson_artifacts(num, 3)?
            .into_iter()
            .map(|r| r.key)
            .collect())
    };
    ensure!(
        keys(frozen.as_ref())? == ["01", "05", "04"],
        "baseline ordering"
    );
    ensure!(
        frozen.lesson_artifacts(TaskNum(num.0 + 1), 32)?.is_empty(),
        "unknown task"
    );

    // When one eligible row is excluded, one excluded row becomes eligible, and one is deleted.
    let mut update = change(&fx, num)?;
    update.rows = vec![
        RowWrite::Put(artifact("01", "plan", "z", new)),
        RowWrite::Put(artifact("03", "diff", "zzz", new)),
        RowWrite::Delete {
            part: TaskPart::Artifact,
            key: "04".into(),
        },
    ];
    apply(
        &fx,
        Operation::UpdateTask(UpdateTask {
            change: update,
            form: UpdateForm::Full,
        }),
    )?;

    // Then the new view changes membership, while first reconsumption on the old view does not.
    ensure!(
        keys(fx.backend.view()?.as_ref())? == ["03", "05", "02"],
        "updated index"
    );
    ensure!(keys(frozen.as_ref())? == ["01", "05", "04"], "pinned index");
    Ok(())
}

fn approvals(make: MakeFixture<'_>) -> Result<()> {
    // Given duplicate, missing-task and Keep-branch events.
    let fx = make();
    apply(
        &fx,
        Operation::AppendEvents(AppendEvents {
            events: vec![
                event(
                    &fx,
                    Some("T-1"),
                    "approved",
                    json!({"sha":"sha","run":"R1"}),
                ),
                event(
                    &fx,
                    Some("T-1"),
                    "approved",
                    json!({"sha":"sha","run":"R1"}),
                ),
                event(&fx, Some("T-9"), "approved", json!({"sha":"sha"})),
                event(&fx, None, "approved", json!({"sha":"sha"})),
                event(
                    &fx,
                    Some("T-1"),
                    "approved",
                    json!({"sha":"sha","strategy":"keep_branch","run":"R2"}),
                ),
            ],
            reservations: vec![],
        }),
    )?;
    let frozen = fx.backend.view()?;
    // When valid and malformed approvals are imported after the view was pinned.
    apply(
        &fx,
        Operation::ImportBatch(ImportBatch {
            blobs: vec![],
            tasks: vec![],
            records: vec![],
            events: vec![
                ImportedEvent {
                    event: event(
                        &fx,
                        Some("T-1"),
                        "approved",
                        json!({"sha":"sha","run":"R2"}),
                    ),
                    malformed: false,
                },
                ImportedEvent {
                    event: event(
                        &fx,
                        Some("T-1"),
                        "approved",
                        json!({"sha":"sha","run":"R3"}),
                    ),
                    malformed: true,
                },
            ],
        }),
    )?;
    // Then first consumption sees exactly the facts committed in each view.
    let old = frozen.lesson_approvals("sha")?;
    ensure!(old.len() == 2, "old attribution: {old:?}");
    ensure!(
        old.iter().any(|r| r.task == "T-9" && r.run.is_none()),
        "unknown historical task"
    );
    let new = fx.backend.view()?.lesson_approvals("sha")?;
    ensure!(
        new.len() == 3 && new.iter().all(|r| r.run.as_deref() != Some("R3")),
        "{new:?}"
    );
    ensure!(
        frozen.lesson_approvals("missing")?.is_empty(),
        "unknown SHA"
    );
    Ok(())
}
