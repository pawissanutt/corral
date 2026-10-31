use super::*;
use corral_core::{AuthorizedSubmission, SubmissionDisposition};

// Reconstruct a deliberately interrupted or foreign receipt in a disposable database.
// Production transitions never roll Submitted back or rewrite an attempted identity.
fn replace_proof(ctx: Ctx, replacement: &AuthorizedSubmission) -> Ctx {
    use corral_core::backend::{
        Authorization, Disposition, Operation, PageReq, PutRecords, RecordQuery, RecordWrite,
        StorageQuery, StorageRow, StoreBackend,
    };
    use corral_core::sqlite::{SqliteBackend, SqliteOptions};

    let tasks = ctx.store.list().unwrap();
    assert_eq!(tasks.len(), 1, "this fixture owns exactly one task");
    let state = ctx
        .store
        .view()
        .unwrap()
        .records(&RecordQuery::default(), PageReq::first(128))
        .unwrap();
    assert!(state.next.is_none(), "fixture record page was truncated");
    // Reconstruct the real canonical evidence too: no mirror snapshot can stand in for
    // a blob row/reference when the interrupted authorization is replayed.
    use std::io::Read;
    let state_records = state.items;
    let hashes: std::collections::BTreeSet<_> = state_records
        .iter()
        .flat_map(|record| corral_core::blob_ref::scan(&record.doc))
        .collect();
    let evidence: std::collections::BTreeMap<_, _> = hashes
        .into_iter()
        .map(|hash| {
            let mut bytes = Vec::new();
            ctx.store
                .blob_reader(&hash)
                .unwrap()
                .unwrap()
                .read_to_end(&mut bytes)
                .unwrap();
            (hash, bytes)
        })
        .collect();
    let events = ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .map(|event| serde_json::to_string(&event).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let page = ctx
        .store
        .view()
        .unwrap()
        .storage_rows(
            &StorageQuery::Authorizations { task: None },
            PageReq::first(128),
        )
        .unwrap();
    assert!(
        page.next.is_none(),
        "fixture authorization page was truncated"
    );
    let records = page
        .items
        .into_iter()
        .map(|row| {
            let StorageRow::Authorization(row) = row else {
                panic!("authorization query returned another row");
            };
            if row.id == replacement.authorization.id {
                replacement.clone()
            } else {
                ctx.store.authorized_submission(&row.id).unwrap()
            }
        })
        .collect::<Vec<_>>();
    assert!(records.iter().any(|record| record == replacement));
    let root = ctx.store.root().to_owned();
    let Ctx {
        store,
        herdr,
        resource_probe,
    } = ctx;
    drop(store);
    let old = tempfile::tempdir_in(&root).unwrap();
    let corral = root.join(".corral");
    std::fs::rename(corral.join("db"), old.path().join("db")).unwrap();
    let backend = SqliteBackend::open(&corral, SqliteOptions::default()).unwrap();
    // Reconstruct only the interrupted receipt, not a different workspace grant.
    // Replay record versions before opening the store so config observation sees
    // the exact original state, including its confirmation and revocation records.
    let blobs = corral_core::blob::FsBlobStore::open(&corral).unwrap();
    let clock = corral_core::backend::SystemClock;
    for record in &state_records {
        let reservations: Vec<_> = corral_core::blob_ref::scan(&record.doc)
            .into_iter()
            .map(|hash| {
                corral_core::blob::Putter::new(&backend, &blobs, &clock)
                    .unwrap()
                    .put(&mut evidence[&hash].as_slice(), &mut |_| Ok(()))
                    .unwrap()
                    .reservation
            })
            .collect();
        for version in 0..record.version {
            backend
                .apply(Operation::PutRecords(PutRecords {
                    writes: vec![RecordWrite {
                        area: record.area.clone(),
                        key: record.key.clone(),
                        expected: (version > 0).then_some(version),
                        doc: Some(record.doc.clone()),
                        updated: record.updated,
                    }],
                    events: vec![],
                    reservations: if version == 0 {
                        reservations.clone()
                    } else {
                        Vec::new()
                    },
                }))
                .unwrap();
        }
    }
    let store = Store::open(&root).unwrap();
    let restored = store
        .view()
        .unwrap()
        .records(&RecordQuery::default(), PageReq::first(128))
        .unwrap();
    assert!(restored.next.is_none());
    assert_eq!(restored.items, state_records);
    for task in tasks {
        store.fixture_put_task(&task).unwrap();
    }
    store.fixture_replace_events(&events).unwrap();
    for mut record in records {
        let stages = match record.disposition.clone() {
            SubmissionDisposition::Prepared => vec![],
            target @ (SubmissionDisposition::Submitted
            | SubmissionDisposition::Uncertain { .. }) => {
                vec![SubmissionDisposition::Submitting, target]
            }
            target => vec![target],
        };
        let mut from = None;
        for stage in std::iter::once(SubmissionDisposition::Prepared).chain(stages) {
            let to = match &stage {
                SubmissionDisposition::Prepared => Disposition::Prepared,
                SubmissionDisposition::Submitting => Disposition::Submitting,
                SubmissionDisposition::Submitted => Disposition::Submitted,
                SubmissionDisposition::Uncertain { .. } => Disposition::Uncertain,
                SubmissionDisposition::AuthorizationRefused { .. } => Disposition::Refused,
            };
            record.disposition = stage;
            backend
                .apply(Operation::Authorization(Authorization {
                    id: record.authorization.id.clone(),
                    task: record.authorization.action.task.clone(),
                    from,
                    to,
                    doc: serde_json::to_value(&record).unwrap(),
                    change: None,
                }))
                .unwrap();
            from = Some(to);
        }
    }
    Ctx {
        store,
        herdr,
        resource_probe,
    }
}

fn interrupted(
    disposition: SubmissionDisposition,
) -> (
    tempfile::TempDir,
    Ctx,
    String,
    String,
    corral_core::PlanApproval,
    String,
) {
    let (root, ctx, id, path) = fixture();
    let plan = pending(&ctx, &id, &path);
    confirm_plan_authority(&ctx.store, &id);
    plan_verdict(
        &ctx,
        &id,
        &PlanVerdict {
            plan: &plan,
            attempt: Some("PR1"),
            verdict: Verdict::Approve,
            notes: "Independent approval",
        },
    )
    .unwrap();
    let accepted = ctx.store.get(&id).unwrap();
    let approval = accepted.plans.last().unwrap().approval.clone().unwrap();
    let authorization = approval.authorization.as_ref().unwrap();
    let proof = authorization.clone();
    let mut record = ctx.store.authorized_submission(authorization).unwrap();
    assert_eq!(record.disposition, SubmissionDisposition::Submitted);
    // Reconstruct the durable prefix: approval saved, receipt not completed,
    // publication not started. This is deterministic persisted-state injection.
    record.disposition = disposition;
    let ctx = replace_proof(ctx, &record);
    ctx.store.delete_record("plan-decompose", &id).unwrap();
    ctx.store
        .update(&id, |task| {
            task.plan = None;
            Ok(())
        })
        .unwrap();
    let mut fresh = ctx.clone();
    fresh.store = Store::open(root.path()).unwrap();
    (root, fresh, id, plan, approval, proof)
}

fn sync_twice(ctx: &Ctx) {
    for _ in 0..2 {
        corral_ops::sync_at(ctx, "2026-10-18T10:00:00Z".parse().unwrap()).unwrap();
    }
}

#[test]
fn interrupted_local_acceptance_reconciles_exact_receipt_without_new_approval() {
    for disposition in [
        SubmissionDisposition::Prepared,
        SubmissionDisposition::Submitting,
    ] {
        // Given either durable crash prefix of a real scoped automatic acceptance.
        let (_root, ctx, id, plan, approval, proof) = interrupted(disposition.clone());
        let before = ctx.store.authorized_submission(&proof).unwrap();
        assert!(approved_plan(&ctx, &ctx.store.get(&id).unwrap()).is_err());
        let mut attached = ctx.store.get(&id).unwrap();
        attached.plan = Some(format!("{id}@{plan}"));
        assert!(
            corral_core::plan_policy::validate_approval(
                &ctx.store,
                &ctx.store.config,
                &attached,
                attached.plans.last().unwrap(),
            )
            .is_err()
        );

        // When ordinary sync restarts, then repeats the completed transition.
        sync_twice(&ctx);

        // Then the same approval and authorization finish once and authorize publication.
        let task = ctx.store.get(&id).unwrap();
        assert_eq!(task.plan, Some(format!("{id}@{plan}")), "{disposition:?}");
        assert_eq!(
            task.plans.last().unwrap().approval.as_ref(),
            Some(&approval)
        );
        assert!(approved_plan(&ctx, &task).is_ok());
        let after = ctx
            .store
            .authorized_submission(approval.authorization.as_ref().unwrap())
            .unwrap();
        assert_eq!(after.authorization, before.authorization);
        assert_eq!(after.authorization_history, before.authorization_history);
        assert_eq!(after.disposition, SubmissionDisposition::Submitted);
    }
}

#[test]
fn withdrawn_or_changed_authority_retains_unfinished_acceptance_without_publication() {
    for disposition in [
        SubmissionDisposition::Prepared,
        SubmissionDisposition::Submitting,
    ] {
        for change in ["assist", "authority", "scope", "contract"] {
            // Given a durable acceptance followed by a new narrowing or contract change.
            let (root, ctx, id, _plan, approval, proof) = interrupted(disposition.clone());
            let other = Store::open(root.path()).unwrap();
            match change {
                "assist" => {
                    other.assist_operating_policy().unwrap();
                }
                "authority" => {
                    other
                        .edit_config(|doc| doc.set_str("automation.authority.plan_accept", "false"))
                        .unwrap();
                }
                "scope" => {
                    other
                        .edit_config(|doc| doc.unset("automation.scope.tasks").map(|_| ()))
                        .unwrap();
                }
                "contract" => {
                    other
                        .update(&id, |task| {
                            task.write_scope.push("changed/**".into());
                            Ok(())
                        })
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let before = ctx.store.authorized_submission(&proof).unwrap();

            // When ordinary sync observes the stale approval and unfinished receipt.
            sync_twice(&ctx);

            // Then neither history nor proof is promoted using withdrawn authority.
            let task = ctx.store.get(&id).unwrap();
            assert!(task.plan.is_none(), "{disposition:?} {change}");
            assert_eq!(
                task.plans.last().unwrap().approval.as_ref(),
                Some(&approval)
            );
            assert_eq!(ctx.store.authorized_submission(&proof).unwrap(), before);
            assert!(task.children.is_empty());
        }
    }
}

#[test]
fn foreign_acceptance_identity_and_uncertain_receipts_are_never_promoted() {
    for change in ["plan", "run", "attempt", "origin", "uncertain"] {
        // Given an unfinished receipt that no longer proves the exact local approval.
        let (_root, ctx, id, _plan, approval, proof) =
            interrupted(SubmissionDisposition::Submitting);
        let mut record = ctx
            .store
            .authorized_submission(approval.authorization.as_ref().unwrap())
            .unwrap();
        match change {
            "plan" => record.authorization.action.plan = Some("P999".into()),
            "run" => record.authorization.action.run = "D999".into(),
            "attempt" => record.authorization.action.attempt = "accept-PR999".into(),
            "origin" => {
                record.authorization.origin = corral_core::OperatingOrigin::ExplicitOwner {
                    approval: "foreign".into(),
                    confirmed_by: "owner".into(),
                    confirmed_at: "2026-10-18T10:00:00Z".parse().unwrap(),
                };
            }
            "uncertain" => {
                record.disposition = SubmissionDisposition::Uncertain {
                    reason: "Not a local receipt-write prefix".into(),
                };
            }
            _ => unreachable!(),
        }
        let before = record.clone();
        let ctx = replace_proof(ctx, &record);

        // When sync tries to recover this accepted plan.
        sync_twice(&ctx);

        // Then it cannot fabricate a submitted proof or publish implementation work.
        let task = ctx.store.get(&id).unwrap();
        assert!(task.plan.is_none(), "{change}");
        assert_eq!(
            task.plans.last().unwrap().approval.as_ref(),
            Some(&approval)
        );
        assert_eq!(ctx.store.authorized_submission(&proof).unwrap(), before);
        assert!(task.children.is_empty());
    }
}

#[test]
fn revoked_local_acceptance_cannot_reconcile_until_same_authority_is_restored() {
    for disposition in [
        SubmissionDisposition::Prepared,
        SubmissionDisposition::Submitting,
    ] {
        for hold in [false, true] {
            // Given an unfinished approval followed by a fresh pause or task hold.
            let (root, ctx, id, plan, approval, proof) = interrupted(disposition.clone());
            let other = Store::open(root.path()).unwrap();
            if hold {
                other
                    .update(&id, |task| {
                        task.dispatch_hold = true;
                        Ok(())
                    })
                    .unwrap();
            } else {
                other.pause_operating_policy(true).unwrap();
            }
            let before = ctx.store.authorized_submission(&proof).unwrap();

            // When a stale context syncs twice while consent is narrowed.
            sync_twice(&ctx);

            // Then it retains the approval without completing proof or publishing.
            let task = ctx.store.get(&id).unwrap();
            assert!(task.plan.is_none(), "{disposition:?} hold={hold}");
            assert_eq!(
                task.plans.last().unwrap().approval.as_ref(),
                Some(&approval)
            );
            assert_eq!(ctx.store.authorized_submission(&proof).unwrap(), before);
            assert!(approved_plan(&ctx, &task).is_err());

            // Restoring that same grant removes the blocker without another acceptance.
            if hold {
                other
                    .update(&id, |task| {
                        task.dispatch_hold = false;
                        Ok(())
                    })
                    .unwrap();
            } else {
                other.pause_operating_policy(false).unwrap();
            }
            sync_twice(&ctx);
            assert_eq!(
                ctx.store.get(&id).unwrap().plan,
                Some(format!("{id}@{plan}"))
            );
        }
    }
}
