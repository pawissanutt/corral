//! Conformance cases for blobs, scratch, lifecycle, leases, conditions and read views.

use anyhow::{Result, bail, ensure};
use serde_json::json;

use super::samples::*;
use super::*;

pub(super) fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "scratch_transitions",
            covers: &[
                OpKind::ScratchRegister,
                OpKind::ScratchClose,
                OpKind::ScratchDefer,
                OpKind::ScratchWarn,
                OpKind::ScratchBeginReclaim,
                OpKind::ScratchFinishReclaim,
                OpKind::Verdict,
            ],
            run: scratch_transitions,
        },
        Case {
            name: "lifecycle_and_leases",
            covers: &[OpKind::Lifecycle, OpKind::Lease],
            run: lifecycle_and_leases,
        },
        Case {
            name: "conditions_claims_and_binding",
            covers: &[
                OpKind::ObserveCondition,
                OpKind::ClearCondition,
                OpKind::BindSession,
                OpKind::Claim,
                OpKind::ReleaseClaim,
            ],
            run: conditions,
        },
    ]
}

fn scratch(fx: &Fixture, id: &str) -> Result<ScratchRow> {
    let view = fx.backend.view()?;
    let rows =
        all_pages(|page| view.storage_rows(&StorageQuery::Scratch { states: Vec::new() }, page))?;
    rows.into_iter()
        .find_map(|row| match row {
            StorageRow::Scratch(row) if row.id == id => Some(row),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("scratch {id} missing"))
}

fn events_of(fx: &Fixture, kind: &str) -> Result<Vec<StoredEvent>> {
    let view = fx.backend.view()?;
    let q = EventQuery {
        kinds: vec![kind.to_owned()],
        ..EventQuery::default()
    };
    Ok(all_pages(|page| view.events(&q, page))?)
}

fn scratch_transitions(make: MakeFixture<'_>) -> Result<()> {
    let fx = make();
    let now = fx.clock.now();
    let op = |op: ScratchOp| Operation::Scratch(op);
    apply(&fx, register(&fx, "s1"))?;
    refused(&fx, register(&fx, "s1"), "duplicate id", precondition)?;
    for path in ["review/s1", "/abs/dir", "a/../b", "a//b", ""] {
        let Operation::Scratch(ScratchOp::Register {
            kind,
            task,
            subject,
            attempt,
            at,
            ..
        }) = register(&fx, "s9")
        else {
            bail!("register sample");
        };
        let bad = ScratchOp::Register {
            id: "s9".to_owned(),
            kind,
            task,
            subject,
            attempt,
            path: path.to_owned(),
            at,
        };
        refused(
            &fx,
            op(bad),
            &format!("register path {path:?}"),
            precondition,
        )?;
    }
    let begin = |from| {
        op(ScratchOp::BeginReclaim {
            id: "s1".to_owned(),
            from,
            owner: "gc".to_owned(),
            at: now,
        })
    };
    refused(
        &fx,
        begin(ScratchState::Closed),
        "reclaim an open dir",
        conflict,
    )?;
    let defer = |from| {
        op(ScratchOp::Defer {
            id: "s1".to_owned(),
            from,
            reason: "pane alive".to_owned(),
            next_check_at: now
                .checked_add(jiff::SignedDuration::from_mins(1))
                .unwrap_or(now),
        })
    };
    refused(
        &fx,
        defer(ScratchState::Open),
        "defer an open dir",
        precondition,
    )?;
    apply(&fx, close(&fx, "s1"))?;
    refused(&fx, close(&fx, "s1"), "close twice", conflict)?;
    apply(&fx, defer(ScratchState::Closed))?;
    ensure!(
        scratch(&fx, "s1")?.state == ScratchState::Deferred,
        "deferred"
    );
    refused(
        &fx,
        begin(ScratchState::Deferred),
        "reclaim before the next check",
        precondition,
    )?;
    fx.clock.advance(std::time::Duration::from_secs(120));
    apply(&fx, begin(ScratchState::Deferred))?;
    let finish = |owner: &str, result| {
        op(ScratchOp::FinishReclaim {
            id: "s1".to_owned(),
            owner: owner.to_owned(),
            result,
            at: now,
        })
    };
    refused(
        &fx,
        finish("intruder", ReclaimResult::Reclaimed { bytes: 1 }),
        "finish by another owner",
        precondition,
    )?;
    apply(&fx, finish("gc", ReclaimResult::Reclaimed { bytes: 7 }))?;
    refused(
        &fx,
        finish("gc", ReclaimResult::Reclaimed { bytes: 7 }),
        "finish twice",
        conflict,
    )?;
    let reclaimed = events_of(&fx, "storage_reclaimed")?;
    ensure!(
        reclaimed.len() == 1 && reclaimed[0].event.detail["scratch"] == "s1",
        "one storage_reclaimed: {reclaimed:?}"
    );
    ensure!(scratch(&fx, "s1")?.reclaimed.is_some(), "reclaimed stamp");

    apply(&fx, register(&fx, "s2"))?;
    let warn = op(ScratchOp::Warn {
        id: "s2".to_owned(),
        evidence_bytes: 9,
        at: now,
    });
    apply(&fx, warn.clone())?;
    refused(&fx, warn, "warn twice", precondition)?;
    ensure!(
        events_of(&fx, "storage_scratch_warning")?.len() == 1,
        "one warning"
    );

    apply(&fx, register(&fx, "s3"))?;
    apply(&fx, close(&fx, "s3"))?;
    apply(
        &fx,
        op(ScratchOp::BeginReclaim {
            id: "s3".to_owned(),
            from: ScratchState::Closed,
            owner: "gc".to_owned(),
            at: now,
        }),
    )?;
    apply(
        &fx,
        op(ScratchOp::FinishReclaim {
            id: "s3".to_owned(),
            owner: "gc".to_owned(),
            result: ReclaimResult::Failed {
                reason: "busy fd".to_owned(),
            },
            at: now,
        }),
    )?;
    ensure!(
        scratch(&fx, "s3")?.state == ScratchState::Failed
            && events_of(&fx, "storage_reclaim_failed")?.len() == 1,
        "failed reclaim"
    );

    let num = base_task(&fx)?;
    apply(&fx, done(&fx, num)?)?;
    apply(&fx, register(&fx, "s4"))?;
    apply(&fx, close(&fx, "s4"))?;
    refused(
        &fx,
        verdict(&fx, num, "s4")?,
        "verdict closing a closed dir",
        conflict,
    )?;
    ensure_integrity(&fx)
}

fn lifecycle_and_leases(make: MakeFixture<'_>) -> Result<()> {
    let fx = make();
    apply(
        &fx,
        Operation::Lifecycle(lifecycle(&fx, None, LifecycleOutcome::Deferred)),
    )?;
    refused(
        &fx,
        Operation::Lifecycle(lifecycle(&fx, None, LifecycleOutcome::Deferred)),
        "insert twice",
        conflict,
    )?;
    apply(
        &fx,
        Operation::Lifecycle(lifecycle(
            &fx,
            Some(LifecycleOutcome::Deferred),
            LifecycleOutcome::Retained,
        )),
    )?;
    let view = fx.backend.view()?;
    let rows = all_pages(|page| {
        view.storage_rows(
            &StorageQuery::Lifecycle {
                task: Some("T-001".to_owned()),
            },
            page,
        )
    })?;
    ensure!(
        matches!(rows.as_slice(), [StorageRow::Lifecycle(r)] if r.outcome == LifecycleOutcome::Retained),
        "{rows:?}"
    );
    drop(view);

    let at = |hours| {
        fx.clock
            .now()
            .checked_add(jiff::SignedDuration::from_hours(hours))
    };
    let lease = |op| Operation::Lease(op);
    apply(
        &fx,
        lease(LeaseOp::Acquire {
            id: "backup".to_owned(),
            holder: "a".to_owned(),
            expires: at(1)?,
        }),
    )?;
    refused(
        &fx,
        lease(LeaseOp::Acquire {
            id: "backup".to_owned(),
            holder: "b".to_owned(),
            expires: at(1)?,
        }),
        "held lease",
        conflict,
    )?;
    refused(
        &fx,
        lease(LeaseOp::Renew {
            id: "backup".to_owned(),
            holder: "b".to_owned(),
            expires: at(2)?,
        }),
        "renew by another",
        precondition,
    )?;
    apply(
        &fx,
        lease(LeaseOp::Renew {
            id: "backup".to_owned(),
            holder: "a".to_owned(),
            expires: at(2)?,
        }),
    )?;
    fx.clock.advance(3 * HOUR);
    apply(
        &fx,
        lease(LeaseOp::Acquire {
            id: "backup".to_owned(),
            holder: "b".to_owned(),
            expires: at(1)?,
        }),
    )?;
    refused(
        &fx,
        lease(LeaseOp::Release {
            id: "backup".to_owned(),
            holder: "a".to_owned(),
        }),
        "release by a former holder",
        precondition,
    )?;
    apply(
        &fx,
        lease(LeaseOp::Release {
            id: "backup".to_owned(),
            holder: "b".to_owned(),
        }),
    )?;
    ensure_integrity(&fx)
}

fn conditions(make: MakeFixture<'_>) -> Result<()> {
    let fx = make();
    refused(
        &fx,
        observe(&fx, "edited", json!({})),
        "non-condition kind",
        precondition,
    )?;
    let first = apply(&fx, observe(&fx, "queued", json!({"a": 1, "b": [1, 2]})))?;
    let Outcome::Condition(ConditionOutcome::Opened {
        opened_seq,
        closed: None,
    }) = first.outcome
    else {
        bail!("first observation {:?}", first.outcome);
    };
    ensure!(first.events == vec![opened_seq], "opening event");
    fx.clock.advance(HOUR);
    let repeat = apply(&fx, observe(&fx, "queued", json!({"b": [1, 2], "a": 1})))?;
    ensure!(repeat.events.is_empty(), "a repeat appends no event");
    ensure!(
        repeat.outcome
            == Outcome::Condition(ConditionOutcome::Repeated {
                opened_seq,
                count: 2
            }),
        "{:?}",
        repeat.outcome
    );
    let changed = apply(&fx, observe(&fx, "queued", json!({"a": 2})))?;
    let Outcome::Condition(ConditionOutcome::Opened {
        opened_seq: second,
        closed: Some(prev),
    }) = changed.outcome
    else {
        bail!("changed observation {:?}", changed.outcome);
    };
    ensure!(
        prev == opened_seq,
        "the changed observation closes the first run"
    );
    let clear = |kind: &str| {
        Operation::ClearCondition(ClearCondition {
            task: "T-001".to_owned(),
            kind: kind.to_owned(),
            at: fx.clock.now(),
            actor: None,
            via: None,
        })
    };
    let cleared = apply(&fx, clear("queued"))?;
    let Outcome::Condition(ConditionOutcome::Cleared { event }) = cleared.outcome else {
        bail!("clear {:?}", cleared.outcome)
    };
    let none = apply(&fx, clear("queued"))?;
    ensure!(
        none.outcome == Outcome::Condition(ConditionOutcome::NotOpen) && none.events.is_empty(),
        "clear without a run"
    );
    let view = fx.backend.view()?;
    let runs = all_pages(|page| {
        view.conditions(
            &ConditionQuery {
                task: Some("T-001".to_owned()),
                ..ConditionQuery::default()
            },
            page,
        )
    })?;
    ensure!(runs.len() == 2, "two durable runs: {runs:?}");
    ensure!(
        runs[0].count == 2
            && runs[0].closed_seq == Some(second)
            && runs[0].close_reason == Some(CloseReason::Changed),
        "first run {:?}",
        runs[0]
    );
    ensure!(
        runs[1].closed_seq == Some(event) && runs[1].close_reason == Some(CloseReason::Cleared),
        "second run {:?}",
        runs[1]
    );
    let open = all_pages(|page| {
        view.conditions(
            &ConditionQuery {
                open_only: true,
                ..ConditionQuery::default()
            },
            page,
        )
    })?;
    ensure!(open.is_empty(), "no open run after clear");
    let ev = all_pages(|page| {
        view.events(
            &EventQuery {
                kinds: vec!["queued_cleared".to_owned()],
                ..EventQuery::default()
            },
            page,
        )
    })?;
    ensure!(
        ev.len() == 1 && ev[0].event.detail["run"] == second.0 && ev[0].event.detail["count"] == 1,
        "cleared event {ev:?}"
    );
    drop(view);

    let bind = |socket: &str| {
        Operation::BindSession(BindSession {
            socket: socket.to_owned(),
            binding: json!({}),
            at: fx.clock.now(),
        })
    };
    apply(&fx, bind("/run/a.sock"))?;
    apply(&fx, bind("/run/a.sock"))?;
    refused(
        &fx,
        bind("/run/b.sock"),
        "rebind to another socket",
        precondition,
    )?;
    apply(&fx, claim(&fx, "operating", "pm"))?;
    let release = |owner: &str| {
        Operation::ReleaseClaim(ReleaseClaim {
            name: "operating".to_owned(),
            owner: owner.to_owned(),
        })
    };
    refused(
        &fx,
        release("other"),
        "release by a non-owner",
        precondition,
    )?;
    apply(&fx, release("pm"))?;
    refused(&fx, release("pm"), "release twice", precondition)?;
    ensure_integrity(&fx)
}
