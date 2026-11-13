//! Conformance cases for atomicity, revisions, claims, the change feed and task operations.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail, ensure};
use serde_json::json;

use super::samples::*;
use super::*;

const TASK_WRITING: [OpKind; 12] = [
    OpKind::UpdateTask,
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
    OpKind::Authorization,
];

pub(super) fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "atomicity_under_injected_failures",
            covers: &OpKind::ALL,
            run: atomicity,
        },
        Case {
            name: "stale_preconditions_conflict",
            covers: &[
                OpKind::UpdateTask,
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
                OpKind::Authorization,
                OpKind::PutRecords,
                OpKind::Lifecycle,
                OpKind::Claim,
                OpKind::ScratchClose,
            ],
            run: stale_preconditions,
        },
        Case {
            name: "claim_race_has_one_winner",
            covers: &[OpKind::Claim],
            run: claim_race,
        },
        Case {
            name: "change_feed_one_entry_per_commit",
            covers: &[
                OpKind::CreateTask,
                OpKind::UpdateTask,
                OpKind::AppendEvents,
                OpKind::Claim,
                OpKind::PutRecords,
                OpKind::ObserveCondition,
                OpKind::ClearCondition,
            ],
            run: change_feed,
        },
        Case {
            name: "change_feed_gap_after_pruning",
            covers: &[OpKind::AppendEvents],
            run: feed_gap,
        },
        Case {
            name: "head_form_keeps_history",
            covers: &[OpKind::UpdateTask],
            run: head_form,
        },
        Case {
            name: "authorization_transitions",
            covers: &[OpKind::Authorization],
            run: authorization,
        },
    ]
}

/// Every operation, failed after each of its primitive writes in turn, leaves nothing visible
/// and no change entry; the same operation then applies.
fn atomicity(make: MakeFixture<'_>) -> Result<()> {
    for kind in OpKind::ALL {
        let fx = make();
        let op = sample(kind, &fx)?;
        ensure!(op.kind() == kind, "sample for {kind:?} is {:?}", op.kind());
        fx.faults.reset_count();
        fx.backend
            .apply(op)
            .map_err(|e| anyhow::anyhow!("{kind:?}: {e}"))?;
        let writes = fx.faults.writes();
        ensure!(writes > 0, "{kind:?} reported no primitive writes");
        for nth in 1..=writes {
            let fx = make();
            let op = sample(kind, &fx)?;
            let before = snapshot(fx.backend.as_ref())?;
            fx.faults.arm(nth);
            let result = fx.backend.apply(op.clone());
            fx.faults.disarm();
            ensure!(
                matches!(result, Err(ApplyError::Io(_))),
                "{kind:?} failing at write {nth}/{writes}: got {result:?}"
            );
            ensure!(
                snapshot(fx.backend.as_ref())? == before,
                "{kind:?} failing at write {nth}/{writes} left partial state"
            );
            fx.backend
                .apply(op)
                .map_err(|e| anyhow::anyhow!("{kind:?} after recovery: {e}"))?;
            ensure_integrity(&fx)?;
        }
    }
    Ok(())
}

/// A second handle moves the state on, by an unrelated edit or by committing the same
/// operation (which changes the operation's own state); the write computed from the old state
/// conflicts either way.
fn stale_preconditions(make: MakeFixture<'_>) -> Result<()> {
    let mut covered = BTreeSet::new();
    for kind in OpKind::ALL {
        let fx = make();
        let op = sample(kind, &fx)?;
        let Some(stale) = op.task_change() else {
            continue;
        };
        let view = fx.second.view()?;
        let current = view
            .head(stale.num)?
            .ok_or_else(|| anyhow::anyhow!("no head"))?;
        drop(view);
        let mut bump = change(&fx, stale.num)?;
        bump.fields.title = format!("{} (edited)", current.header.fields.title);
        fx.second
            .apply(Operation::UpdateTask(UpdateTask {
                change: bump,
                form: UpdateForm::Head,
            }))
            .map_err(|e| anyhow::anyhow!("bump: {e}"))?;
        refused(&fx, op, &format!("stale {kind:?}"), conflict)?;

        let fx = make();
        let op = sample(kind, &fx)?;
        fx.second
            .apply(op.clone())
            .map_err(|e| anyhow::anyhow!("competing {kind:?}: {e}"))?;
        refused(
            &fx,
            op,
            &format!("{kind:?} replayed after a competing write of its own state"),
            conflict,
        )?;
        covered.insert(kind);
    }
    ensure!(
        covered == BTreeSet::from(TASK_WRITING),
        "task-writing operations covered: {covered:?}"
    );

    let fx = make();
    let now = fx.clock.now();
    apply(
        &fx,
        put_records(
            vec![record("pm", "policy", None, Some(json!({})), now)],
            Vec::new(),
        ),
    )?;
    fx.second
        .apply(put_records(
            vec![record("pm", "policy", Some(1), Some(json!({"v": 2})), now)],
            Vec::new(),
        ))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    refused(
        &fx,
        put_records(vec![record("pm", "policy", Some(1), None, now)], Vec::new()),
        "stale record",
        conflict,
    )?;
    refused(
        &fx,
        put_records(
            vec![record("pm", "policy", None, Some(json!({})), now)],
            Vec::new(),
        ),
        "record exists",
        conflict,
    )?;

    apply(
        &fx,
        Operation::Lifecycle(lifecycle(&fx, None, LifecycleOutcome::Deferred)),
    )?;
    refused(
        &fx,
        Operation::Lifecycle(lifecycle(&fx, None, LifecycleOutcome::Retained)),
        "lifecycle exists",
        conflict,
    )?;
    refused(
        &fx,
        Operation::Lifecycle(lifecycle(
            &fx,
            Some(LifecycleOutcome::Retained),
            LifecycleOutcome::Reclaimed,
        )),
        "stale lifecycle",
        conflict,
    )?;

    apply(&fx, claim(&fx, "operating", "pm-1"))?;
    refused(&fx, claim(&fx, "operating", "pm-2"), "held claim", conflict)?;

    apply(&fx, register(&fx, "s1"))?;
    apply(&fx, close(&fx, "s1"))?;
    refused(&fx, close(&fx, "s1"), "closed scratch", conflict)?;
    ensure_integrity(&fx)
}

