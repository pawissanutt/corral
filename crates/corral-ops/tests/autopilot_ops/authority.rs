use super::*;

fn assist(fx: &mut Fx) {
    fx.ctx.store.config.automation = Default::default();
    fx.ctx.store.save_config().unwrap();
}

#[test]
fn assist_reports_blocked_review_once_and_confirmation_resumes_it() {
    // Given completed work with configured checks but no confirmed grant.
    let mut fx = Fx::new(|review| {
        review.gate = gate(&["true"]);
        review.reviewer = Some("rev".into());
    });
    fx.done();
    assist(&mut fx);
    // When repeated syncs inspect the task before and after owner confirmation.
    fx.sync();
    fx.sync();
    // Then no launch is silently skipped: task readback, PM and events agree.
    let reason = corral_ops::automation_blocker(&fx.ctx, &fx.task())
        .unwrap()
        .unwrap();
    let blocked = fx
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .find(|event| event.kind == "automation_blocked")
        .unwrap();
    assert_eq!(blocked.detail["reason"], reason);
    assert_eq!(fx.events("automation_blocked"), 1);
    assert_eq!(fx.events("gate_started"), 0);
    let action = corral_ops::pm_next(&fx.ctx).unwrap();
    assert_eq!(action.kind, corral_ops::PmActionKind::Owner);
    assert!(action.evidence.iter().any(|evidence| evidence == &reason));
    fx.grant(&[fx.id.clone()]);
    fx.sync();
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
    assert_eq!(fx.events("automation_resumed"), 1);
    assert!(
        corral_ops::automation_blocker(&fx.ctx, &fx.task())
            .unwrap()
            .is_none()
    );
}

#[test]
fn assist_collects_completed_gate_without_launching_another_process() {
    // Given an explicitly started gate that completed before authority was revoked.
    let mut fx = Fx::new(|review| {
        review.gate = gate(&["true"]);
        review.reviewer = Some("rev".into());
    });
    fx.done();
    corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
    assist(&mut fx);
    // When sync processes the completed receipt in Assist mode.
    fx.sync();
    // Then the result is retained, but an independent reviewer requires consent.
    assert_eq!(fx.run().gate.unwrap().result, GateResult::Pass);
    assert_eq!(fx.events("gate"), 1);
    assert_eq!(fx.events("review_started"), 0);
    assert_eq!(fx.events("automation_blocked"), 1);
}

#[test]
fn assist_retains_independent_reviewer_verdict_without_landing() {
    // Given an independent reviewer already launched under a confirmed policy.
    let mut fx = Fx::new(|review| {
        review.gate = gate(&["true"]);
        review.reviewer = Some("rev".into());
        review.accept = Some(Accept::Auto);
    });
    fx.done();
    fx.sync();
    assist(&mut fx);
    // When the launched reviewer records its verdict after policy revocation.
    fx.verdict(Verdict::Approve, "Candidate approved.");
    fx.sync();
    // Then Assist records evidence without treating the old grant as landing authority.
    assert_eq!(fx.run().review.unwrap().verdict, Some(Verdict::Approve));
    assert_eq!(fx.task().status, Status::Review);
    assert_eq!(fx.events("automation_blocked"), 1);
    assert_eq!(fx.events("approved"), 0);
}

#[test]
fn task_outside_confirmed_scope_has_an_explicit_blocker() {
    // Given a valid grant for another task rather than the completed candidate.
    let mut fx = Fx::new(|review| {
        review.gate = gate(&["true"]);
    });
    fx.done();
    let other = fx
        .ctx
        .store
        .create("Other work", Some("app"), Status::Review, "")
        .unwrap();
    fx.grant(&[other.id]);
    // When the next sync considers the out-of-scope candidate.
    fx.sync();
    // Then it records a concrete refusal, not an empty reason or an error loop.
    let blocker = corral_ops::automation_blocker(&fx.ctx, &fx.task())
        .unwrap()
        .unwrap();
    assert!(!blocker.ends_with(": "));
    assert_eq!(fx.events("automation_blocked"), 1);
    assert_eq!(fx.events("autopilot_error"), 0);
    assert_eq!(fx.events("gate_started"), 0);
}
