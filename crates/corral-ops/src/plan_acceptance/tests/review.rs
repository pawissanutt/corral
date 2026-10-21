use super::*;

const GATE: [&str; 2] = ["test -f first.txt", "test -f second.txt"];

/// A second passing C1 observation under a new key: valid evidence, but different bytes.
fn rerun_c1(f: &Fixture, attempt: &str) {
    let observation: EvidenceObservation = serde_json::from_value(json!({
        "version":1, "key":format!("{attempt}-C1-rerun"), "criterion":"C1", "run":"aggregate",
        "attempt":attempt, "status":"pass", "action":"rerun the combined scenario",
        "environment":"fixture", "expected":"pass", "observed":"observed again",
        "artifacts":[], "captured":Timestamp::now(), "producer":"test", "live":false,
    }))
    .unwrap();
    plan_evidence(&f.ctx, &f.plan, &observation).unwrap();
}

fn set_reviewer(f: &Fixture, reviewer: &str) {
    let mut store = f.ctx.store.reopen().unwrap();
    store.config.agents.insert(
        "rev2".into(),
        AgentCmd {
            command: "true".into(),
        },
    );
    store.config.review.reviewer = Some(reviewer.into());
    store.save_config().unwrap();
}

#[test]
fn owner_acceptance_requires_the_configured_aggregate_review() {
    // Given a configured reviewer, a passing combined gate and passing aggregate scenarios.
    let f = Fixture::new(Delivery::Integration, &GATE);
    let main = f.main();
    let attempt = f.passing();
    // When no aggregate review exists, the owner cannot accept.
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));
    assert!(
        error.contains("aggregate review missing or pending"),
        "{error}"
    );
    // When the reviewer was launched but has not called back, the owner cannot accept.
    let first = f.launch(&attempt.id).unwrap();
    assert!(first.pending() && first.herdr.is_some(), "{first:?}");
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));
    assert!(
        error.contains("aggregate review missing or pending"),
        "{error}"
    );
    // When evidence changes after the review completed, its receipt is stale.
    f.verdict(&attempt.id, &first.id, Verdict::Approve, "looks right")
        .unwrap();
    rerun_c1(&f, &attempt.id);
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));
    assert!(error.contains("aggregate evidence changed"), "{error}");
    f.assert_not_completed(&main);
    // When a fresh review completes with changes, the owner may still override its outcome.
    let second = f.review(&attempt.id, Verdict::Changes).unwrap();
    assert_eq!(second.id, format!("{}-R2", attempt.id));
    let acceptance = plan_accept(&f.ctx, &f.plan, &attempt.id, false).unwrap();
    assert!(!acceptance.automatic && acceptance.by == "owner");
    assert_eq!(f.plan().status, Status::Done);
}

fn map_auto(f: &Fixture, profile: &str) {
    let mut store = f.ctx.store.reopen().unwrap();
    store.config.review.reviewer = Some("auto".into());
    store.config.agents.insert(
        "rev2".into(),
        AgentCmd {
            command: "true".into(),
        },
    );
    for kind in [
        corral_core::ReviewContractKind::Implementation,
        corral_core::ReviewContractKind::Plan,
    ] {
        store
            .config
            .routing
            .review_profiles
            .insert(kind, profile.into());
    }
    store.config.validate_review().unwrap();
    store.save_config().unwrap();
}

#[test]
fn auto_policy_launches_its_mapped_reviewer_through_callback_and_acceptance() {
    // Given `reviewer = "auto"` mapping the review contract to a configured profile.
    let f = Fixture::new(Delivery::Integration, &GATE);
    map_auto(&f, "rev");
    let main = f.main();
    let attempt = f.passing();
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));
    assert!(
        error.contains("aggregate review missing or pending"),
        "{error}"
    );
    // When the aggregate reviewer launches, it is the contract mapping's profile.
    let review = f.launch(&attempt.id).unwrap();
    assert_eq!(review.reviewer, "rev");
    assert_eq!(review.route.profile_source, "auto_mapping");
    assert!(review.route.mapping.is_some(), "{:?}", review.route);
    f.verdict(
        &attempt.id,
        &review.id,
        Verdict::Approve,
        "combined scenario holds",
    )
    .unwrap();
    // Then a changed mapping invalidates the completed review for acceptance.
    map_auto(&f, "rev2");
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));
    assert!(
        error.contains("not the configured reviewer rev2; review again"),
        "{error}"
    );
    f.assert_not_completed(&main);
    // And with the original mapping restored, the same review is current again.
    map_auto(&f, "rev");
    let acceptance = plan_accept(&f.ctx, &f.plan, &attempt.id, false).unwrap();
    assert!(!acceptance.automatic && acceptance.by == "owner");
    assert_eq!(f.plan().status, Status::Done);
}

#[test]
fn changed_auto_mapping_refuses_the_pending_review_callback() {
    // Given a launched auto-mapped review that has not called back yet.
    let f = Fixture::new(Delivery::Integration, &GATE);
    map_auto(&f, "rev");
    let attempt = f.passing();
    let review = f.launch(&attempt.id).unwrap();
    // When the mapping selects another reviewer, the old callback no longer counts.
    map_auto(&f, "rev2");
    let error = message(f.verdict(&attempt.id, &review.id, Verdict::Approve, "notes"));
    assert!(error.contains("stale aggregate verdict"), "{error}");
    assert!(error.contains("review again"), "{error}");
    // Then a fresh launch selects the newly mapped profile.
    let fresh = f.launch(&attempt.id).unwrap();
    assert_eq!(
        (fresh.reviewer.as_str(), fresh.route.profile_source.as_str()),
        ("rev2", "auto_mapping")
    );
}

