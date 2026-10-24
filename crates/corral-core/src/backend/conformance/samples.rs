//! One valid operation of every kind, applied to a store prepared for it.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

use super::Fixture;
use crate::backend::*;
use crate::blob_ref::BlobHash;

pub(super) const HOUR: Duration = Duration::from_secs(3600);

pub(super) fn ago(at: Timestamp, hours: i64) -> Result<Timestamp> {
    Ok(at.checked_sub(SignedDuration::from_hours(hours))?)
}

pub(super) fn fields(fx: &Fixture, title: &str, status: &str) -> TaskFields {
    TaskFields {
        title: title.to_owned(),
        status: status.to_owned(),
        project: Some("corral".to_owned()),
        priority: "P1".to_owned(),
        parent: None,
        updated: fx.clock.now(),
        edges: Vec::new(),
    }
}

pub(super) fn event(fx: &Fixture, task: Option<&str>, kind: &str, detail: Value) -> NewEvent {
    NewEvent {
        ts: fx.clock.now(),
        task: task.map(str::to_owned),
        kind: kind.to_owned(),
        actor: Some("agent".to_owned()),
        via: None,
        run: None,
        detail,
    }
}

pub(super) fn row(part: TaskPart, key: &str, current: bool, open: bool, doc: Value) -> HistoryRow {
    HistoryRow {
        part,
        key: key.to_owned(),
        current,
        open,
        doc,
    }
}

pub(super) fn apply(fx: &Fixture, op: Operation) -> Result<Committed> {
    let kind = op.kind();
    fx.backend
        .apply(op)
        .map_err(|error| anyhow::anyhow!("setup {kind:?}: {error}"))
}

pub(super) fn create_op(fx: &Fixture, title: &str, rows: Vec<HistoryRow>) -> CreateTask {
    CreateTask {
        id_prefix: "T".to_owned(),
        created: fx.clock.now(),
        fields: fields(fx, title, "in_progress"),
        doc: json!({"body": title}),
        summary: json!({"runs": 0}),
        rows,
        events: vec![event(fx, None, "created", json!({}))],
        reservations: Vec::new(),
    }
}

/// Creates a task with an open current run `R1` and a current plan `P1`.
pub(super) fn base_task(fx: &Fixture) -> Result<TaskNum> {
    let rows = vec![
        row(TaskPart::Run, "R1", true, true, json!({"run": "R1"})),
        row(TaskPart::Plan, "P1", true, true, json!({"plan": "P1"})),
    ];
    match apply(fx, Operation::CreateTask(create_op(fx, "base", rows)))?.outcome {
        Outcome::Created { num, .. } => Ok(num),
        other => bail!("create returned {other:?}"),
    }
}

pub(super) fn head(fx: &Fixture, num: TaskNum) -> Result<TaskHeadDoc> {
    fx.backend.view()?.head(num)?.context("task head missing")
}

/// A change that rewrites the head as it is, at its current revision.
pub(super) fn change(fx: &Fixture, num: TaskNum) -> Result<TaskChange> {
    let head = head(fx, num)?;
    Ok(TaskChange {
        num,
        expected_rev: head.header.rev,
        fields: head.header.fields,
        doc: head.doc,
        summary: head.summary,
        rows: Vec::new(),
        events: Vec::new(),
        records: Vec::new(),
        aggregates: Vec::new(),
        reservations: Vec::new(),
    })
}

/// Stages and reserves `bytes`; the blob is `writing` until an operation references it.
pub(super) fn reserve(
    fx: &Fixture,
    bytes: &[u8],
    token: &str,
) -> Result<(BlobHash, ReservationUse)> {
    let staged = fx.blobs.stage(&mut &bytes[..])?;
    let hash = staged.hash.clone();
    let op = BlobOp::Reserve {
        hash: hash.clone(),
        raw_bytes: staged.raw_bytes,
        stored_bytes: staged.stored_bytes,
        codec: staged.codec.clone(),
        token: token.to_owned(),
        owner: "putter".to_owned(),
        ttl: HOUR,
    };
    match apply(fx, Operation::Blob(op))?.outcome {
        Outcome::Reserved(ReserveOutcome::MustWrite) => fx.blobs.put_staged(staged)?,
        Outcome::Reserved(ReserveOutcome::Exists) => {}
        other => bail!("reserve returned {other:?}"),
    }
    let used = ReservationUse {
        hash: hash.clone(),
        token: token.to_owned(),
    };
    Ok((hash, used))
}

