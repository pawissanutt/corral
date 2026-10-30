//! Conformance cases for row queries, typed header columns and the head-form row rules.

use anyhow::{Result, ensure};
use serde_json::json;

use super::samples::*;
use super::*;

pub(super) fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "row_queries",
            covers: &[OpKind::CreateTask, OpKind::UpdateTask],
            run: row_queries,
        },
        Case {
            name: "head_form_rows",
            covers: &[OpKind::UpdateTask],
            run: head_form_rows,
        },
        Case {
            name: "head_documents_and_columns",
            covers: &[OpKind::CreateTask, OpKind::UpdateTask],
            run: head_documents,
        },
    ]
}

fn indexed(part: TaskPart, key: &str, current: bool, open: bool, tokens: &[&str]) -> HistoryRow {
    HistoryRow {
        index: tokens.iter().map(|t| (*t).to_owned()).collect(),
        ..row(part, key, current, open, json!({"key": key}))
    }
}

fn keys(page: &Page<HistoryRow>) -> Vec<String> {
    page.items.iter().map(|row| row.key.clone()).collect()
}

fn query(fx: &Fixture, num: TaskNum, q: &RowQuery, limit: u32) -> Result<Page<HistoryRow>> {
    Ok(fx.backend.view()?.rows(num, q, PageReq::first(limit))?)
}

fn created(fx: &Fixture, rows: Vec<HistoryRow>) -> Result<TaskNum> {
    match apply(fx, Operation::CreateTask(create_op(fx, "rows", rows)))?.outcome {
        Outcome::Created { num, .. } => Ok(num),
        other => anyhow::bail!("create returned {other:?}"),
    }
}

fn row_queries(make: MakeFixture<'_>) -> Result<()> {
    let fx = make();
    let rows = vec![
        indexed(TaskPart::Run, "000001", false, false, &[]),
        indexed(TaskPart::Run, "000002", true, true, &[]),
        indexed(
            TaskPart::ReviewRun,
            "000001/000001",
            false,
            false,
            &["at:a"],
        ),
        indexed(
            TaskPart::ReviewRun,
            "000002/000001",
            false,
            false,
            &["at:b"],
        ),
        indexed(
            TaskPart::ReviewRun,
            "000002/000002",
            false,
            true,
            &["at:c", "sha:x"],
        ),
        indexed(TaskPart::ReviewRun, "000002/000003", true, true, &["at:c"]),
        indexed(TaskPart::ReviewRun, "0000020/000001", true, false, &[]),
    ];
    let num = created(&fx, rows.clone())?;
    let stored = fx
        .backend
        .view()?
        .task(num, &TaskParts::All)?
        .ok_or_else(|| anyhow::anyhow!("task"))?;
    ensure!(
        stored.rows.iter().map(|r| &r.index).collect::<Vec<_>>()
            == rows.iter().map(|r| &r.index).collect::<Vec<_>>(),
        "index tokens round-trip: {:?}",
        stored.rows
    );
    let part = RowQuery::part(TaskPart::ReviewRun);
    let under = RowQuery::under(TaskPart::ReviewRun, "000002/");
    for (q, expected) in [
        (
            part.clone(),
            vec![
                "000001/000001",
                "000002/000001",
                "000002/000002",
                "000002/000003",
                "0000020/000001",
            ],
        ),
        (
            under.clone(),
            vec!["000002/000001", "000002/000002", "000002/000003"],
        ),
        (under.clone().current(), vec!["000002/000003"]),
        (under.clone().open(), vec!["000002/000002", "000002/000003"]),
        (
            under.clone().with_index("at:c"),
            vec!["000002/000002", "000002/000003"],
        ),
        (part.clone().with_index("sha:x"), vec!["000002/000002"]),
        (
            under.clone().after_key("000002/000001"),
            vec!["000002/000002", "000002/000003"],
        ),
        (
            under.clone().with_index("at:c").after_key("000002/000002"),
            vec!["000002/000003"],
        ),
        (
            under.clone().newest_first().after_key("000002/000002"),
            vec!["000002/000003"],
        ),
        (
            under.clone().newest_first(),
            vec!["000002/000003", "000002/000002", "000002/000001"],
        ),
        (RowQuery::key(TaskPart::Run, "000002"), vec!["000002"]),
        (RowQuery::part(TaskPart::Run).current(), vec!["000002"]),
        (RowQuery::part(TaskPart::Plan), vec![]),
    ] {
        let page = query(&fx, num, &q, 10)?;
        ensure!(keys(&page) == expected, "{q:?} returned {:?}", keys(&page));
        ensure!(page.items.iter().all(|row| q.matches(row)), "{q:?} filter");
    }
    let bounded = under.clone().after_key("000002/000001");
    let bounded_first = query(&fx, num, &bounded, 1)?;
    ensure!(
        keys(&bounded_first) == ["000002/000002"],
        "bounded first page"
    );
    let bounded_next = fx.backend.view()?.rows(
        num,
        &bounded,
        PageReq {
            after: bounded_first.next,
            limit: 1,
        },
    )?;
    ensure!(
        keys(&bounded_next) == ["000002/000003"],
        "bounded opaque-cursor continuation"
    );
    let first = query(&fx, num, &under, 2)?;
    ensure!(
        keys(&first) == ["000002/000001", "000002/000002"],
        "{first:?}"
    );
    let rest = fx.backend.view()?.rows(
        num,
        &under,
        PageReq {
            after: first.next.clone(),
            limit: 2,
        },
    )?;
    ensure!(
        keys(&rest) == ["000002/000003"] && rest.next.is_none(),
        "{rest:?}"
    );
    ensure!(
        query(&fx, TaskNum(num.0 + 7), &part, 5)?.items.is_empty(),
        "unknown task"
    );

    let mut c = change(&fx, num)?;
    c.rows = vec![RowWrite::Put(indexed(
        TaskPart::ReviewRun,
        "000002/000002",
        false,
        false,
        &["at:d"],
    ))];
    apply(
        &fx,
        Operation::UpdateTask(UpdateTask {
            change: c,
            form: UpdateForm::Full,
        }),
    )?;
    ensure!(
        keys(&query(&fx, num, &under.clone().with_index("at:c"), 10)?) == ["000002/000003"],
        "a rewritten row drops its old tokens"
    );
    ensure!(
        keys(&query(&fx, num, &under.clone().with_index("at:d"), 10)?) == ["000002/000002"],
        "a rewritten row gains its new tokens"
    );
    let mut c = change(&fx, num)?;
    c.rows = vec![RowWrite::Delete {
        part: TaskPart::ReviewRun,
        key: "000002/000002".to_owned(),
    }];
    apply(
        &fx,
        Operation::UpdateTask(UpdateTask {
            change: c,
            form: UpdateForm::Full,
        }),
    )?;
    ensure!(
        query(&fx, num, &part.with_index("at:d"), 10)?
            .items
            .is_empty(),
        "a deleted row leaves no token"
    );
    Ok(())
}