/// 8 threads over two handles race 1,000 claims; each claim has exactly one winner.
fn claim_race(make: MakeFixture<'_>) -> Result<()> {
    const NAMES: usize = 1000;
    const THREADS: usize = 8;
    let fx = make();
    let handles: [&dyn StoreBackend; 2] = [fx.backend.as_ref(), fx.second.as_ref()];
    let now = fx.clock.now();
    let results: Vec<Result<Vec<usize>>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..THREADS)
            .map(|t| {
                let backend = handles[t % 2];
                scope.spawn(move || -> Result<Vec<usize>> {
                    let mut won = Vec::new();
                    for i in 0..NAMES {
                        let name = (i + t * (NAMES / THREADS)) % NAMES;
                        let op = Operation::Claim(Claim {
                            name: format!("c{name}"),
                            owner: format!("w{t}"),
                            acquired: now,
                            doc: serde_json::Value::Null,
                            reservations: Vec::new(),
                        });
                        match backend.apply(op) {
                            Ok(_) => won.push(name),
                            Err(ApplyError::Conflict(_)) => {}
                            Err(other) => bail!("claim c{name} by w{t}: {other}"),
                        }
                    }
                    Ok(won)
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|w| {
                w.join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("worker panicked")))
            })
            .collect()
    });
    let mut winners: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (t, result) in results.into_iter().enumerate() {
        for name in result? {
            winners.entry(name).or_default().push(t);
        }
    }
    ensure!(
        winners.len() == NAMES,
        "{} of {NAMES} claims were won",
        winners.len()
    );
    if let Some((name, ws)) = winners.iter().find(|(_, ws)| ws.len() != 1) {
        bail!("claim c{name} was won by {ws:?}");
    }
    let view = fx.backend.view()?;
    let rows = all_pages(|page| view.storage_rows(&StorageQuery::Claims, page))?;
    ensure!(rows.len() == NAMES, "{} claim rows", rows.len());
    for row in rows {
        let StorageRow::Claim(claim) = row else {
            bail!("non-claim row")
        };
        let name: usize = claim.name.trim_start_matches('c').parse()?;
        ensure!(
            claim.owner == format!("w{}", winners[&name][0]),
            "claim {} owner {}",
            claim.name,
            claim.owner
        );
    }
    drop(view);
    ensure_integrity(&fx)
}

/// What one commit in `change_feed` must do to the feed.
#[derive(Clone, Copy, Debug)]
enum Expect {
    /// Refused: no entry.
    Refused,
    /// Wrote: exactly one entry, at the returned seq.
    Writes,
    /// Applied with a no-op outcome: no entry, and the returned seq is the current one.
    NoOp,
}