#[test]
fn changed_configured_reviewer_invalidates_the_completed_review() {
    // Given an approved aggregate review by the configured reviewer.
    let f = Fixture::new(Delivery::Integration, &GATE);
    let main = f.main();
    let attempt = f.reviewed();
    // When the configured reviewer changes, the old receipt no longer proves the policy.
    set_reviewer(&f, "rev2");
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));
    assert!(error.contains("stale Plan proof"), "{error}");
    f.assert_not_completed(&main);
}

#[test]
fn owner_acceptance_without_a_configured_reviewer_needs_no_review() {
    // Given review turned off: owner-controlled acceptance keeps today's manual default.
    let f = Fixture::new(Delivery::Main, &GATE);
    set_reviewer(&f, "none");
    f.grant(OperatingAuthority {
        aggregate_accept: true,
        ..Default::default()
    });
    let attempt = f.passing();
    // Then nothing can be launched, and automatic acceptance has no independent review.
    let error = message(f.launch(&attempt.id));
    assert!(
        error.contains("no aggregate reviewer is configured"),
        "{error}"
    );
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, true));
    assert!(
        error.contains("independent approving aggregate review"),
        "{error}"
    );
    // While the owner can accept the verified candidate.
    plan_accept(&f.ctx, &f.plan, &attempt.id, false).unwrap();
    assert_eq!(f.plan().status, Status::Done);
}

#[test]
fn aggregate_verdicts_bind_only_to_the_launched_pending_review() {
    let f = Fixture::new(Delivery::Integration, &GATE);
    let main = f.main();
    let attempt = f.passing();
    // Unstarted: no review was launched, so no review ID can be completed.
    let error = message(f.verdict(&attempt.id, "A1-R1", Verdict::Approve, "notes"));
    assert!(error.contains("not the current review"), "{error}");
    // Empty notes are refused, as for every other verdict path.
    let first = f.launch(&attempt.id).unwrap();
    let error = message(f.verdict(&attempt.id, &first.id, Verdict::Approve, " \n"));
    assert!(error.contains("nonempty notes"), "{error}");
    // Replaced: a new launch supersedes the pending one and refuses its callback.
    let second = f.launch(&attempt.id).unwrap();
    let error = message(f.verdict(&attempt.id, &first.id, Verdict::Approve, "notes"));
    assert!(error.contains("not the current review"), "{error}");
    let superseded = &f.plan().aggregate_attempts[0].reviews[0];
    assert_eq!(
        superseded.abandoned.as_deref(),
        Some(format!("superseded by {}", second.id).as_str())
    );
    // Wrong attempt: a callback naming another attempt is refused.
    let error = message(f.verdict("A9", &second.id, Verdict::Approve, "notes"));
    assert!(
        error.contains("not the current aggregate attempt"),
        "{error}"
    );
    // Duplicate: the first callback wins, later ones are refused.
    f.verdict(&attempt.id, &second.id, Verdict::Approve, "notes")
        .unwrap();
    let error = message(f.verdict(&attempt.id, &second.id, Verdict::Changes, "notes"));
    assert!(error.contains("already has a verdict"), "{error}");
    // Stale: evidence recorded after launch invalidates the pending review.
    let third = f.launch(&attempt.id).unwrap();
    rerun_c1(&f, &attempt.id);
    let error = message(f.verdict(&attempt.id, &third.id, Verdict::Approve, "notes"));
    assert!(error.contains("aggregate evidence changed"), "{error}");
    // Wrong reviewer: a review launched for another configured reviewer cannot complete.
    let fourth = f.launch(&attempt.id).unwrap();
    assert_eq!(fourth.reviewer, "rev");
    set_reviewer(&f, "rev2");
    let error = message(f.verdict(&attempt.id, &fourth.id, Verdict::Approve, "notes"));
    assert!(error.contains("stale aggregate verdict"), "{error}");
    f.assert_not_completed(&main);
}

#[test]
fn unlaunched_reviewer_cannot_qualify_automatic_acceptance() {
    // Given an explicit aggregate grant and a configured reviewer that never calls back.
    let f = Fixture::new(Delivery::Integration, &GATE);
    let mut store = f.ctx.store.reopen().unwrap();
    store.config.agents.get_mut("rev").unwrap().command = "false".into();
    store.save_config().unwrap();
    f.grant(OperatingAuthority {
        aggregate_accept: true,
        ..Default::default()
    });
    let main = f.main();
    let attempt = f.passing();
    // When a caller tries to complete review without launching the reviewer.
    let error = message(f.verdict(&attempt.id, "A1-R1", Verdict::Approve, ""));
    assert!(error.contains("nonempty notes"), "{error}");
    let error = message(f.verdict(&attempt.id, "A1-R1", Verdict::Approve, "approve"));
    assert!(error.contains("not the current review"), "{error}");
    // And when a launch fails before the reviewer starts.
    let mut broken = f.ctx.clone();
    broken.herdr = corral_herdr::Herdr::with_bin(
        f._dir.path().join("missing-herdr"),
        Some("corral-test".into()),
    );
    assert!(plan_aggregate_review(&broken, &f.plan, &attempt.id).is_err());
    let failed = &f.plan().aggregate_attempts[0].reviews[0];
    assert!(
        failed
            .abandoned
            .as_deref()
            .is_some_and(|reason| reason.starts_with("launch failed")),
        "{failed:?}"
    );
    let error = message(f.verdict(&attempt.id, &failed.id, Verdict::Approve, "approve"));
    assert!(error.contains("launch failed"), "{error}");
    // Then neither automatic nor owner acceptance treats that as a review.
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, true));
    assert!(
        error.contains("independent approving aggregate review"),
        "{error}"
    );
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));
    assert!(
        error.contains("aggregate review missing or pending"),
        "{error}"
    );
    f.assert_not_completed(&main);
}
