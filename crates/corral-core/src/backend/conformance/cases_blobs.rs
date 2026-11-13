//! Conformance cases for blob row transitions (plan section 7) and the blob store.

use std::io::Read;

use anyhow::{Result, bail, ensure};
use serde_json::json;

use super::samples::*;
use super::*;

pub(super) fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "documents_keep_their_blobs",
            covers: &[
                OpKind::Claim,
                OpKind::ReleaseClaim,
                OpKind::UpdateTask,
                OpKind::PutRecords,
                OpKind::BlobBeginDelete,
            ],
            run: documents_keep_their_blobs,
        },
        Case {
            name: "blob_transitions",
            covers: &[
                OpKind::BlobReserve,
                OpKind::BlobBeginDelete,
                OpKind::BlobFinishDelete,
                OpKind::BlobAdoptStray,
                OpKind::PutRecords,
                OpKind::AppendEvents,
                OpKind::CreateTask,
            ],
            run: blob_transitions,
        },
        Case {
            name: "blob_store_contract",
            covers: &[OpKind::BlobReserve],
            run: blob_store,
        },
    ]
}

fn blob(fx: &Fixture, hash: &BlobHash) -> Result<Option<BlobRow>> {
    let view = fx.backend.view()?;
    let rows = all_pages(|page| {
        view.blobs(
            &BlobQuery {
                hash: Some(hash.clone()),
                state: None,
            },
            page,
        )
    })?;
    Ok(rows.into_iter().next())
}

fn reserve_op(hash: &BlobHash, token: &str) -> Operation {
    Operation::Blob(BlobOp::Reserve {
        hash: hash.clone(),
        raw_bytes: 1,
        stored_bytes: 1,
        codec: "identity".to_owned(),
        token: token.to_owned(),
        owner: "putter".to_owned(),
        ttl: HOUR,
    })
}

fn reserved(fx: &Fixture, op: Operation) -> Result<ReserveOutcome> {
    match apply(fx, op)?.outcome {
        Outcome::Reserved(outcome) => Ok(outcome),
        other => bail!("reserve returned {other:?}"),
    }
}

fn begin(fx: &Fixture, hash: &BlobHash, owner: &str) -> Result<Operation> {
    let horizon = ago(fx.clock.now(), 24)?;
    Ok(Operation::Blob(BlobOp::BeginDelete {
        hash: hash.clone(),
        gc_owner: owner.to_owned(),
        horizon,
    }))
}

fn finish(hash: &BlobHash) -> Operation {
    Operation::Blob(BlobOp::FinishDelete {
        hash: hash.clone(),
        gc_owner: "gc".to_owned(),
    })
}

