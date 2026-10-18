use super::*;
use corral_core::{AuthorizedSubmission, SubmissionDisposition};

fn interrupted(
    disposition: SubmissionDisposition,
) -> (
    tempfile::TempDir,
    Ctx,
    String,
    String,
    corral_core::PlanApproval,
    PathBuf,
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
    let proof = ctx
        .store
        .state_dir()
        .join("action-authorizations")
        .join(format!("{authorization}.json"));
    let mut record = ctx.store.authorized_submission(authorization).unwrap();
    assert_eq!(record.disposition, SubmissionDisposition::Submitted);
    // Reconstruct the durable prefix: approval saved, receipt not completed,
    // publication not started. This is deterministic persisted-state injection.
    record.disposition = disposition;
    std::fs::write(&proof, serde_json::to_vec(&record).unwrap()).unwrap();
    std::fs::remove_file(
        ctx.store
            .state_dir()
            .join(format!("plan-decompose-{id}.json")),
    )
    .unwrap();
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
        let before: AuthorizedSubmission =
            serde_json::from_slice(&std::fs::read(&proof).unwrap()).unwrap();
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
            let before = std::fs::read(&proof).unwrap();

            // When ordinary sync observes the stale approval and unfinished receipt.
            sync_twice(&ctx);

            // Then neither history nor proof is promoted using withdrawn authority.
            let task = ctx.store.get(&id).unwrap();
            assert!(task.plan.is_none(), "{disposition:?} {change}");
            assert_eq!(
                task.plans.last().unwrap().approval.as_ref(),
                Some(&approval)
            );
            assert_eq!(std::fs::read(&proof).unwrap(), before);
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
        let before = serde_json::to_vec(&record).unwrap();
        std::fs::write(&proof, &before).unwrap();

        // When sync tries to recover this accepted plan.
        sync_twice(&ctx);

        // Then it cannot fabricate a submitted proof or publish implementation work.
        let task = ctx.store.get(&id).unwrap();
        assert!(task.plan.is_none(), "{change}");
        assert_eq!(
            task.plans.last().unwrap().approval.as_ref(),
            Some(&approval)
        );
        assert_eq!(std::fs::read(&proof).unwrap(), before);
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
            let before = std::fs::read(&proof).unwrap();

            // When a stale context syncs twice while consent is narrowed.
            sync_twice(&ctx);

            // Then it retains the approval without completing proof or publishing.
            let task = ctx.store.get(&id).unwrap();
            assert!(task.plan.is_none(), "{disposition:?} hold={hold}");
            assert_eq!(
                task.plans.last().unwrap().approval.as_ref(),
                Some(&approval)
            );
            assert_eq!(std::fs::read(&proof).unwrap(), before);
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