pub(super) fn record(
    area: &str,
    key: &str,
    expected: Option<u64>,
    doc: Option<Value>,
    at: Timestamp,
) -> RecordWrite {
    RecordWrite {
        area: area.to_owned(),
        key: key.to_owned(),
        expected,
        doc,
        updated: at,
    }
}

pub(super) fn put_records(
    writes: Vec<RecordWrite>,
    reservations: Vec<ReservationUse>,
) -> Operation {
    Operation::PutRecords(PutRecords {
        writes,
        events: Vec::new(),
        reservations,
    })
}

/// A live blob whose last reference was removed now.
pub(super) fn unreferenced_blob(fx: &Fixture, bytes: &[u8]) -> Result<BlobHash> {
    let (hash, used) = reserve(fx, bytes, "tok-orphan")?;
    let now = fx.clock.now();
    let doc = json!({"log": hash.to_ref()});
    apply(
        fx,
        put_records(
            vec![record("orphans", hash.as_str(), None, Some(doc), now)],
            vec![used],
        ),
    )?;
    apply(
        fx,
        put_records(
            vec![record("orphans", hash.as_str(), Some(1), None, now)],
            Vec::new(),
        ),
    )?;
    Ok(hash)
}

pub(super) fn claim(fx: &Fixture, name: &str, owner: &str) -> Operation {
    Operation::Claim(Claim {
        name: name.to_owned(),
        owner: owner.to_owned(),
        acquired: fx.clock.now(),
        doc: json!({}),
        reservations: Vec::new(),
    })
}

/// Persists an open, current review child at `key`, as review launch does before dispatch.
pub(super) fn launch_review(fx: &Fixture, num: TaskNum, part: TaskPart, key: &str) -> Result<()> {
    let mut launch = change(fx, num)?;
    let doc = json!({"verdict": null, "inbox": {"receipt": "r-1"}, "usage": {"tokens": 10}});
    launch.rows = vec![RowWrite::Put(row(part, key, true, true, doc))];
    apply(
        fx,
        Operation::UpdateTask(UpdateTask {
            change: launch,
            form: UpdateForm::Head,
        }),
    )?;
    Ok(())
}

pub(super) fn register(fx: &Fixture, id: &str) -> Operation {
    Operation::Scratch(ScratchOp::Register {
        id: id.to_owned(),
        kind: ScratchKind::ImplementationReview,
        task: Some("T-001".to_owned()),
        subject: Some("T-001".to_owned()),
        attempt: Some("R1".to_owned()),
        path: format!("review/{id}"),
        at: fx.clock.now(),
    })
}

pub(super) fn close(fx: &Fixture, id: &str) -> Operation {
    Operation::Scratch(ScratchOp::Close {
        id: id.to_owned(),
        at: fx.clock.now(),
    })
}

pub(super) fn observe(fx: &Fixture, kind: &str, detail: Value) -> Operation {
    Operation::ObserveCondition(ObserveCondition {
        task: "T-001".to_owned(),
        kind: kind.to_owned(),
        detail,
        at: fx.clock.now(),
        actor: Some("autopilot".to_owned()),
        via: None,
    })
}

pub(super) fn gate_attempt(fx: &Fixture, identity: &BlobHash, commands: &BlobHash) -> GateAttempt {
    GateAttempt {
        run: "R1".to_owned(),
        attempt: "ag-1".to_owned(),
        scope: "task".to_owned(),
        sha: "abc123".to_owned(),
        tier: Some("quick".to_owned()),
        identity: identity.to_ref(),
        commands: commands.to_ref(),
        context: None,
        launched: None,
        result: None,
        failed_cmd: None,
        log: None,
        result_doc: None,
        started: fx.clock.now(),
        ended: None,
    }
}