fn blob_transitions(make: MakeFixture<'_>) -> Result<()> {
    let fx = make();
    let now = fx.clock.now();
    let h1 = BlobHash::of(b"one");
    let refers = |key: &str, hash: &BlobHash, uses: Vec<ReservationUse>| {
        put_records(
            vec![record(
                "refs",
                key,
                None,
                Some(json!({"deep": [{"r": hash.to_ref()}]})),
                now,
            )],
            uses,
        )
    };
    let use_of = |hash: &BlobHash, token: &str| ReservationUse {
        hash: hash.clone(),
        token: token.to_owned(),
    };
    ensure!(
        reserved(&fx, reserve_op(&h1, "t1"))? == ReserveOutcome::MustWrite,
        "first reserve"
    );
    ensure!(
        reserved(&fx, reserve_op(&h1, "t2"))? == ReserveOutcome::Exists,
        "second reserve"
    );
    let got = blob(&fx, &h1)?.ok_or_else(|| anyhow::anyhow!("row"))?;
    ensure!(
        got.state == BlobState::Writing && got.reservations.len() == 2,
        "reserved row {got:?}"
    );
    refused(
        &fx,
        refers("a", &h1, Vec::new()),
        "ref to a writing blob without its reservation",
        precondition,
    )?;
    refused(
        &fx,
        refers("a", &BlobHash::of(b"nowhere"), Vec::new()),
        "ref to an unknown blob",
        precondition,
    )?;
    apply(&fx, refers("a", &h1, vec![use_of(&h1, "t1")]))?;
    let got = blob(&fx, &h1)?.ok_or_else(|| anyhow::anyhow!("row"))?;
    ensure!(
        got.state == BlobState::Live && got.refs == 1 && got.unreferenced_since.is_none(),
        "referenced {got:?}"
    );
    ensure!(
        got.reservations
            .iter()
            .map(|r| r.token.as_str())
            .collect::<Vec<_>>()
            == vec!["t2"],
        "t1 released"
    );

    apply(&fx, claim(&fx, STORAGE_GC_CLAIM, "gc"))?;
    refused(
        &fx,
        begin(&fx, &h1, "gc")?,
        "delete a referenced blob",
        precondition,
    )?;
    apply(
        &fx,
        put_records(vec![record("refs", "a", Some(1), None, now)], Vec::new()),
    )?;
    ensure!(
        blob(&fx, &h1)?.is_some_and(|r| r.refs == 0 && r.unreferenced_since == Some(now)),
        "unreferenced_since set"
    );
    refused(
        &fx,
        begin(&fx, &h1, "gc")?,
        "putter reserved before BeginDelete (barrier 1)",
        precondition,
    )?;
    fx.clock.advance(2 * HOUR);
    refused(
        &fx,
        begin(&fx, &h1, "gc")?,
        "inside the grace period",
        precondition,
    )?;
    fx.clock.advance(25 * HOUR);
    refused(
        &fx,
        begin(&fx, &h1, "intruder")?,
        "without the storage-gc claim",
        precondition,
    )?;
    apply(&fx, begin(&fx, &h1, "gc")?)?;
    ensure!(
        blob(&fx, &h1)?.is_some_and(|r| r.state == BlobState::Deleting),
        "deleting"
    );
    let before = snapshot(fx.backend.as_ref())?.seq;
    ensure!(
        reserved(&fx, reserve_op(&h1, "t3"))? == ReserveOutcome::Busy,
        "reserve while deleting (barrier 2)"
    );
    ensure!(
        blob(&fx, &h1)?.is_some_and(|r| r.reservations.len() == 1)
            && snapshot(fx.backend.as_ref())?.seq == before,
        "busy reserve wrote nothing and appended no change entry"
    );
    refused(
        &fx,
        refers("b", &h1, Vec::new()),
        "ref to a deleting blob",
        precondition,
    )?;
    refused(&fx, begin(&fx, &h1, "gc")?, "begin twice", conflict)?;
    apply(&fx, finish(&h1))?;
    ensure!(blob(&fx, &h1)?.is_none(), "row removed");
    refused(&fx, finish(&h1), "finish twice", precondition)?;
    ensure!(
        reserved(&fx, reserve_op(&h1, "t4"))? == ReserveOutcome::MustWrite,
        "rewrite after FinishDelete"
    );

    let adopt = |hash: &BlobHash| {
        Operation::Blob(BlobOp::AdoptStray {
            hash: hash.clone(),
            gc_owner: "gc".to_owned(),
            stored_bytes: 3,
        })
    };
    refused(&fx, adopt(&h1), "adopt a blob with a row", precondition)?;
    let stray = BlobHash::of(b"stray");
    apply(&fx, adopt(&stray))?;
    refused(
        &fx,
        finish(&BlobHash::of(b"other")),
        "finish without a row",
        precondition,
    )?;
    apply(&fx, finish(&stray))?;

    let abandoned = BlobHash::of(b"abandoned");
    reserved(&fx, reserve_op(&abandoned, "gone"))?;
    fx.clock.advance(26 * HOUR);
    apply(&fx, begin(&fx, &abandoned, "gc")?)?;

    let (in_record, r) = reserve(&fx, b"record-only", "r")?;
    apply(
        &fx,
        put_records(
            vec![record(
                "t251",
                "receipt",
                None,
                Some(json!({"log": in_record.to_ref()})),
                now,
            )],
            vec![r],
        ),
    )?;
    let (in_event, e) = reserve(&fx, b"event-only", "e")?;
    let mut note = event(
        &fx,
        None,
        "gate",
        json!({"nested": {"log": in_event.to_ref()}}),
    );
    note.run = Some("R1".to_owned());
    apply(
        &fx,
        Operation::AppendEvents(AppendEvents {
            events: vec![note],
            reservations: vec![e],
        }),
    )?;
    let (in_run, u) = reserve(&fx, b"run-only", "u")?;
    let mut create = create_op(
        &fx,
        "refs",
        vec![row(
            TaskPart::Run,
            "R1",
            true,
            true,
            json!({"prompt": in_run.to_ref()}),
        )],
    );
    create.reservations = vec![u];
    apply(&fx, Operation::CreateTask(create))?;
    fx.clock.advance(30 * HOUR);
    for (hash, holder) in [
        (&in_record, "record doc"),
        (&in_event, "event detail"),
        (&in_run, "run row"),
    ] {
        refused(
            &fx,
            begin(&fx, hash, "gc")?,
            &format!("ref held only by a {holder} (barrier 7)"),
            precondition,
        )?;
    }
    ensure_integrity(&fx)
}

