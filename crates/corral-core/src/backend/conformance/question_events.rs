//! Question outbox/ack atomicity and identity dedup run unchanged on both backends.
use super::{samples::*, *};
use anyhow::{Result, ensure};
use serde_json::json;

pub(super) fn cases() -> Vec<Case> {
    vec![Case {
        name: "question_event_outbox",
        covers: &[
            OpKind::CreateTask,
            OpKind::UpdateTask,
            OpKind::AppendEvents,
            OpKind::ImportBatch,
        ],
        run: outbox,
    }]
}

fn outbox(make: MakeFixture<'_>) -> Result<()> {
    let fx = make();
    let question = row(
        TaskPart::AgentRecord,
        "000001/questions/000001",
        false,
        true,
        json!({
            "version":1,"key":"T-001/R1/Q1","revision":1,"kind":"clarification",
            "asked":{"request_id":"ask","payload_sha256":"00","session_id":"session","generation":1,"at":epoch()},
            "blocking":true,"text":"question","state":"escalated",
            "wake":{"route":"none","key":"wake","state":"pending"},
            "triage":[{"action":"escalate","revision":1,"by":"pm","at":epoch()}]
        }),
    );
    let num = match apply(
        &fx,
        Operation::CreateTask(create_op(&fx, "questions", vec![question.clone()])),
    )?
    .outcome
    {
        Outcome::Created { num, .. } => num,
        other => anyhow::bail!("create: {other:?}"),
    };
    let task = head(&fx, num)?.header.id;
    let area = crate::backend::question_events::area(&task);
    let records = || -> Result<Vec<StoredRecord>> {
        Ok(fx
            .backend
            .view()?
            .records(
                &RecordQuery {
                    area: Some(area.clone()),
                    key: None,
                },
                PageReq::first(10),
            )?
            .items)
    };
    ensure!(
        records()?.len() == 2,
        "task commit must owe both transitions"
    );
    let events: Vec<_> = crate::backend::question_events::transitions(&question)?
        .into_iter()
        .map(|(kind, detail)| event(&fx, Some(&task), &kind, detail))
        .collect();
    // Imported identities can precede (or lack) a task association; replay must still ack work.
    let mut unscoped = events[0].clone();
    unscoped.task = None;
    apply(
        &fx,
        Operation::AppendEvents(AppendEvents {
            events: vec![unscoped],
            reservations: vec![],
        }),
    )?;
    fx.faults.arm(1);
    let failed = fx.backend.apply(Operation::AppendEvents(AppendEvents {
        events: events.clone(),
        reservations: vec![],
    }));
    fx.faults.disarm();
    ensure!(failed.is_err(), "injected crash must fail");
    ensure!(
        records()?.len() == 2,
        "failed append cannot acknowledge work"
    );
    apply(
        &fx,
        Operation::AppendEvents(AppendEvents {
            events: events.clone(),
            reservations: vec![],
        }),
    )?;
    ensure!(records()?.is_empty(), "event commit acknowledges work");
    apply(
        &fx,
        Operation::ImportBatch(ImportBatch {
            blobs: vec![],
            tasks: vec![],
            records: vec![],
            events: events
                .iter()
                .cloned()
                .map(|event| ImportedEvent {
                    event,
                    malformed: false,
                })
                .collect(),
        }),
    )?;
    apply(
        &fx,
        Operation::AppendEvents(AppendEvents {
            events,
            reservations: vec![],
        }),
    )?;
    let mut update = change(&fx, num)?;
    update.rows = vec![RowWrite::Put(question)];
    apply(
        &fx,
        Operation::UpdateTask(UpdateTask {
            change: update,
            form: UpdateForm::Full,
        }),
    )?;
    ensure!(
        records()?.is_empty(),
        "unchanged records must not recreate acknowledged work"
    );
    let count = fx
        .backend
        .view()?
        .events(
            &EventQuery {
                kinds: vec!["question_opened".into(), "question_escalated".into()],
                ..EventQuery::default()
            },
            PageReq::first(10),
        )?
        .items
        .len();
    ensure!(
        count == 2,
        "append/import must dedup (kind,key,revision,at)"
    );
    Ok(())
}