fn head_update(fx: &Fixture, num: TaskNum, rows: Vec<RowWrite>) -> Result<Operation> {
    let mut c = change(fx, num)?;
    c.rows = rows;
    Ok(Operation::UpdateTask(UpdateTask {
        change: c,
        form: UpdateForm::Head,
    }))
}

fn head_form_rows(make: MakeFixture<'_>) -> Result<()> {
    let fx = make();
    let old = row(TaskPart::Run, "000001", false, false, json!({"run": 1}));
    let cur = row(TaskPart::Run, "000002", true, true, json!({"run": 2}));
    let num = created(&fx, vec![old.clone(), cur.clone()])?;
    refused(
        &fx,
        head_update(
            &fx,
            num,
            vec![RowWrite::Put(HistoryRow {
                doc: json!({"run": "edited"}),
                ..old.clone()
            })],
        )?,
        "head-form edit of a history row",
        precondition,
    )?;
    refused(
        &fx,
        head_update(
            &fx,
            num,
            vec![RowWrite::Delete {
                part: TaskPart::Run,
                key: old.key.clone(),
            }],
        )?,
        "head-form delete of a history row",
        precondition,
    )?;
    // A demotion and a promotion change flags only.
    apply(
        &fx,
        head_update(
            &fx,
            num,
            vec![
                RowWrite::Put(HistoryRow {
                    current: false,
                    open: false,
                    ..cur.clone()
                }),
                RowWrite::Put(HistoryRow {
                    current: true,
                    ..old.clone()
                }),
            ],
        )?,
    )?;
    let current = query(&fx, num, &RowQuery::part(TaskPart::Run).current(), 5)?;
    ensure!(keys(&current) == ["000001"], "promoted {current:?}");
    apply(
        &fx,
        head_update(
            &fx,
            num,
            vec![RowWrite::Delete {
                part: TaskPart::Run,
                key: old.key.clone(),
            }],
        )?,
    )?;
    let runs = query(&fx, num, &RowQuery::part(TaskPart::Run), 5)?;
    ensure!(keys(&runs) == ["000002"], "after delete {runs:?}");
    Ok(())
}

fn head_documents(make: MakeFixture<'_>) -> Result<()> {
    let fx = make();
    let num = created(&fx, Vec::new())?;
    let mut fields = fields(&fx, "columns", "ready");
    fields.tracker = true;
    fields.queued = true;
    fields.design = false;
    fields.kind = None;
    fields.size = Some("xl".to_owned());
    fields.spec_rev = "rev-2".to_owned();
    fields.dashboard_due = Some(fx.clock.now());
    for doc in [
        json!({"spec": {"title": "x"}, "state": {"a": 1}, "body": "# body\n"}),
        json!({"spec": null, "body": ""}),
        json!({"only": "state"}),
        json!({}),
        json!(["not", "an", "object"]),
        json!("text"),
    ] {
        let mut c = change(&fx, num)?;
        c.fields = fields.clone();
        c.doc = doc.clone();
        apply(
            &fx,
            Operation::UpdateTask(UpdateTask {
                change: c,
                form: UpdateForm::Head,
            }),
        )?;
        let stored = head(&fx, num)?;
        ensure!(
            stored.doc == doc,
            "head document {doc} read as {}",
            stored.doc
        );
        ensure!(
            stored.header.fields == fields,
            "typed columns {:?}",
            stored.header.fields
        );
    }
    Ok(())
}