fn gate_started(fx: &Fixture, num: TaskNum) -> Result<()> {
    let (identity, a) = reserve(fx, b"identity", "tok-identity")?;
    let (commands, b) = reserve(fx, b"commands", "tok-commands")?;
    let mut change = change(fx, num)?;
    change.events = vec![event(
        fx,
        Some("T-001"),
        "gate_started",
        json!({"attempt": "ag-1"}),
    )];
    change.reservations = vec![a, b];
    let attempt = gate_attempt(fx, &identity, &commands);
    apply(fx, Operation::GateStart(GateStart { change, attempt }))?;
    Ok(())
}

pub(super) fn artifact(fx: &Fixture, seq: u64, blob: &BlobHash) -> ArtifactRow {
    ArtifactRow {
        seq,
        run: "R1".to_owned(),
        kind: "log".to_owned(),
        title: "gate log".to_owned(),
        name: "gate.log".to_owned(),
        sha256: blob.to_string(),
        bytes: 3,
        blob: Some(blob.to_ref()),
        tier_override: None,
        pinned: false,
        added: fx.clock.now(),
        purged_at: None,
    }
}

fn promote(fx: &Fixture, num: TaskNum) -> Result<Operation> {
    let (blob, used) = reserve(fx, b"log", "tok-artifact")?;
    let mut change = change(fx, num)?;
    change.events = vec![event(fx, Some("T-001"), "artifact", json!({"seq": 1}))];
    change.reservations = vec![used];
    Ok(Operation::PromoteArtifacts(PromoteArtifacts {
        change,
        artifacts: vec![artifact(fx, 1, &blob)],
    }))
}

pub(super) fn done(fx: &Fixture, num: TaskNum) -> Result<Operation> {
    let mut change = change(fx, num)?;
    change.fields.status = "review".to_owned();
    change.events = vec![event(fx, Some("T-001"), "done", json!({"run": "R1"}))];
    Ok(Operation::Done(Done {
        change,
        run_key: "R1".to_owned(),
        run_doc: json!({"run": "R1", "outcome": "done"}),
    }))
}

pub(super) fn verdict(fx: &Fixture, num: TaskNum, scratch: &str) -> Result<Operation> {
    let mut change = change(fx, num)?;
    change.events = vec![event(
        fx,
        Some("T-001"),
        "verdict",
        json!({"verdict": "approve"}),
    )];
    Ok(Operation::Verdict(Verdict {
        change,
        review_key: "R1/1".to_owned(),
        review_doc: json!({"verdict": "approve"}),
        close_scratch: vec![scratch.to_owned()],
    }))
}

fn authorization(
    num_change: Option<(TaskChange, UpdateForm)>,
    from: Option<Disposition>,
    to: Disposition,
) -> Operation {
    Operation::Authorization(Authorization {
        id: "auth-1".to_owned(),
        task: "T-001".to_owned(),
        from,
        to,
        doc: json!({"path": "worker"}),
        change: num_change,
    })
}

