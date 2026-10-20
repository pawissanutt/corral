use super::*;
use crate::operating::Origin;
use corral_core::{
    OperatingAuthority, OperatingOrigin, OperatingSelection, ReviewRecord, SubmissionDisposition,
};

pub(super) fn grant(fx: &mut Fixture, ids: &[String]) {
    fx.ctx.store.config.dispatch.paused = false;
    fx.ctx.store.save_config().unwrap();
    let selection = OperatingSelection {
        tasks: ids.iter().cloned().collect(),
        authority: OperatingAuthority {
            dispatch: true,
            accept: false,
            plan_accept: false,
        },
        ..OperatingSelection::default()
    };
    let preview = fx.ctx.store.preview_operating_policy(&selection).unwrap();
    fx.ctx
        .store
        .confirm_operating_policy(&preview, "owner", Timestamp::now())
        .unwrap();
    fx.ctx.store.reload_config_if_changed().unwrap();
}

fn prepared<'a>(fx: &'a Fixture, id: &str, kind: Kind) -> ReviewLaunch<'a> {
    let mut launch =
        ReviewLaunch::reserve_origin(&fx.ctx, id, kind, false, Origin::Automatic).unwrap();
    let task = fx.ctx.store.get(id).unwrap();
    let run = task.current_run().unwrap();
    let candidate =
        crate::verification::capture_identity(&fx.ctx, &task, run, "candidate").unwrap();
    let child_id = (kind == Kind::Reviewer).then_some("R1-review1");
    launch
        .bind(&candidate, child_id.unwrap_or("candidate"), child_id)
        .unwrap();
    launch.attach("w1:prepared", true).unwrap();
    fx.ctx
        .store
        .update(id, |task| {
            let run = task.current_run_mut().unwrap();
            match kind {
                Kind::Gate => {
                    run.gate = Some(serde_json::from_value(json!({
                        "result": "running", "sha": candidate.head,
                        "at": "2026-10-18T00:00:00Z", "pane": "w1:prepared",
                        "socket": fx.ctx.herdr.socket_path().unwrap(), "identity": candidate
                    }))?);
                }
                Kind::Reviewer => {
                    let record = ReviewRecord {
                        route: None,
                        identity: Some(candidate.clone()),
                        evidence_digest: Some(crate::verification::evidence_digest(run)?),
                        verdict: None,
                        process_lost: false,
                        reviewer: "rev".into(),
                        notes: None,
                        sha: candidate.head.clone(),
                        at: Timestamp::now(),
                        pane: Some("w1:prepared".into()),
                    };
                    let mut child = run.clone();
                    child.id = "R1-review1".into();
                    child.review = Some(record.clone());
                    child.review_runs.clear();
                    child.pending_reply = Some("prepared transport request".into());
                    run.review = Some(record);
                    run.review_runs.push(child);
                }
            }
            Ok(())
        })
        .unwrap();
    let prepared_file = match kind {
        Kind::Gate => crate::gate::gate_paths(&fx.ctx, id, &run.id).script,
        Kind::Reviewer => fx
            .ctx
            .store
            .prompts_dir()
            .join(format!("{id}-R1-review.md")),
    };
    std::fs::create_dir_all(prepared_file.parent().unwrap()).unwrap();
    std::fs::write(prepared_file, "prepared").unwrap();
    launch
}

