//! Conformance cases for read-view isolation and expiry (plan sections 2 and 5).

use anyhow::{Result, bail, ensure};
use serde_json::json;

use super::samples::*;
use super::*;

pub(super) fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "view_isolation",
            covers: &[
                OpKind::UpdateTask,
                OpKind::PutRecords,
                OpKind::ObserveCondition,
                OpKind::AppendEvents,
            ],
            run: view_isolation,
        },
        Case {
            name: "view_expiry",
            covers: &[],
            run: view_expiry,
        },
    ]
}

/// The acceptance scenario of plan section 5: a view opened before six commits by a second
/// handle reads the pre-commit state through every method; a later view sees all six.
fn view_isolation(make: MakeFixture<'_>) -> Result<()> {
    let fx = make();
    let now = fx.clock.now();
    let task = base_task(&fx)?;
    let mut dep = create_op(&fx, "dependency", Vec::new());
    dep.fields.edges = Vec::new();
    let Outcome::Created {
        num: dependency,
        id: dep_id,
    } = apply(&fx, Operation::CreateTask(dep))?.outcome
    else {
        bail!("create")
    };
    let mut edge = change(&fx, task)?;
    edge.fields.edges = vec![TaskEdge {
        kind: "depends_on".to_owned(),
        other: dep_id.clone(),
    }];
    apply(
        &fx,
        Operation::UpdateTask(UpdateTask {
            change: edge,
            form: UpdateForm::Head,
        }),
    )?;
    let Outcome::Created { num: counted, .. } = apply(
        &fx,
        Operation::CreateTask(create_op(&fx, "counted", Vec::new())),
    )?
    .outcome
    else {
        bail!("create")
    };
    apply(&fx, observe(&fx, "queued", json!({"n": 1})))?;
    let (live, used) = reserve(&fx, b"live", "tok")?;
    apply(
        &fx,
        put_records(
            vec![record(
                "keep",
                "live",
                None,
                Some(json!({"b": live.to_ref()})),
                now,
            )],
            vec![used],
        ),
    )?;

    let reference = snapshot(fx.backend.as_ref())?;
    let view = fx.backend.view()?;
    let seq = view.seq();
    let open = all_pages(|page| view.heads(&HeadQuery::Open { project: None }, page))?;
    ensure!(open.len() == 3, "three open heads");

    let commit = |op: Operation| fx.second.apply(op).map_err(|e| anyhow::anyhow!("{e}"));
    let mut head = change(&fx, task)?;
    head.fields.title = "moved on".to_owned();
    commit(Operation::UpdateTask(UpdateTask {
        change: head,
        form: UpdateForm::Head,
    }))?;
    let mut dep_change = change(&fx, dependency)?;
    dep_change.doc = json!({"body": "dependency changed"});
    commit(Operation::UpdateTask(UpdateTask {
        change: dep_change,
        form: UpdateForm::Head,
    }))?;
    let mut finished = change(&fx, counted)?;
    finished.fields.status = "done".to_owned();
    commit(Operation::UpdateTask(UpdateTask {
        change: finished,
        form: UpdateForm::Head,
    }))?;
    commit(put_records(
        vec![record("pm", "claim", None, Some(json!({"by": "pm"})), now)],
        Vec::new(),
    ))?;
    commit(observe(&fx, "queued", json!({"n": 1})))?;
    commit(Operation::AppendEvents(AppendEvents {
        events: vec![event(&fx, None, "note", json!({"b": live.to_ref()}))],
        reservations: Vec::new(),
    }))?;

    ensure!(view.seq() == seq, "the view's seq moved");
    let after = dump(view.as_ref())?;
    ensure!(after == reference, "the open view observed a later commit");
    ensure!(
        view.head(task)?
            .is_some_and(|h| h.header.fields.title == "base"),
        "head through the old view"
    );
    let deps = all_pages(|page| view.heads(&HeadQuery::Ids(vec![dep_id.clone()]), page))?;
    ensure!(
        deps.len() == 1 && deps[0].doc == json!({"body": "dependency"}),
        "dependency through the old view"
    );
    drop(view);

    let fresh = fx.backend.view()?;
    ensure!(
        fresh.seq() == ChangeSeq(seq.0 + 6),
        "six commits after the view"
    );
    ensure!(
        fresh
            .head(task)?
            .is_some_and(|h| h.header.fields.title == "moved on"),
        "task head change"
    );
    let deps = all_pages(|page| fresh.heads(&HeadQuery::Ids(vec![dep_id.clone()]), page))?;
    ensure!(
        deps.len() == 1 && deps[0].doc == json!({"body": "dependency changed"}),
        "dependency head change"
    );
    let counts = fresh
        .aggregates(&[AggregateName::StatusCounts])?
        .status_counts;
    ensure!(
        counts.iter().any(|c| c.status == "done" && c.n == 1),
        "status-count change {counts:?}"
    );
    ensure!(
        !all_pages(|page| fresh.records(
            &RecordQuery {
                area: Some("pm".to_owned()),
                key: None
            },
            page
        ))?
        .is_empty(),
        "record write"
    );
    let runs = all_pages(|page| {
        fresh.conditions(
            &ConditionQuery {
                open_only: true,
                ..ConditionQuery::default()
            },
            page,
        )
    })?;
    ensure!(
        runs.len() == 1 && runs[0].count == 2,
        "condition-run update {runs:?}"
    );
    let rows = all_pages(|page| {
        fresh.blobs(
            &BlobQuery {
                hash: Some(live.clone()),
                state: None,
            },
            page,
        )
    })?;
    ensure!(
        rows.len() == 1 && rows[0].refs == 2,
        "blob reference {rows:?}"
    );
    drop(fresh);
    ensure_integrity(&fx)
}