/// Prepares `fx` for one operation of `kind` and returns that operation, valid on `fx`.
pub(super) fn sample(kind: OpKind, fx: &Fixture) -> Result<Operation> {
    let now = fx.clock.now();
    Ok(match kind {
        OpKind::CreateTask => Operation::CreateTask(create_op(
            fx,
            "new",
            vec![row(TaskPart::Run, "R1", true, true, json!({"run": "R1"}))],
        )),
        OpKind::UpdateTask => {
            let num = base_task(fx)?;
            let mut change = change(fx, num)?;
            change.fields.title = "renamed".to_owned();
            change.rows = vec![RowWrite::Put(row(
                TaskPart::Run,
                "R1",
                true,
                true,
                json!({"run": "R1", "pane": "p1"}),
            ))];
            change.events = vec![event(fx, Some("T-001"), "edited", json!({}))];
            change.records = vec![record(
                "progress",
                "T-001",
                None,
                Some(json!({"step": 1})),
                now,
            )];
            change.aggregates = vec![
                AggregateWrite::UsageTotal {
                    scope: "task".to_owned(),
                    key: "T-001".to_owned(),
                    doc: json!({"tokens": 5}),
                },
                AggregateWrite::PaneOwner {
                    socket: "/tmp/herdr.sock".to_owned(),
                    delta: 1,
                },
            ];
            Operation::UpdateTask(UpdateTask {
                change,
                form: UpdateForm::Head,
            })
        }
        OpKind::AppendEvents => Operation::AppendEvents(AppendEvents {
            events: vec![
                event(fx, None, "note", json!({"n": 1})),
                event(fx, Some("T-001"), "note", json!({"n": 2})),
            ],
            reservations: Vec::new(),
        }),
        OpKind::Comment => {
            let num = base_task(fx)?;
            let mut change = change(fx, num)?;
            change.events = vec![event(
                fx,
                Some("T-001"),
                "commented",
                json!({"by": "owner"}),
            )];
            Operation::Comment(Comment {
                change,
                comment: json!({"text": "looks good"}),
            })
        }
        OpKind::Done => done(fx, base_task(fx)?)?,
        OpKind::Verdict => {
            let num = base_task(fx)?;
            apply(fx, done(fx, num)?)?;
            apply(fx, register(fx, "s-review"))?;
            launch_review(fx, num, TaskPart::ReviewRun, "R1/1")?;
            verdict(fx, num, "s-review")?
        }
        OpKind::PlanSubmit => {
            let mut change = change(fx, base_task(fx)?)?;
            change.events = vec![event(
                fx,
                Some("T-001"),
                "plan_submitted",
                json!({"plan": "P2"}),
            )];
            Operation::PlanSubmit(PlanSubmit {
                change,
                plan_key: "P2".to_owned(),
                plan_doc: json!({"plan": "P2"}),
            })
        }
        OpKind::PlanVerdict => {
            let num = base_task(fx)?;
            apply(fx, register(fx, "s-plan"))?;
            launch_review(fx, num, TaskPart::PlanReview, "P1/PR1")?;
            let mut change = change(fx, num)?;
            change.events = vec![event(
                fx,
                Some("T-001"),
                "plan_verdict",
                json!({"verdict": "approve"}),
            )];
            Operation::PlanVerdict(PlanVerdict {
                change,
                plan_key: "P1".to_owned(),
                review_key: "PR1".to_owned(),
                review_doc: json!({"verdict": "approve"}),
                close_scratch: vec!["s-plan".to_owned()],
            })
        }
        OpKind::GateStart => {
            let num = base_task(fx)?;
            let (identity, a) = reserve(fx, b"identity", "tok-identity")?;
            let (commands, b) = reserve(fx, b"commands", "tok-commands")?;
            let mut change = change(fx, num)?;
            change.events = vec![event(
                fx,
                Some("T-001"),
                "gate_started",
                json!({"attempt": "ag-1"}),
            )];
            change.reservations = vec![a, b];
            Operation::GateStart(GateStart {
                change,
                attempt: gate_attempt(fx, &identity, &commands),
            })
        }
        OpKind::GateLaunched => {
            let num = base_task(fx)?;
            gate_started(fx, num)?;
            let change = change(fx, num)?;
            Operation::GateLaunched(GateLaunched {
                change,
                run: "R1".to_owned(),
                attempt: "ag-1".to_owned(),
                pane: json!({"pane": "w1:p2"}),
            })
        }
        OpKind::GateResult => {
            let num = base_task(fx)?;
            gate_started(fx, num)?;
            apply(fx, register(fx, "s-gate"))?;
            let (log, used) = reserve(fx, b"gate log", "tok-log")?;
            let mut change = change(fx, num)?;
            change.events = vec![event(fx, Some("T-001"), "gate", json!({"result": "pass"}))];
            change.reservations = vec![used];
            Operation::GateResult(GateResult {
                change,
                run: "R1".to_owned(),
                attempt: "ag-1".to_owned(),
                result: "pass".to_owned(),
                failed_cmd: None,
                log: Some(log.to_ref()),
                result_doc: None,
                context: None,
                ended: now,
                close_scratch: vec!["s-gate".to_owned()],
            })
        }
        OpKind::PromoteArtifacts => promote(fx, base_task(fx)?)?,
        OpKind::PurgeArtifact => {
            let num = base_task(fx)?;
            apply(fx, promote(fx, num)?)?;
            Operation::PurgeArtifact(PurgeArtifact {
                change: change(fx, num)?,
                seq: 1,
                purged_at: now,
            })
        }
        OpKind::Authorization => {
            let num = base_task(fx)?;
            apply(fx, authorization(None, None, Disposition::Prepared))?;
            let mut change = change(fx, num)?;
            change.events = vec![event(
                fx,
                Some("T-001"),
                "submitted",
                json!({"auth": "auth-1"}),
            )];
            change.records = vec![record(
                "inbox",
                "T-001/1",
                None,
                Some(json!({"reserved": true})),
                now,
            )];
            authorization(
                Some((change, UpdateForm::Head)),
                Some(Disposition::Prepared),
                Disposition::Submitting,
            )
        }
        OpKind::Claim => claim(fx, STORAGE_GC_CLAIM, "gc"),
        OpKind::ReleaseClaim => {
            apply(fx, claim(fx, STORAGE_GC_CLAIM, "gc"))?;
            Operation::ReleaseClaim(ReleaseClaim {
                name: STORAGE_GC_CLAIM.to_owned(),
                owner: "gc".to_owned(),
            })
        }
        OpKind::PutRecords => {
            apply(
                fx,
                put_records(
                    vec![record("pm", "policy", None, Some(json!({"v": 1})), now)],
                    Vec::new(),
                ),
            )?;
            Operation::PutRecords(PutRecords {
                writes: vec![
                    record("pm", "policy", Some(1), Some(json!({"v": 2})), now),
                    record("pm", "budget", None, Some(json!({"max": 3})), now),
                ],
                events: vec![event(fx, None, "pm_policy", json!({}))],
                reservations: Vec::new(),
            })
        }
        OpKind::BindSession => Operation::BindSession(BindSession {
            socket: "/tmp/herdr.sock".to_owned(),
            binding: json!({"session": "main"}),
            at: now,
        }),
        OpKind::ObserveCondition => {
            apply(fx, observe(fx, "queued", json!({"n": 1})))?;
            observe(fx, "queued", json!({"n": 2}))
        }
        OpKind::ClearCondition => {
            apply(fx, observe(fx, "queued", json!({"n": 1})))?;
            Operation::ClearCondition(ClearCondition {
                task: "T-001".to_owned(),
                kind: "queued".to_owned(),
                at: now,
                actor: None,
                via: None,
            })
        }
        OpKind::BlobReserve => {
            let staged = fx.blobs.stage(&mut &b"fresh"[..])?;
            Operation::Blob(BlobOp::Reserve {
                hash: staged.hash,
                raw_bytes: staged.raw_bytes,
                stored_bytes: staged.stored_bytes,
                codec: staged.codec,
                token: "tok-fresh".to_owned(),
                owner: "putter".to_owned(),
                ttl: HOUR,
            })
        }
        OpKind::BlobBeginDelete | OpKind::BlobFinishDelete => {
            apply(fx, claim(fx, STORAGE_GC_CLAIM, "gc"))?;
            let hash = unreferenced_blob(fx, b"orphan")?;
            fx.clock.advance(25 * HOUR);
            let horizon = ago(fx.clock.now(), 24)?;
            let begin = Operation::Blob(BlobOp::BeginDelete {
                hash: hash.clone(),
                gc_owner: "gc".to_owned(),
                horizon,
            });
            if kind == OpKind::BlobBeginDelete {
                begin
            } else {
                apply(fx, begin)?;
                fx.blobs.delete(&hash)?;
                Operation::Blob(BlobOp::FinishDelete {
                    hash,
                    gc_owner: "gc".to_owned(),
                })
            }
        }
        OpKind::BlobAdoptStray => {
            apply(fx, claim(fx, STORAGE_GC_CLAIM, "gc"))?;
            Operation::Blob(BlobOp::AdoptStray {
                hash: BlobHash::of(b"stray"),
                gc_owner: "gc".to_owned(),
                stored_bytes: 5,
            })
        }
        OpKind::ScratchRegister => register(fx, "s1"),
        OpKind::ScratchClose => {
            apply(fx, register(fx, "s1"))?;
            close(fx, "s1")
        }
        OpKind::ScratchDefer | OpKind::ScratchBeginReclaim | OpKind::ScratchFinishReclaim => {
            apply(fx, register(fx, "s1"))?;
            apply(fx, close(fx, "s1"))?;
            let begin = Operation::Scratch(ScratchOp::BeginReclaim {
                id: "s1".to_owned(),
                from: ScratchState::Closed,
                owner: "gc".to_owned(),
                at: now,
            });
            match kind {
                OpKind::ScratchDefer => Operation::Scratch(ScratchOp::Defer {
                    id: "s1".to_owned(),
                    from: ScratchState::Closed,
                    reason: "pane alive".to_owned(),
                    next_check_at: now,
                }),
                OpKind::ScratchBeginReclaim => begin,
                _ => {
                    apply(fx, begin)?;
                    Operation::Scratch(ScratchOp::FinishReclaim {
                        id: "s1".to_owned(),
                        owner: "gc".to_owned(),
                        result: ReclaimResult::Reclaimed { bytes: 42 },
                        at: now,
                    })
                }
            }
        }
        OpKind::ScratchWarn => {
            apply(fx, register(fx, "s1"))?;
            Operation::Scratch(ScratchOp::Warn {
                id: "s1".to_owned(),
                evidence_bytes: 1 << 30,
                at: now,
            })
        }
        OpKind::Lifecycle => Operation::Lifecycle(lifecycle(fx, None, LifecycleOutcome::Deferred)),
        OpKind::Lease => Operation::Lease(LeaseOp::Acquire {
            id: "backup".to_owned(),
            holder: "backup-1".to_owned(),
            expires: now.checked_add(SignedDuration::from_hours(1))?,
        }),
        OpKind::ImportBatch => import(fx)?,
    })
}

