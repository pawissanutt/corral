//! Conformance case for the preconditions of every task-writing operation.

use anyhow::{Result, bail, ensure};
use serde_json::json;

use super::samples::*;
use super::*;

pub(super) fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "review_completion",
            covers: &[OpKind::Verdict, OpKind::PlanVerdict],
            run: review_completion,
        },
        Case {
            name: "task_operation_preconditions",
            covers: &[
                OpKind::CreateTask,
                OpKind::Comment,
                OpKind::Done,
                OpKind::Verdict,
                OpKind::PlanSubmit,
                OpKind::PlanVerdict,
                OpKind::GateStart,
                OpKind::GateLaunched,
                OpKind::GateResult,
                OpKind::PromoteArtifacts,
                OpKind::PurgeArtifact,
                OpKind::ImportBatch,
            ],
            run: task_operations,
        },
    ]
}

fn task_operations(make: MakeFixture<'_>) -> Result<()> {
    let fx = make();
    let num = base_task(&fx)?;
    let mut bad = create_op(&fx, "x", Vec::new());
    bad.id_prefix = "t".to_owned();
    refused(
        &fx,
        Operation::CreateTask(bad),
        "lowercase prefix",
        precondition,
    )?;

    let mut silent = change(&fx, num)?;
    refused(
        &fx,
        Operation::Comment(Comment {
            change: silent.clone(),
            comment: json!({}),
        }),
        "comment without event",
        precondition,
    )?;
    silent.events = vec![event(&fx, Some("T-001"), "commented", json!({}))];
    for _ in 0..2 {
        let mut c = silent.clone();
        c.expected_rev = head(&fx, num)?.header.rev;
        apply(
            &fx,
            Operation::Comment(Comment {
                change: c,
                comment: json!({"t": 1}),
            }),
        )?;
    }
    let task = fx
        .backend
        .view()?
        .task(num, &TaskParts::Only(vec![TaskPart::Comment]))?
        .ok_or_else(|| anyhow::anyhow!("task"))?;
    let keys: Vec<String> = task.rows.iter().map(|r| r.key.clone()).collect();
    ensure!(
        keys == vec![artifact_key(1), artifact_key(2)],
        "comment keys {keys:?}"
    );

    let Operation::Done(mut no_event) = done(&fx, num)? else {
        bail!("done sample")
    };
    no_event.change.events.clear();
    refused(
        &fx,
        Operation::Done(no_event),
        "done without event",
        precondition,
    )?;
    let Operation::Done(mut wrong_status) = done(&fx, num)? else {
        bail!("done sample")
    };
    wrong_status.change.fields.status = "ready".to_owned();
    refused(
        &fx,
        Operation::Done(wrong_status),
        "done not to review",
        precondition,
    )?;
    apply(&fx, register(&fx, "s1"))?;
    refused(
        &fx,
        verdict(&fx, num, "s1")?,
        "verdict before review",
        precondition,
    )?;
    apply(&fx, done(&fx, num)?)?;
    refused(&fx, done(&fx, num)?, "done on a closed run", precondition)?;
    apply(&fx, verdict(&fx, num, "s1")?)?;
    let mut again = verdict(&fx, num, "s1")?;
    if let Operation::Verdict(v) = &mut again {
        v.close_scratch.clear();
    }
    refused(&fx, again, "verdict twice", precondition)?;

    let mut plan = change(&fx, num)?;
    plan.events = vec![event(&fx, Some("T-001"), "plan_submitted", json!({}))];
    let submit = |key: &str, c: TaskChange| {
        Operation::PlanSubmit(PlanSubmit {
            change: c,
            plan_key: key.to_owned(),
            plan_doc: json!({}),
        })
    };
    refused(
        &fx,
        submit("P1", plan.clone()),
        "plan key reused",
        precondition,
    )?;
    apply(&fx, submit("P2", plan))?;
    let plans = fx
        .backend
        .view()?
        .task(num, &TaskParts::Only(vec![TaskPart::Plan]))?
        .ok_or_else(|| anyhow::anyhow!("task"))?;
    let current: Vec<(&str, bool)> = plans
        .rows
        .iter()
        .map(|r| (r.key.as_str(), r.current))
        .collect();
    ensure!(
        current == vec![("P1", false), ("P2", true)],
        "plans {current:?}"
    );
    let mut review = change(&fx, num)?;
    review.events = vec![event(&fx, Some("T-001"), "plan_verdict", json!({}))];
    let pv = |plan: &str, c: TaskChange| {
        Operation::PlanVerdict(PlanVerdict {
            change: c,
            plan_key: plan.to_owned(),
            review_key: "PR1".to_owned(),
            review_doc: json!({}),
            close_scratch: Vec::new(),
        })
    };
    refused(
        &fx,
        pv("P1", review.clone()),
        "verdict on a demoted plan",
        precondition,
    )?;
    apply(&fx, pv("P2", review))?;

    let (identity, a) = reserve(&fx, b"id", "t1")?;
    let (commands, b) = reserve(&fx, b"cmds", "t2")?;
    let mut start = change(&fx, num)?;
    start.events = vec![event(&fx, Some("T-001"), "gate_started", json!({}))];
    let gate = gate_attempt(&fx, &identity, &commands);
    let mut no_token = start.clone();
    no_token.reservations = vec![a.clone()];
    refused(
        &fx,
        Operation::GateStart(GateStart {
            change: no_token,
            attempt: gate.clone(),
        }),
        "gate ref without reservation",
        precondition,
    )?;
    start.reservations = vec![a, b];
    apply(
        &fx,
        Operation::GateStart(GateStart {
            change: start.clone(),
            attempt: gate.clone(),
        }),
    )?;
    start.expected_rev += 1;
    start.reservations.clear();
    refused(
        &fx,
        Operation::GateStart(GateStart {
            change: start,
            attempt: gate,
        }),
        "gate attempt reused",
        precondition,
    )?;
    let launched = |c: TaskChange| {
        Operation::GateLaunched(GateLaunched {
            change: c,
            run: "R1".to_owned(),
            attempt: "ag-1".to_owned(),
            pane: json!({}),
        })
    };
    apply(&fx, launched(change(&fx, num)?))?;
    refused(
        &fx,
        launched(change(&fx, num)?),
        "launched twice",
        precondition,
    )?;
    let result = |c: TaskChange| {
        Operation::GateResult(GateResult {
            change: c,
            run: "R1".to_owned(),
            attempt: "ag-1".to_owned(),
            result: "fail".to_owned(),
            failed_cmd: Some("cargo test".to_owned()),
            log: None,
            result_doc: None,
            context: None,
            ended: fx.clock.now(),
            close_scratch: Vec::new(),
        })
    };
    apply(&fx, result(change(&fx, num)?))?;
    refused(
        &fx,
        result(change(&fx, num)?),
        "gate ended twice",
        precondition,
    )?;

    let (blob, used) = reserve(&fx, b"artifact", "t3")?;
    let mut promote = change(&fx, num)?;
    refused(
        &fx,
        Operation::PromoteArtifacts(PromoteArtifacts {
            change: promote.clone(),
            artifacts: vec![artifact(&fx, 1, &blob)],
        }),
        "promote without event",
        precondition,
    )?;
    promote.events = vec![event(&fx, Some("T-001"), "artifact", json!({}))];
    promote.reservations = vec![used];
    apply(
        &fx,
        Operation::PromoteArtifacts(PromoteArtifacts {
            change: promote,
            artifacts: vec![artifact(&fx, 1, &blob)],
        }),
    )?;
    let purge = |c: TaskChange, seq| {
        Operation::PurgeArtifact(PurgeArtifact {
            change: c,
            seq,
            purged_at: fx.clock.now(),
        })
    };
    refused(
        &fx,
        purge(change(&fx, num)?, 9),
        "purge missing artifact",
        precondition,
    )?;
    apply(&fx, purge(change(&fx, num)?, 1))?;
    refused(
        &fx,
        purge(change(&fx, num)?, 1),
        "purge twice",
        precondition,
    )?;
    let view = fx.backend.view()?;
    let row = all_pages(|page| {
        view.blobs(
            &BlobQuery {
                hash: Some(blob.clone()),
                state: None,
            },
            page,
        )
    })?;
    ensure!(
        matches!(row.as_slice(), [b] if b.refs == 0 && b.unreferenced_since.is_some()),
        "purged blob {row:?}"
    );
    drop(view);

    let import = sample(OpKind::ImportBatch, &fx)?;
    apply(&fx, import.clone())?;
    refused(&fx, import, "import twice", precondition)?;
    let view = fx.backend.view()?;
    let events = all_pages(|page| view.events(&EventQuery::default(), page))?;
    ensure!(
        events.iter().filter(|e| e.malformed).count() == 1,
        "one malformed legacy event"
    );
    ensure!(
        view.head(TaskNum(50))?
            .is_some_and(|h| h.header.id == "T-050" && h.header.rev == 7),
        "imported head"
    );
    drop(view);
    let created = apply(
        &fx,
        Operation::CreateTask(create_op(&fx, "after import", Vec::new())),
    )?;
    ensure!(
        matches!(&created.outcome, Outcome::Created { id, .. } if id == "T-051"),
        "id after import {:?}",
        created.outcome
    );
    ensure_integrity(&fx)
}