fn blob_store(make: MakeFixture<'_>) -> Result<()> {
    let fx = make();
    let bytes = b"evidence bytes".to_vec();
    let staged = fx.blobs.stage(&mut bytes.as_slice())?;
    let hash = staged.hash.clone();
    ensure!(
        hash == BlobHash::of(&bytes) && staged.raw_bytes == bytes.len() as u64,
        "staged {staged:?}"
    );
    fx.blobs.put_staged(staged)?;
    fx.blobs
        .put_staged(fx.blobs.stage(&mut bytes.as_slice())?)?;
    let mut out = Vec::new();
    fx.blobs.get(&hash)?.read_to_end(&mut out)?;
    ensure!(out == bytes, "round trip");
    let listed = fx.blobs.list(None, 10)?;
    ensure!(
        listed.len() == 1 && listed[0].hash == hash,
        "list {listed:?}"
    );
    ensure!(
        fx.blobs.list(Some(&hash), 10)?.is_empty(),
        "list after the last hash"
    );
    (fx.corrupt_blob)(&hash, b"evidence bytez".to_vec())?;
    let mut altered = Vec::new();
    let error = fx.blobs.get(&hash)?.read_to_end(&mut altered).err();
    ensure!(
        error.as_ref().is_some_and(
            |e| e.to_string().contains("HashMismatch") && e.to_string().contains(hash.as_str())
        ),
        "altered content: {error:?}"
    );
    fx.blobs.delete(&hash)?;
    fx.blobs.delete(&hash)?;
    ensure!(
        fx.blobs.get(&hash).is_err() && fx.blobs.list(None, 10)?.is_empty(),
        "deleted"
    );
    Ok(())
}

fn refs(fx: &Fixture, hash: &BlobHash) -> Result<(u64, Option<jiff::Timestamp>)> {
    let row = blob(fx, hash)?.ok_or_else(|| anyhow::anyhow!("blob {hash} has no row"))?;
    Ok((row.refs, row.unreferenced_since))
}