pub(super) fn lifecycle(
    fx: &Fixture,
    expected: Option<LifecycleOutcome>,
    outcome: LifecycleOutcome,
) -> LifecycleWrite {
    LifecycleWrite {
        task: "T-001".to_owned(),
        generation: "R1".to_owned(),
        target: "worktree".to_owned(),
        expected,
        outcome,
        policy_digest: "p1".to_owned(),
        reason: Some("dirty".to_owned()),
        bytes: None,
        next_check_at: None,
        at: fx.clock.now(),
    }
}

fn import(fx: &Fixture) -> Result<Operation> {
    let now = fx.clock.now();
    let blob = BlobHash::of(b"legacy review");
    let header = TaskHeader {
        num: TaskNum(50),
        id: "T-050".to_owned(),
        rev: 7,
        created: now,
        fields: fields(fx, "legacy", "done"),
    };
    Ok(Operation::ImportBatch(ImportBatch {
        blobs: vec![ImportedBlob {
            hash: blob.clone(),
            raw_bytes: 13,
            stored_bytes: 13,
            codec: "identity".to_owned(),
        }],
        tasks: vec![ImportedTask {
            header,
            doc: json!({"body": "legacy"}),
            summary: json!({"runs": 1}),
            rows: vec![row(
                TaskPart::ReviewRun,
                "R1/1",
                true,
                false,
                json!({"notes": blob.to_ref()}),
            )],
        }],
        events: vec![
            ImportedEvent {
                event: event(fx, Some("T-050"), "created", json!({})),
                malformed: false,
            },
            ImportedEvent {
                event: event(fx, None, "raw", json!({"line": "{not json"})),
                malformed: true,
            },
        ],
        records: vec![StoredRecord {
            area: "lessons".to_owned(),
            key: "l1".to_owned(),
            version: 1,
            doc: json!({}),
            updated: now,
        }],
    }))
}