fn review_row(fx: &Fixture, num: TaskNum, part: TaskPart, key: &str) -> Result<HistoryRow> {
    let task = fx.backend.view()?.task(num, &TaskParts::Only(vec![part]))?;
    task.and_then(|task| task.rows.into_iter().find(|row| row.key == key))
        .ok_or_else(|| anyhow::anyhow!("review row {key} missing"))
}

/// Replaces the launched child at `old` with a newer launched attempt at `new`.
fn relaunch(fx: &Fixture, num: TaskNum, part: TaskPart, old: &str, new: &str) -> Result<()> {
    let mut change = change(fx, num)?;
    let mut demoted = review_row(fx, num, part, old)?;
    demoted.current = false;
    demoted.open = false;
    change.rows = vec![
        RowWrite::Put(demoted),
        RowWrite::Put(row(part, new, true, true, json!({"attempt": new}))),
    ];
    apply(
        fx,
        Operation::UpdateTask(UpdateTask {
            change,
            form: UpdateForm::Head,
        }),
    )?;
    Ok(())
}

/// Launch persists an open child; the verdict closes that child in place and keeps its stored
/// fields; a second completion, a replaced attempt and a stale writer are refused.
fn review_completion(make: MakeFixture<'_>) -> Result<()> {
    let fx = make();
    let num = base_task(&fx)?;
    apply(&fx, done(&fx, num)?)?;
    apply(&fx, register(&fx, "s1"))?;
    launch_review(&fx, num, TaskPart::ReviewRun, "R1/1")?;
    let first = verdict(&fx, num, "s1")?;
    apply(&fx, first.clone())?;
    let closed = review_row(&fx, num, TaskPart::ReviewRun, "R1/1")?;
    ensure!(closed.current && !closed.open, "completed child {closed:?}");
    ensure!(
        closed.doc["verdict"] == "approve"
            && closed.doc["inbox"]["receipt"] == "r-1"
            && closed.doc["usage"]["tokens"] == 10,
        "completion lost launch data: {}",
        closed.doc
    );
    refused(&fx, first, "stale verdict", conflict)?;
    let mut again = verdict(&fx, num, "s1")?;
    if let Operation::Verdict(v) = &mut again {
        v.close_scratch.clear();
    }
    refused(&fx, again, "second completion", precondition)?;

    let fx = make();
    let num = base_task(&fx)?;
    apply(&fx, done(&fx, num)?)?;
    launch_review(&fx, num, TaskPart::ReviewRun, "R1/1")?;
    relaunch(&fx, num, TaskPart::ReviewRun, "R1/1", "R1/2")?;
    apply(&fx, register(&fx, "s1"))?;
    refused(
        &fx,
        verdict(&fx, num, "s1")?,
        "verdict on a replaced attempt",
        precondition,
    )?;
    let Operation::Verdict(mut newer) = verdict(&fx, num, "s1")? else {
        bail!("verdict sample")
    };
    newer.review_key = "R1/2".to_owned();
    apply(&fx, Operation::Verdict(newer))?;
    ensure!(
        review_row(&fx, num, TaskPart::ReviewRun, "R1/2")?.doc["attempt"] == "R1/2",
        "newer attempt kept its fields"
    );
    let Operation::Verdict(mut manual) = verdict(&fx, num, "s1")? else {
        bail!("verdict sample")
    };
    manual.review_key = "R1/owner".to_owned();
    manual.close_scratch.clear();
    apply(&fx, Operation::Verdict(manual))?;
    ensure!(
        !review_row(&fx, num, TaskPart::ReviewRun, "R1/owner")?.open,
        "owner verdict recorded closed"
    );

    let fx = make();
    let num = base_task(&fx)?;
    launch_review(&fx, num, TaskPart::PlanReview, "P1/PR1")?;
    let plan_verdict = |review: &str| -> Result<Operation> {
        let mut change = change(&fx, num)?;
        change.events = vec![event(&fx, Some("T-001"), "plan_verdict", json!({}))];
        Ok(Operation::PlanVerdict(PlanVerdict {
            change,
            plan_key: "P1".to_owned(),
            review_key: review.to_owned(),
            review_doc: json!({"verdict": "changes"}),
            close_scratch: Vec::new(),
        }))
    };
    let first = plan_verdict("PR1")?;
    apply(&fx, first.clone())?;
    let closed = review_row(&fx, num, TaskPart::PlanReview, "P1/PR1")?;
    ensure!(
        !closed.open
            && closed.doc["verdict"] == "changes"
            && closed.doc["inbox"]["receipt"] == "r-1",
        "plan review {closed:?}"
    );
    refused(&fx, first, "stale plan verdict", conflict)?;
    refused(
        &fx,
        plan_verdict("PR1")?,
        "second plan completion",
        precondition,
    )?;
    launch_review(&fx, num, TaskPart::PlanReview, "P1/PR2")?;
    relaunch(&fx, num, TaskPart::PlanReview, "P1/PR2", "P1/PR3")?;
    refused(
        &fx,
        plan_verdict("PR2")?,
        "plan verdict on a replaced attempt",
        precondition,
    )?;
    apply(&fx, plan_verdict("PR3")?)?;
    ensure_integrity(&fx)
}
