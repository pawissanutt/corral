use super::*;
use crate::attention::handling_tests::fixture::{Fx, at};
use corral_core::{AvailabilityEpisode, ProfileRole, ResourceState, Status};

fn pressure(severe: bool) -> ResourceState {
    ResourceState {
        snapshot: crate::resources::FixedResourceProbe::healthy().0,
        reason: severe.then(|| "fixture memory pressure".into()),
        severe,
        held: severe,
        observed_at: at(0),
    }
}

#[test]
fn real_store_entry_binding_covers_pm_lost_pane_without_exempting_the_batch() {
    // Given an actual Needs me entry written with its claim in one Store transaction.
    let fx = Fx::new();
    let task = fx.questions(0);
    let mut pending = None;
    let saved = fx
        .ctx
        .store
        .update(&task.id, |task| {
            task.current_run_mut().unwrap().agent_state = Some("gone".into());
            task.status = Status::NeedsMe;
            pending = Some(record_entry(
                &fx.ctx,
                task,
                "needs_me_entry",
                at(0),
                Vec::new(),
            )?);
            Ok(())
        })
        .unwrap();
    let batch = pending.as_ref().unwrap().batch_id.clone();
    assert_eq!(
        saved.attention.entry_notice.as_ref().unwrap().status_since,
        saved.status_since
    );
    assert!(saved.attention.notice_outbox[0].exemption_basis.is_none());
    notify(
        &fx.ctx,
        newly_committed(&saved, pending.unwrap()).unwrap(),
        "lost",
        "body",
        true,
    )
    .unwrap();
    // When attention opens and crosses both deadlines after restart.
    crate::attention::reconcile(&fx.reopen(), at(0), Some(true)).unwrap();
    crate::attention::reconcile(&fx.reopen(), at(1800), Some(true)).unwrap();
    // Then PM responsibility retains its re-wake but never sends a second owner notification.
    assert_eq!(fx.calls(), 1);
    let saved = fx.ctx.store.get(&task.id).unwrap();
    let record = saved
        .attention
        .records
        .values()
        .find(|record| record.kind == "agent_lost")
        .unwrap();
    assert_eq!(record.levels.owner_claim.as_deref(), Some(batch.as_str()));
    assert!(record.levels.rewoke_at.is_some());
}

#[test]
fn later_question_revision_is_not_covered_by_an_earlier_status_entry() {
    // Given an owner entry for another reason, followed by a later escalated question.
    let fx = Fx::new();
    let task = fx.questions(1);
    let mut pending = None;
    let saved = fx
        .ctx
        .store
        .update(&task.id, |task| {
            task.status = Status::NeedsMe;
            pending = Some(record_entry(
                &fx.ctx,
                task,
                "needs_me_entry",
                at(0),
                Vec::new(),
            )?);
            Ok(())
        })
        .unwrap();
    notify(
        &fx.ctx,
        newly_committed(&saved, pending.unwrap()).unwrap(),
        "entry",
        "body",
        true,
    )
    .unwrap();
    fx.ctx
        .store
        .update(&task.id, |task| {
            let question = &mut task
                .current_run_mut()
                .unwrap()
                .agent_host
                .as_mut()
                .unwrap()
                .questions[0];
            question.revision = 2;
            question.state = corral_core::agent::QuestionState::Escalated;
            question.unattended_at = Some(at(1));
            Ok(())
        })
        .unwrap();
    // When the later question passes its own deadline.
    crate::attention::reconcile(&fx.ctx, at(1), Some(true)).unwrap();
    crate::attention::reconcile(&fx.reopen(), at(1801), Some(true)).unwrap();
    crate::attention::reconcile(&fx.reopen(), at(3600), Some(true)).unwrap();
    // Then exactly one additional ladder batch covers only that revision.
    assert_eq!(fx.calls(), 2);
    let events = fx.ctx.store.events().unwrap();
    let ladder: Vec<_> = events
        .iter()
        .filter(|event| {
            event.kind == "notification_claimed" && event.detail["producer"] == "attention_ladder"
        })
        .collect();
    assert_eq!(ladder.len(), 1);
    assert!(
        ladder[0].detail["coverage"][0]["condition_key"]
            .as_str()
            .unwrap()
            .ends_with("@2")
    );
}

#[test]
fn stage_escalation_claim_retains_its_real_producer_and_suppresses_ladder_owner_send() {
    for producer in [
        "autopilot_escalate",
        "legacy_review_fallback",
        "legacy_review_watchdog",
        "native_review_watchdog",
    ] {
        // Given each stage producer's canonical Escalated claim.
        let fx = Fx::new();
        let task = fx.stalled(None);
        let mut pending = None;
        let saved = fx
            .ctx
            .store
            .update(&task.id, |task| {
                task.status = Status::NeedsMe;
                let run = task.current_run_mut().unwrap();
                run.autopilot = Some(corral_core::AutopilotState {
                    stage: corral_core::AutoStage::Escalated,
                    at: at(0),
                    sha: None,
                    reason: Some("integration preflight error: fixture".into()),
                });
                pending = Some(record_stage(&fx.ctx, task, producer, at(0))?);
                Ok(())
            })
            .unwrap();
        notify(
            &fx.ctx,
            newly_committed(&saved, pending.unwrap()).unwrap(),
            "escalated",
            "body",
            true,
        )
        .unwrap();
        // When the independent PM ladder opens and reaches its deadlines.
        for seconds in [0, 900, 1800, 7200] {
            crate::attention::reconcile(&fx.reopen(), at(seconds), Some(true)).unwrap();
        }
        // Then the one physical invocation keeps its producer identity.
        assert_eq!(fx.calls(), 1, "{producer}");
        let events = fx.ctx.store.events().unwrap();
        let notice = events
            .iter()
            .find(|event| event.kind == "owner_notified")
            .unwrap();
        assert_eq!(notice.detail["producer"], producer);
        assert_eq!(fx.logical("attention_overdue").len(), 1, "{producer}");
    }
}