/// Every commit that writes appends exactly one entry; a refused commit or one whose outcome
/// is a no-op (a clear without an open run) appends none.
fn change_feed(make: MakeFixture<'_>) -> Result<()> {
    let fx = make();
    let now = fx.clock.now();
    let mut committed = Vec::new();
    let num = base_task(&fx)?;
    committed.push((snapshot(fx.backend.as_ref())?.seq, OpKind::CreateTask));
    let mut stale = change(&fx, num)?;
    stale.expected_rev += 5;
    let clear = || {
        Operation::ClearCondition(ClearCondition {
            task: "T-001".to_owned(),
            kind: "queued".to_owned(),
            at: now,
            actor: None,
            via: None,
        })
    };
    let ops: Vec<(Operation, Expect)> = vec![
        (
            Operation::UpdateTask(UpdateTask {
                change: stale,
                form: UpdateForm::Head,
            }),
            Expect::Refused,
        ),
        (
            Operation::AppendEvents(AppendEvents {
                events: vec![event(&fx, None, "note", json!({}))],
                reservations: Vec::new(),
            }),
            Expect::Writes,
        ),
        (claim(&fx, "x", "a"), Expect::Writes),
        (claim(&fx, "x", "b"), Expect::Refused),
        (
            put_records(
                vec![record("pm", "k", Some(5), Some(json!({})), now)],
                Vec::new(),
            ),
            Expect::Refused,
        ),
        (
            put_records(
                vec![record("pm", "k", None, Some(json!({})), now)],
                Vec::new(),
            ),
            Expect::Writes,
        ),
        (observe(&fx, "queued", json!({})), Expect::Writes),
        (clear(), Expect::Writes),
        (clear(), Expect::NoOp),
    ];
    for (op, expect) in ops {
        let kind = op.kind();
        let before = snapshot(fx.backend.as_ref())?.seq;
        match (fx.backend.apply(op), expect) {
            (Ok(done), Expect::Writes) => committed.push((done.seq, kind)),
            (Ok(done), Expect::NoOp) => {
                let after = snapshot(fx.backend.as_ref())?.seq;
                ensure!(
                    done.seq == before && after == before,
                    "{kind:?} no-op moved the feed from {before:?} to {after:?}, returned {:?}",
                    done.seq
                );
            }
            (Err(_), Expect::Refused) => {}
            (result, _) => bail!("{kind:?}: expected {expect:?}, got {result:?}"),
        }
    }
    let mut update = change(&fx, num)?;
    update.fields.status = "done".to_owned();
    let mark = snapshot(fx.backend.as_ref())?.seq;
    let done = fx.backend.apply(Operation::UpdateTask(UpdateTask {
        change: update,
        form: UpdateForm::Head,
    }))?;
    committed.push((done.seq, OpKind::UpdateTask));

    let view = fx.backend.view()?;
    let mut entries = Vec::new();
    let mut after = ChangeSeq(0);
    loop {
        let page = view.changes(after, 2)?;
        ensure!(page.entries.len() <= 2, "page larger than its limit");
        after = page.entries.last().map_or(after, |e| e.seq);
        entries.extend(page.entries);
        if !page.more {
            break;
        }
    }
    let seen: Vec<(ChangeSeq, OpKind)> = entries.iter().map(|e| (e.seq, e.op)).collect();
    ensure!(
        seen == committed,
        "feed {seen:?} differs from commits {committed:?}"
    );
    ensure!(
        entries.windows(2).all(|w| w[0].seq < w[1].seq),
        "feed not strictly increasing"
    );
    ensure!(
        view.seq() == committed.last().map_or(ChangeSeq(0), |c| c.0),
        "view seq is not the last commit"
    );
    ensure!(
        entries[0]
            .scopes
            .iter()
            .any(|s| matches!(s, ChangeScope::Task { num: n, .. } if *n == num)),
        "create lacks its task scope"
    );
    let delta = view.board(Some(mark))?;
    ensure!(
        delta.heads.is_empty() && delta.left == vec![num],
        "board delta {delta:?}"
    );
    ensure!(
        view.board(None)?.heads.is_empty(),
        "a done task is still on the open board"
    );
    drop(view);
    ensure_integrity(&fx)
}

fn feed_gap(make: MakeFixture<'_>) -> Result<()> {
    let fx = make();
    for n in 0..6 {
        apply(
            &fx,
            Operation::AppendEvents(AppendEvents {
                events: vec![event(&fx, None, "note", json!({"n": n}))],
                reservations: Vec::new(),
            }),
        )?;
    }
    (fx.prune_changes_through)(ChangeSeq(3))?;
    let view = fx.backend.view()?;
    match view.changes(ChangeSeq(1), 10) {
        Err(FeedError::Gap { oldest }) => ensure!(oldest == ChangeSeq(4), "gap oldest {oldest:?}"),
        other => bail!("changes after a pruned seq: {other:?}"),
    }
    ensure!(
        matches!(view.board(Some(ChangeSeq(2))), Err(FeedError::Gap { .. })),
        "board delta across a gap"
    );
    let page = view.changes(ChangeSeq(3), 10)?;
    let seqs: Vec<u64> = page.entries.iter().map(|e| e.seq.0).collect();
    ensure!(
        seqs == vec![4, 5, 6] && !page.more,
        "entries after the floor: {seqs:?}"
    );
    ensure!(
        view.board(Some(ChangeSeq(3))).is_ok(),
        "board delta from the floor"
    );
    drop(view);
    ensure_integrity(&fx)
}