fn view_expiry(make: MakeFixture<'_>) -> Result<()> {
    let fx = make();
    base_task(&fx)?;
    let view = fx.backend.view()?;
    let max = fx.config.view_max_age;
    fx.clock.advance(max);
    ensure!(
        view.head(TaskNum(1))?.is_some(),
        "a view exactly view_max_age old still reads"
    );
    fx.clock.advance(std::time::Duration::from_secs(1));
    let expected = max + std::time::Duration::from_secs(1);
    let expired = |result: Result<(), ViewError>, what: &str| match result {
        Err(ViewError::Expired { age }) if age == expected => Ok(()),
        other => bail!("{what}: expected Expired({expected:?}), got {other:?}"),
    };
    let feed_expired = |result: Result<(), FeedError>, what: &str| match result {
        Err(FeedError::View(ViewError::Expired { age })) if age == expected => Ok(()),
        other => bail!("{what}: expected Expired({expected:?}), got {other:?}"),
    };
    let page = || PageReq::first(10);
    feed_expired(view.board(None).map(drop), "board")?;
    feed_expired(view.changes(ChangeSeq(0), 10).map(drop), "changes")?;
    expired(view.head(TaskNum(1)).map(drop), "head")?;
    expired(
        view.heads(&HeadQuery::Open { project: None }, page())
            .map(drop),
        "heads",
    )?;
    expired(view.task(TaskNum(1), &TaskParts::All).map(drop), "task")?;
    expired(
        view.tasks(
            &TaskQuery {
                statuses: Vec::new(),
                project: None,
                order: TaskOrder::IdAsc,
            },
            page(),
        )
        .map(drop),
        "tasks",
    )?;
    expired(
        view.events(&EventQuery::default(), page()).map(drop),
        "events",
    )?;
    expired(
        view.conditions(&ConditionQuery::default(), page())
            .map(drop),
        "conditions",
    )?;
    expired(
        view.records(&RecordQuery::default(), page()).map(drop),
        "records",
    )?;
    expired(view.aggregates(&AggregateName::ALL).map(drop), "aggregates")?;
    expired(view.blobs(&BlobQuery::default(), page()).map(drop), "blobs")?;
    expired(
        view.storage_rows(&StorageQuery::Claims, page()).map(drop),
        "storage rows",
    )?;
    drop(view);
    let fresh = fx.backend.view()?;
    ensure!(
        fresh.head(TaskNum(1))?.is_some(),
        "a new view reads normally"
    );
    drop(fresh);
    ensure_integrity(&fx)
}