#[test]
fn real_resource_entries_and_taskless_alerts_survive_all_notification_crash_windows() {
    for selected in [false, true] {
        for point in [
            CrashPoint::AfterClaim,
            CrashPoint::BeforeNotify,
            CrashPoint::AfterNotify,
            CrashPoint::AfterOutcomeSave,
            CrashPoint::AfterAppend,
            CrashPoint::BeforeClear,
        ] {
            // Given the production resource producer, with and without a selected task.
            let fx = Fx::new();
            let task = selected.then(|| fx.questions(0));
            CRASH.with(|crash| crash.set(Some(point)));
            assert!(
                crate::resource_escalation::reconcile(&fx.ctx, &pressure(true), at(0)).is_err(),
                "{point:?}"
            );
            let calls = fx.calls();
            // When restart drains the same singleton, then recovery clears it.
            let reopened = fx.reopen();
            crate::resource_escalation::reconcile(&reopened, &pressure(true), at(1)).unwrap();
            drain(&reopened).unwrap();
            crate::resource_escalation::reconcile(&reopened, &pressure(false), at(2)).unwrap();
            drain(&reopened).unwrap();
            assert_eq!(fx.calls(), calls, "{point:?}");
            assert_eq!(fx.logical("notification_claimed").len(), 1, "{point:?}");
            assert!(
                !fx.tmp
                    .path()
                    .join(".corral/state/resource-alert.json")
                    .exists()
            );
            if let Some(task) = task {
                let after = reopened.store.get(&task.id).unwrap();
                assert!(after.attention.source_notices.is_empty());
                assert_eq!(after.status, Status::Running);
            }
        }
    }
}

#[test]
fn standalone_episode_registration_and_source_clear_survive_terminal_tasks() {
    for point in [
        CrashPoint::BeforeSourceClaim,
        CrashPoint::AfterClaim,
        CrashPoint::BeforeNotify,
        CrashPoint::AfterNotify,
        CrashPoint::AfterOutcomeSave,
        CrashPoint::AfterAppend,
        CrashPoint::AfterSourceClear,
    ] {
        // Given a known standalone episode, retained by the owning task's keyed reference.
        let fx = Fx::new();
        let task = fx
            .ctx
            .store
            .create("Terminal", None, Status::Done, "")
            .unwrap();
        let key = format!("{}-R1-implementation", task.id);
        let claim = prepare_subject(
            &fx.ctx,
            Some(&task.id),
            "provider_wait",
            &key,
            at(0),
            Vec::new(),
        )
        .unwrap();
        CRASH.with(|crash| crash.set(Some(point)));
        let result = (|| -> Result<()> {
            let registration =
                register_source(&fx.ctx, Some(&task.id), "availability", &key, &claim)?;
            let episode = AvailabilityEpisode {
                version: 1,
                id: key.clone(),
                task: task.id.clone(),
                role: ProfileRole::Implementation,
                profile: "fixture".into(),
                revision: "1".into(),
                started: at(0),
                attempts: Vec::new(),
                exhausted: None,
                notice: Some(claim.clone()),
            };
            let dir = fx.ctx.store.state_dir().join("availability");
            std::fs::create_dir_all(&dir)?;
            std::fs::write(
                dir.join(format!("{key}.json")),
                serde_json::to_vec(&episode)?,
            )?;
            notify(
                &fx.ctx,
                source_committed(Some(&task.id), registration, claim.clone())?,
                "provider reset",
                "body",
                false,
            )
        })();
        assert!(result.is_err(), "{point:?}");
        let calls = fx.calls();
        // When a new owner drains twice, including an absent pre-claim source.
        drain(&fx.reopen()).unwrap();
        drain(&fx.reopen()).unwrap();
        // Then the ref retires only after retained facts publish, without resending.
        assert_eq!(fx.calls(), calls, "{point:?}");
        assert!(
            fx.ctx
                .store
                .get(&task.id)
                .unwrap()
                .attention
                .source_notices
                .is_empty()
        );
        assert_eq!(
            fx.logical("notification_claimed").len(),
            usize::from(point != CrashPoint::BeforeSourceClaim)
        );
        if point == CrashPoint::AfterOutcomeSave {
            assert_eq!(fx.logical("owner_notified").len(), 1);
        }
    }
}

#[test]
fn an_absent_source_reference_is_not_removed_under_a_live_source_executor() {
    // Given a registration whose executor has not yet committed its source claim.
    let fx = Fx::new();
    let task = fx.questions(0);
    let claim = prepare_subject(
        &fx.ctx,
        Some(&task.id),
        "provider_wait",
        "pending",
        at(0),
        Vec::new(),
    )
    .unwrap();
    let registration =
        register_source(&fx.ctx, Some(&task.id), "availability", "pending", &claim).unwrap();
    // When another pass sees the writer lease, then later sees no evidence of settlement.
    drain(&fx.ctx).unwrap();
    assert_eq!(
        fx.ctx
            .store
            .get(&task.id)
            .unwrap()
            .attention
            .source_notices
            .len(),
        1
    );
    drop(registration);
    drain(&fx.reopen()).unwrap();
    // Then settling that exact creator lease permits removal, regardless of worker liveness.
    assert!(
        fx.ctx
            .store
            .get(&task.id)
            .unwrap()
            .attention
            .source_notices
            .is_empty()
    );
    assert_eq!(fx.calls(), 0);
}