fn head_form(make: MakeFixture<'_>) -> Result<()> {
    let fx = make();
    let rows = vec![
        row(TaskPart::Run, "R0", false, false, json!({"run": "R0"})),
        row(TaskPart::Run, "R1", true, true, json!({"run": "R1"})),
    ];
    let Outcome::Created { num, id } =
        apply(&fx, Operation::CreateTask(create_op(&fx, "t", rows)))?.outcome
    else {
        bail!("create");
    };
    ensure!(id == "T-001", "first id {id}");
    let update = |rows: Vec<RowWrite>, form| -> Result<Operation> {
        let mut change = change(&fx, num)?;
        change.rows = rows;
        Ok(Operation::UpdateTask(UpdateTask { change, form }))
    };
    let edit_history = vec![RowWrite::Put(row(
        TaskPart::Run,
        "R0",
        false,
        false,
        json!({"edited": true}),
    ))];
    let delete = vec![RowWrite::Delete {
        part: TaskPart::Run,
        key: "R0".to_owned(),
    }];
    refused(
        &fx,
        update(edit_history.clone(), UpdateForm::Head)?,
        "head form edits history",
        precondition,
    )?;
    refused(
        &fx,
        update(delete.clone(), UpdateForm::Head)?,
        "head form deletes",
        precondition,
    )?;
    let demote = vec![
        RowWrite::Put(row(TaskPart::Run, "R1", false, false, json!({"run": "R1"}))),
        RowWrite::Put(row(TaskPart::Run, "R2", true, true, json!({"run": "R2"}))),
    ];
    let done = apply(&fx, update(demote, UpdateForm::Head)?)?;
    ensure!(
        done.outcome == Outcome::Task { num, rev: 2 },
        "head write outcome {:?}",
        done.outcome
    );
    apply(&fx, update(edit_history, UpdateForm::Full)?)?;
    apply(&fx, update(delete, UpdateForm::Full)?)?;
    let task = fx
        .backend
        .view()?
        .task(num, &TaskParts::Only(vec![TaskPart::Run]))?
        .ok_or_else(|| anyhow::anyhow!("task"))?;
    let keys: Vec<(&str, bool)> = task
        .rows
        .iter()
        .map(|r| (r.key.as_str(), r.current))
        .collect();
    ensure!(
        keys == vec![("R1", false), ("R2", true)] && task.head.header.rev == 4,
        "rows {keys:?}"
    );
    refused(
        &fx,
        update(
            vec![RowWrite::Delete {
                part: TaskPart::Run,
                key: "R9".to_owned(),
            }],
            UpdateForm::Full,
        )?,
        "delete missing",
        precondition,
    )?;
    ensure_integrity(&fx)
}

fn authorization(make: MakeFixture<'_>) -> Result<()> {
    let fx = make();
    let num = base_task(&fx)?;
    let op = |from, to, change: Option<TaskChange>| {
        Operation::Authorization(Authorization {
            id: "a1".to_owned(),
            task: "T-001".to_owned(),
            from,
            to,
            doc: json!({}),
            change: change.map(|c| (c, UpdateForm::Head)),
        })
    };
    use Disposition::*;
    refused(
        &fx,
        op(Some(Prepared), Submitting, None),
        "missing authorization",
        conflict,
    )?;
    refused(
        &fx,
        op(None, Submitting, None),
        "skip Prepared",
        precondition,
    )?;
    apply(&fx, op(None, Prepared, None))?;
    refused(&fx, op(None, Prepared, None), "prepare twice", conflict)?;
    refused(
        &fx,
        op(Some(Prepared), Refused, Some(change(&fx, num)?)),
        "refusal with a task change",
        precondition,
    )?;
    let mut submit = change(&fx, num)?;
    submit.events = vec![event(&fx, Some("T-001"), "submitted", json!({}))];
    let done = apply(&fx, op(Some(Prepared), Submitting, Some(submit)))?;
    ensure!(
        done.outcome == Outcome::Task { num, rev: 2 } && done.events.len() == 1,
        "submit {done:?}"
    );
    refused(
        &fx,
        op(Some(Submitting), Prepared, None),
        "backwards",
        precondition,
    )?;
    apply(&fx, op(Some(Submitting), Uncertain, None))?;
    let view = fx.backend.view()?;
    let rows = all_pages(|page| {
        view.storage_rows(
            &StorageQuery::Authorizations {
                task: Some("T-001".to_owned()),
            },
            page,
        )
    })?;
    ensure!(
        matches!(rows.as_slice(), [StorageRow::Authorization(a)] if a.disposition == Uncertain),
        "rows {rows:?}"
    );
    drop(view);
    ensure_integrity(&fx)
}