/// Claim and usage-total documents hold their references like every other document, and
/// records whose `area/key` concatenations coincide keep separate reference sets.
fn documents_keep_their_blobs(make: MakeFixture<'_>) -> Result<()> {
    let fx = make();
    apply(&fx, claim(&fx, STORAGE_GC_CLAIM, "gc"))?;
    let held = |name: &str, hash: &BlobHash, uses: Vec<ReservationUse>| {
        Operation::Claim(Claim {
            name: name.to_owned(),
            owner: "worker".to_owned(),
            acquired: fx.clock.now(),
            doc: json!({"evidence": [hash.to_ref()]}),
            reservations: uses,
        })
    };
    let in_claim = unreferenced_blob(&fx, b"claim evidence")?;
    apply(&fx, held("evidence-claim", &in_claim, Vec::new()))?;
    ensure!(
        refs(&fx, &in_claim)? == (1, None),
        "claim reference {:?}",
        refs(&fx, &in_claim)?
    );
    fx.clock.advance(30 * HOUR);
    refused(
        &fx,
        begin(&fx, &in_claim, "gc")?,
        "GC of a claim-held blob",
        precondition,
    )?;
    let release = Operation::ReleaseClaim(ReleaseClaim {
        name: "evidence-claim".to_owned(),
        owner: "worker".to_owned(),
    });
    apply(&fx, release)?;
    ensure!(
        refs(&fx, &in_claim)? == (0, Some(fx.clock.now())),
        "released claim {:?}",
        refs(&fx, &in_claim)?
    );
    refused(
        &fx,
        begin(&fx, &in_claim, "gc")?,
        "GC inside the grace after release",
        precondition,
    )?;
    fx.clock.advance(25 * HOUR);
    apply(&fx, begin(&fx, &in_claim, "gc")?)?;

    let writing = BlobHash::of(b"claim upload");
    reserved(&fx, reserve_op(&writing, "up"))?;
    refused(
        &fx,
        held("upload", &writing, Vec::new()),
        "claim of a writing blob without its reservation",
        precondition,
    )?;
    let token = ReservationUse {
        hash: writing.clone(),
        token: "up".to_owned(),
    };
    apply(&fx, held("upload", &writing, vec![token]))?;
    let row = blob(&fx, &writing)?.ok_or_else(|| anyhow::anyhow!("row"))?;
    ensure!(
        row.state == BlobState::Live && row.refs == 1 && row.reservations.is_empty(),
        "claimed upload {row:?}"
    );

    let num = base_task(&fx)?;
    let in_usage = unreferenced_blob(&fx, b"usage evidence")?;
    let usage = |doc: serde_json::Value| -> Result<Operation> {
        let mut change = change(&fx, num)?;
        change.aggregates = vec![AggregateWrite::UsageTotal {
            scope: "task".to_owned(),
            key: "T-001".to_owned(),
            doc,
        }];
        Ok(Operation::UpdateTask(UpdateTask {
            change,
            form: UpdateForm::Head,
        }))
    };
    apply(&fx, usage(json!({"transcript": in_usage.to_ref()}))?)?;
    ensure!(
        refs(&fx, &in_usage)? == (1, None),
        "usage reference {:?}",
        refs(&fx, &in_usage)?
    );
    fx.clock.advance(30 * HOUR);
    refused(
        &fx,
        begin(&fx, &in_usage, "gc")?,
        "GC of a usage-held blob",
        precondition,
    )?;
    apply(&fx, usage(json!({"tokens": 5}))?)?;
    ensure!(
        refs(&fx, &in_usage)? == (0, Some(fx.clock.now())),
        "replaced usage {:?}",
        refs(&fx, &in_usage)?
    );

    let now = fx.clock.now();
    let (in_record, used) = reserve(&fx, b"record evidence", "rec")?;
    let write = |area: &str, key: &str, expected, doc| record(area, key, expected, doc, now);
    let with_ref = Some(json!({"evidence": in_record.to_ref()}));
    apply(
        &fx,
        put_records(vec![write("a", "b/c", None, with_ref.clone())], vec![used]),
    )?;
    apply(
        &fx,
        put_records(
            vec![write("a/b", "c", None, Some(json!({"unrelated": true})))],
            Vec::new(),
        ),
    )?;
    ensure!(
        refs(&fx, &in_record)?.0 == 1,
        "an unrelated record dropped the reference"
    );
    apply(
        &fx,
        put_records(vec![write("a/b", "c", Some(1), with_ref)], Vec::new()),
    )?;
    ensure!(refs(&fx, &in_record)?.0 == 2, "two records, two references");
    apply(
        &fx,
        put_records(vec![write("a", "b/c", Some(1), None)], Vec::new()),
    )?;
    ensure!(
        refs(&fx, &in_record)? == (1, None),
        "deleting one record kept the other's reference"
    );
    apply(
        &fx,
        put_records(vec![write("a/b", "c", Some(2), None)], Vec::new()),
    )?;
    ensure!(refs(&fx, &in_record)?.0 == 0, "both records deleted");
    ensure_integrity(&fx)
}