fn retained(fx: &Fixture, id: &str, kind: Kind) -> Pending {
    let kind = match kind {
        Kind::Gate => "gate",
        Kind::Reviewer => "review",
    };
    serde_json::from_slice(
        &std::fs::read(
            fx.ctx
                .store
                .state_dir()
                .join("review-prepared")
                .join(format!("{id}-R1-{kind}.json")),
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn refusal_retains_exact_auxiliary_identity_without_holding_capacity() {
    for kind in [Kind::Gate, Kind::Reviewer] {
        // Given an automatic attempt with attached preparation and a subscribed channel.
        let mut fx = Fixture::new(1);
        let id = fx.task();
        let other = fx.task();
        grant(&mut fx, &[id.clone(), other.clone()]);
        let (arrived, arrival) = mpsc::channel();
        let (release, released) = mpsc::channel();
        std::thread::scope(|scope| {
            let fx = &fx;
            let id = &id;
            let handle = scope.spawn(move || {
                let mut launch = prepared(fx, id, kind);
                let authorization = launch.pending.authorization.clone().unwrap();
                arrived.send(authorization.clone()).unwrap();
                released.recv_timeout(Duration::from_secs(5)).unwrap();
                let error = launch
                    .submit(|_, _| Ok(()), || -> Result<()> { panic!("refused IPC") })
                    .unwrap_err();
                assert!(crate::operating::refused(&error), "{error:#}");
                authorization
            });
            let expected = arrival.recv_timeout(Duration::from_secs(5)).unwrap();
            // When a second fresh context revokes before the final submission boundary.
            let store = Store::open(fx.ctx.store.root()).unwrap();
            store.pause_operating_policy(true).unwrap();
            release.send(()).unwrap();
            assert_eq!(handle.join().unwrap(), expected);

            // Then refusal is definite, retained, and not a failed/lost process.
            let pending = retained(fx, id, kind);
            assert!(pending.authorization.as_ref() == Some(&expected));
            assert_eq!(pending.automatic, Some(true));
            assert_eq!(pending.submitted, Some(false));
            assert!(pending.refused.is_some());
            assert!(matches!(
                store
                    .authorized_submission(&expected.id)
                    .unwrap()
                    .disposition,
                SubmissionDisposition::AuthorizationRefused { .. }
            ));
            let task = store.get(id).unwrap();
            assert!(task.review_reservation.is_none());
            let run = task.current_run().unwrap();
            assert_eq!(run.failures, 0);
            match kind {
                Kind::Gate => {
                    assert!(run.gate.is_none());
                    let record = pending.prepared_gate.unwrap();
                    assert_eq!(record.identity, expected.action.candidate);
                    assert!(!record.process_lost);
                    assert_eq!(record.result, GateResult::Running);
                    assert!(
                        fx.ctx
                            .store
                            .state_dir()
                            .join("gates")
                            .join(format!("{id}-R1-candidate.sh"))
                            .is_file()
                    );
                }
                Kind::Reviewer => {
                    assert!(run.review.is_none());
                    let child = run.review_runs.last().unwrap();
                    assert_eq!(Some(&child.id), pending.review_run.as_ref());
                    assert!(child.pending_reply.is_some());
                    assert!(child.ended.is_some());
                    assert!(!child.review.as_ref().unwrap().process_lost);
                    assert!(
                        fx.ctx
                            .store
                            .prompts_dir()
                            .join(format!("{id}-R1-review.md"))
                            .is_file()
                    );
                }
            }
            assert!(!journal(&fx.ctx).exists());
            assert!(ReviewLaunch::reserve(&fx.ctx, id, kind).is_err());
            let fresh = Ctx {
                store: Store::open(fx.ctx.store.root()).unwrap(),
                ..fx.ctx.clone()
            };
            ReviewLaunch::reserve(&fresh, &other, kind)
                .unwrap()
                .commit()
                .unwrap();
        });
    }
}

#[test]
fn automatic_wrappers_do_not_fall_back_to_owner_in_assist() {
    // Given manual-capable tasks but no scoped automatic grant.
    let fx = Fixture::new(1);
    let gate = fx.task();
    let review = fx.task();

    // When both automatic entry points are invoked.
    assert!(crate::gate::gate_start_automatic(&fx.ctx, &gate).is_err());
    assert!(crate::reviewer::review_run_automatic(&fx.ctx, &review, Some("rev")).is_err());

    // Then no pane, preparation journal, gate or review attempt exists.
    assert_eq!(fx.created(), 0);
    assert!(!journal(&fx.ctx).exists());
    assert!(
        fx.ctx
            .store
            .get(&gate)
            .unwrap()
            .current_run()
            .unwrap()
            .gate
            .is_none()
    );
    assert!(
        fx.ctx
            .store
            .get(&review)
            .unwrap()
            .current_run()
            .unwrap()
            .review_runs
            .is_empty()
    );
}

#[test]
fn attachment_is_not_submission_and_crash_recovery_never_infers_origin() {
    // Given a bound automatic attachment with no attempted IPC.
    let mut fx = Fixture::new(1);
    let id = fx.task();
    let other = fx.task();
    grant(&mut fx, &[id.clone(), other.clone()]);
    let mut launch = prepared(&fx, &id, Kind::Gate);
    let authorization = launch.pending.authorization.clone().unwrap();
    // Retaining a legacy missing origin must not manufacture an owner authorization.
    launch.pending.automatic = None;
    save(&fx.ctx, &launch.pending).unwrap();
    assert_eq!(launch.pending.submitted, Some(false));

    // When launcher cleanup runs after authority has been revoked.
    fx.ctx.store.assist_operating_policy().unwrap();
    drop(launch);

    // Then no process is marked lost and automatic core provenance survives unchanged.
    let pending = retained(&fx, &id, Kind::Gate);
    assert_eq!(pending.automatic, None);
    assert_eq!(pending.authorization, Some(authorization.clone()));
    assert!(matches!(
        authorization.origin,
        OperatingOrigin::Automatic { .. }
    ));
    assert!(matches!(
        fx.ctx
            .store
            .authorized_submission(&authorization.id)
            .unwrap()
            .disposition,
        SubmissionDisposition::Prepared
    ));
    assert!(!pending.prepared_gate.unwrap().process_lost);
    let fresh = Ctx {
        store: Store::open(fx.ctx.store.root()).unwrap(),
        ..fx.ctx.clone()
    };
    ReviewLaunch::reserve(&fresh, &other, Kind::Reviewer)
        .unwrap()
        .commit()
        .unwrap();
}

#[test]
fn changed_accounting_child_refuses_without_transport_or_replacement() {
    // Given a prepared reviewer whose accounting child changes before submission.
    let mut fx = Fixture::new(1);
    let id = fx.task();
    grant(&mut fx, std::slice::from_ref(&id));
    let mut launch = prepared(&fx, &id, Kind::Reviewer);
    fx.ctx
        .store
        .update(&id, |task| {
            task.current_run_mut()
                .unwrap()
                .review_runs
                .last_mut()
                .unwrap()
                .id = "replacement".into();
            Ok(())
        })
        .unwrap();

    // When the exact serialized identity is checked.
    let error = launch
        .submit(
            |_, _| Ok(()),
            || -> Result<()> { panic!("changed child must not submit") },
        )
        .unwrap_err();

    // Then it is a known refusal and the unrelated replacement remains intact.
    assert!(crate::operating::refused(&error));
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(
        task.current_run().unwrap().review_runs.last().unwrap().id,
        "replacement"
    );
    assert_eq!(
        retained(&fx, &id, Kind::Reviewer).review_run.as_deref(),
        Some("R1-review1")
    );
}

#[test]
fn owner_wrapper_persists_explicit_provenance_and_running_receipt_survives_revocation() {
    // Given a manually submitted gate and subsequent Assist policy.
    let fx = Fixture::new(1);
    let id = fx.task();
    let gate = crate::gate::gate_start(&fx.ctx, &id).unwrap();
    let identity = gate.identity.as_ref().unwrap();
    let authorization = crate::operating::load(
        &fx.ctx,
        &id,
        "R1",
        corral_core::OperatingActionKind::Gate,
        &identity.attempt,
    )
    .unwrap();
    assert!(matches!(
        authorization.origin,
        OperatingOrigin::ExplicitOwner { .. }
    ));
    fx.ctx.store.assist_operating_policy().unwrap();
    let result = support::command("bash", fx.tmp.path())
        .arg(crate::gate::gate_paths(&fx.ctx, &id, "R1").script)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(result.status.success());

    // When a genuine running receipt is collected with no automatic authority.
    let collected = crate::gate::gate_collect(&fx.ctx, &id).unwrap().unwrap();

    // Then receipt reconciliation is unaffected by the grant.
    assert_eq!(collected.result, GateResult::Pass);
    assert_eq!(collected.identity, gate.identity);
}
