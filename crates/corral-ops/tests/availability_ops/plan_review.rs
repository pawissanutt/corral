use super::*;

const PLAN: &str = "---\ngoal: Make the widget spin\nnon_goals: []\ndecisions: []\naffected_paths: [widget.rs]\ncontracts: []\ndependencies: []\nacceptance:\n  - scenario: widget --spin\n    expected: clockwise\nverification: [cargo test]\nowner_decisions: []\n---\nImplementation details.\n";

/// A plan-first task reviewed by Design Review, whose preferred model may be unavailable.
fn design_review(enabled: Vec<String>) -> Fx {
    let fx = Fx::new(Setup {
        enabled,
        dispatched: false,
        configure: Box::new(|store| {
            store.config.profiles.insert(
                "design-review".into(),
                profile(
                    vec![
                        candidate("design-sol", "openai", "owner-openai", "high"),
                        candidate("design-opus", "anthropic", "owner-anthropic", "low"),
                    ],
                    ProfileRole::PlanReview,
                ),
            );
            store.config.review.reviewer = Some("design-review".into());
            store.config.review.plan_required = Some(true);
        }),
        ..Setup::default()
    });
    let path = fx.ctx.store.root().join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    corral_ops::plan_submit(&fx.ctx, &fx.id, &path).unwrap();
    fx
}

#[test]
fn design_review_opens_on_the_next_approved_candidate() {
    // Given Design Review preferring a model that is not enabled locally.
    let fx = design_review(vec![sol().model, "anthropic/design-opus-fixture".into()]);
    // When the plan review session opens.
    corral_ops::plan_review(&fx.ctx, &fx.id, None).unwrap();
    // Then it launches the approved alternative of the same profile.
    let launched = fx.launched();
    assert!(
        launched.contains("anthropic/design-opus-fixture"),
        "{launched}"
    );
    assert!(
        !launched.contains("openai/design-sol-fixture"),
        "{launched}"
    );
    let switched = fx.events("availability_switched");
    assert_eq!(switched.len(), 1);
    assert_eq!(switched[0]["role"], "plan_review");
    assert_eq!(switched[0]["profile"], "design-review");
}

#[test]
fn design_review_with_every_candidate_unavailable_does_not_open() {
    // Given no approved Design Review candidate enabled locally.
    let fx = design_review(vec![sol().model]);
    // When the plan review is requested.
    let error = format!(
        "{:#}",
        corral_ops::plan_review(&fx.ctx, &fx.id, None).unwrap_err()
    );
    // Then the attempted candidates are reported and no session launches.
    assert!(error.contains("cannot start"), "{error}");
    assert!(error.contains("design-opus"), "{error}");
    assert_eq!(fx.launched(), "");
}

/// A recorded planning run on `vendor`, so plan independence is judged against it.
fn record_planner(fx: &Fx, vendor: &str) {
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.plan_runs.push(corral_core::PlanRun {
                execution: None,
                route: None,
                delivery: Default::default(),
                inbox: Default::default(),
                id: "D1".into(),
                profile: "planner".into(),
                harness: "omo".into(),
                model: Some(format!("{vendor}/planner-fixture")),
                vendor: Some(vendor.into()),
                effort: None,
                spec_revision: corral_ops::plan_spec_revision(task)?,
                started: fx.at(0),
                ended: None,
                herdr: None,
                output_dir: fx.ctx.store.root().join("plan-output"),
                submitted: None,
                rounds: 0,
                state: corral_core::PlanRunState::Planning,
                pending_reply: None,
            });
            Ok(())
        })
        .unwrap();
}

/// Cross-family plan review whose preferred Anthropic reviewer is unavailable and whose
/// approved Google alternative is the only reviewer model enabled locally.
fn cross_family_plan(planner: Option<&str>) -> Fx {
    let fx = Fx::new(Setup {
        enabled: vec![sol().model, "google/review-gemini-fixture".into()],
        dispatched: false,
        configure: Box::new(|store| {
            store.config.profiles.insert(
                "review-general".into(),
                profile(
                    vec![
                        opus(),
                        candidate("review-gemini", "google", "owner-google", "high"),
                    ],
                    ProfileRole::PlanReview,
                ),
            );
            store.config.review.reviewer = Some("review-general".into());
            store.config.review.require_cross_family = Some(true);
        }),
        ..Setup::default()
    });
    if planner.is_none() {
        fx.ctx
            .store
            .update(&fx.id, |task| {
                task.profile = Some("general".into());
                Ok(())
            })
            .unwrap();
    }
    if let Some(vendor) = planner {
        record_planner(&fx, vendor);
    }
    let path = fx.ctx.store.root().join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    corral_ops::plan_submit(&fx.ctx, &fx.id, &path).unwrap();
    fx
}

#[test]
fn a_recorded_planner_is_judged_against_the_review_candidate_that_starts() {
    // Given an Anthropic planner and an unavailable Anthropic preferred reviewer.
    let fx = cross_family_plan(Some("anthropic"));
    // When the plan review opens.
    corral_ops::plan_review(&fx.ctx, &fx.id, None).unwrap();
    // Then the independent, approved Google reviewer launches.
    let launched = fx.launched();
    assert!(
        launched.contains("google/review-gemini-fixture"),
        "{launched}"
    );
    assert!(!launched.contains("anthropic/opus-fixture"), "{launched}");
}

#[test]
fn a_manually_submitted_plan_uses_the_policy_author_for_its_fallback() {
    // Given a plan submitted without a planning run, authored by the task's OpenAI route.
    let fx = cross_family_plan(None);
    // When the plan review opens.
    corral_ops::plan_review(&fx.ctx, &fx.id, None).unwrap();
    // Then the fallback is judged against that author and launches.
    let launched = fx.launched();
    assert!(
        launched.contains("google/review-gemini-fixture"),
        "{launched}"
    );
}

#[test]
fn a_same_family_fallback_is_still_refused() {
    // Given an Anthropic planner whose only available approved reviewer is Anthropic too.
    let fx = cross_family_plan(Some("google"));
    // When the plan review is requested.
    let error = format!(
        "{:#}",
        corral_ops::plan_review(&fx.ctx, &fx.id, None).unwrap_err()
    );
    // Then independence is not downgraded to obtain availability.
    assert!(
        error.contains("cannot start") || error.contains("vendor"),
        "{error}"
    );
    assert_eq!(fx.launched(), "");
}

#[test]
fn a_same_family_pm_is_refused_despite_an_unused_independent_fallback() {
    // Given an OpenAI-authored plan and a PM on the same OpenAI model, whose profile also
    // lists an approved, available Anthropic alternative that the PM session does not use.
    let fx = Fx::new(Setup {
        dispatched: false,
        enabled: vec![sol().model, opus().model],
        configure: Box::new(|store| {
            let mut pm = store.config.profiles["general"].clone();
            pm.metadata.as_mut().unwrap().eligibility.roles =
                [ProfileRole::Pm, ProfileRole::PlanReview].into();
            store.config.profiles.insert("coordinator".into(), pm);
            store.config.pm.agent = "coordinator".into();
            store.config.review.reviewer = Some("pm".into());
            store.config.review.require_cross_family = Some(true);
        }),
        ..Setup::default()
    });
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.profile = Some("general".into());
            Ok(())
        })
        .unwrap();
    let path = fx.ctx.store.root().join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    corral_ops::plan_submit(&fx.ctx, &fx.id, &path).unwrap();
    // When the PM is asked to review the plan.
    let result = corral_ops::plan_review(&fx.ctx, &fx.id, None);
    // Then its actual same-family model is refused and nothing launches.
    let error = format!("{:#}", result.unwrap_err());
    assert!(error.contains("vendor"), "{error}");
    assert_eq!(fx.launched(), "");
}

/// A PM whose preferred Anthropic model is unavailable opens on its approved OpenAI
/// fallback, which is also the family of the manually submitted plan's author.
fn pm_fallback() -> Fx {
    let fx = Fx::new(Setup {
        dispatched: false,
        enabled: vec![sol().model],
        configure: Box::new(|store| {
            let mut pm = store.config.profiles["general"].clone();
            let metadata = pm.metadata.as_mut().unwrap();
            metadata.candidates.reverse();
            metadata.eligibility.roles = [ProfileRole::Pm, ProfileRole::PlanReview].into();
            let preferred = metadata.candidates[0].clone();
            pm.model = Some(preferred.model);
            pm.vendor = Some(preferred.provider);
            pm.effort = preferred.effort;
            store.config.profiles.insert("coordinator".into(), pm);
            store.config.pm.agent = "coordinator".into();
            store.config.review.reviewer = Some("pm".into());
            store.config.review.require_cross_family = Some(true);
        }),
        ..Setup::default()
    });
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.profile = Some("general".into());
            Ok(())
        })
        .unwrap();
    let path = fx.ctx.store.root().join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    corral_ops::plan_submit(&fx.ctx, &fx.id, &path).unwrap();
    fx
}

fn refuses_same_family_pm(fx: &Fx, reviewer: Option<&str>) {
    let result = corral_ops::plan_review(&fx.ctx, &fx.id, reviewer);
    if let Ok(review) = &result {
        let verdict = corral_ops::plan_verdict(
            &fx.ctx,
            &fx.id,
            &corral_ops::PlanVerdict {
                plan: &fx.task().plans.last().unwrap().id,
                attempt: Some(&review.id),
                verdict: corral_core::Verdict::Approve,
                notes: "Fixture review completed.",
            },
        );
        assert!(
            !verdict
                .as_ref()
                .is_ok_and(|r| r.verdict == Some(corral_core::Verdict::Approve)),
            "same-family PM approval was accepted"
        );
    }
    assert!(result.is_err(), "same-family PM received the plan review");
}

#[test]
fn a_fresh_pm_fallback_is_judged_by_the_model_it_would_open_on() {
    // Given no PM session yet, and a PM that would open on the author's family.
    let fx = pm_fallback();
    // When the PM is asked to review, then it is refused without borrowing the
    // preferred model's independence.
    refuses_same_family_pm(&fx, None);
}

#[test]
fn a_reused_pm_fallback_is_judged_by_its_recorded_session_model() {
    // Given a PM session already open on its same-family fallback.
    let fx = pm_fallback();
    corral_ops::open_pm(&fx.ctx).unwrap();
    assert!(fx.launched().contains("--model openai/sol-fixture"));
    // When the PM is asked to review, then its recorded model is refused.
    refuses_same_family_pm(&fx, Some("pm"));
}

#[test]
fn a_pm_session_without_a_recorded_model_fails_closed() {
    // Given a live PM session recorded before its execution identity was tracked.
    let fx = pm_fallback();
    fx.write_state("", "pm", &serde_json::json!({"pane_id": "w1:p1"}));
    // When the PM is asked to review under cross-family policy.
    let error = format!(
        "{:#}",
        corral_ops::plan_review(&fx.ctx, &fx.id, Some("pm")).unwrap_err()
    );
    // Then the unknown identity refuses the review.
    assert!(error.contains("model is unknown"), "{error}");
}

const BEDROCK_CLAUDE: &str = "amazon-bedrock/us.anthropic.claude-sonnet-4-5-20250929-v1:0";
const BEDROCK_LLAMA: &str = "amazon-bedrock/us.meta.llama3-3-70b-instruct-v1:0";

/// A live PM session whose recorded execution is `model` on Bedrock, asked to review an
/// Anthropic planner's plan. The PM's configured profile is OpenAI, so only the recorded
/// execution can show the shared family.
fn bedrock_pm(cross_family: bool, model: &str) -> Fx {
    let fx = Fx::new(Setup {
        dispatched: false,
        enabled: vec![sol().model, opus().model],
        configure: Box::new(move |store| {
            let mut pm = store.config.profiles["general"].clone();
            pm.metadata.as_mut().unwrap().eligibility.roles =
                [ProfileRole::Pm, ProfileRole::PlanReview].into();
            store.config.profiles.insert("coordinator".into(), pm);
            store.config.pm.agent = "coordinator".into();
            store.config.review.reviewer = Some("pm".into());
            store.config.review.require_cross_family = Some(cross_family);
        }),
        ..Setup::default()
    });
    record_planner(&fx, "anthropic");
    let path = fx.ctx.store.root().join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    corral_ops::plan_submit(&fx.ctx, &fx.id, &path).unwrap();
    fx.write_state(
        "",
        "pm",
        &serde_json::json!({
            "pane_id": "w1:p1",
            "execution": {"harness": "omo", "model": model, "vendor": "amazon-bedrock"},
        }),
    );
    fx
}

#[test]
fn a_pm_recorded_on_bedrock_claude_is_the_anthropic_family() {
    // Given a live PM whose recorded execution is Bedrock Claude and an Anthropic planner.
    let fx = bedrock_pm(true, BEDROCK_CLAUDE);
    // When the PM is asked to review under cross-family policy, then it is refused.
    let error = format!(
        "{:#}",
        corral_ops::plan_review(&fx.ctx, &fx.id, Some("pm")).unwrap_err()
    );
    assert!(error.contains("cross-family review is required"), "{error}");
    assert!(error.contains("'anthropic'"), "{error}");
    // A PM recorded on Bedrock Llama is another family and receives the plan.
    let fx = bedrock_pm(true, BEDROCK_LLAMA);
    corral_ops::plan_review(&fx.ctx, &fx.id, Some("pm")).unwrap();
    // With the policy off the Bedrock Claude PM receives the plan as before.
    let fx = bedrock_pm(false, BEDROCK_CLAUDE);
    corral_ops::plan_review(&fx.ctx, &fx.id, Some("pm")).unwrap();
}

/// A manually submitted plan, authored by the task's OpenAI route and approved by a Bedrock
/// Claude plan reviewer; an Anthropic implementation profile is available for dispatch.
fn bedrock_approved_plan(cross_family: bool) -> Fx {
    let fx = Fx::new(Setup {
        dispatched: false,
        enabled: vec![sol().model, opus().model, BEDROCK_CLAUDE.into()],
        configure: Box::new(move |store| {
            let reviewer = ExecutionCandidate {
                model: BEDROCK_CLAUDE.into(),
                ..candidate("bedrock-claude", "amazon-bedrock", "owner-aws", "high")
            };
            store.config.profiles.insert(
                "bedrock-claude".into(),
                profile(vec![reviewer], ProfileRole::PlanReview),
            );
            store.config.profiles.insert(
                "claude".into(),
                profile(vec![opus()], ProfileRole::Implementation),
            );
            store.config.review.reviewer = Some("bedrock-claude".into());
            store.config.review.plan_required = Some(true);
            store.config.review.require_cross_family = Some(cross_family);
        }),
        ..Setup::default()
    });
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.profile = Some("general".into());
            Ok(())
        })
        .unwrap();
    let path = fx.ctx.store.root().join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    corral_ops::plan_submit(&fx.ctx, &fx.id, &path).unwrap();
    let digest = corral_core::plan_policy::policy_digest(&fx.ctx.store.config, &fx.task()).unwrap();
    // Seed the launched review attempt at the store seam; plan review launch is covered above.
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.plans.last_mut().unwrap().reviews = vec![corral_core::PlanReview {
                delivery: corral_core::inbox::DeliveryMode::Legacy,
                inbox: Default::default(),
                route: None,
                id: "PR1".into(),
                reviewer: "bedrock-claude".into(),
                policy_digest: digest,
                verdict: None,
                notes: None,
                at: Timestamp::now(),
                herdr: None,
                pending_reply: None,
            }];
            Ok(())
        })
        .unwrap();
    let plan = fx.task().plans.last().unwrap().id.clone();
    for (attempt, notes) in [
        (Some("PR1"), "Independent review passed"),
        (None, "Owner accepts"),
    ] {
        corral_ops::plan_verdict(
            &fx.ctx,
            &fx.id,
            &corral_ops::PlanVerdict {
                plan: &plan,
                attempt,
                verdict: corral_core::Verdict::Approve,
                notes,
            },
        )
        .unwrap();
    }
    let approved = corral_ops::approved_plan(&fx.ctx, &fx.task());
    assert!(approved.as_ref().is_ok_and(Option::is_some), "{approved:?}");
    // The owner returns the accepted plan's task to Ready before dispatching it.
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.status = Status::Ready;
            Ok(())
        })
        .unwrap();
    fx
}

#[test]
fn dispatch_rechecks_a_bedrock_plan_reviewer_against_the_dispatch_route_family() {
    for cross_family in [true, false] {
        // Given an approved plan whose Bedrock Claude reviewer was independent of its author.
        let fx = bedrock_approved_plan(cross_family);
        // When the task is dispatched on an Anthropic implementation profile.
        let result = corral_ops::dispatch(
            &fx.ctx,
            &fx.id,
            &DispatchOpts {
                profile: Some("claude".into()),
                ..DispatchOpts::default()
            },
        );
        if cross_family {
            // Then the shared anthropic family is refused before any worktree or pane.
            let error = format!("{:#}", result.unwrap_err());
            assert!(error.contains("cross-family review is required"), "{error}");
            assert!(error.contains("'anthropic'"), "{error}");
            let task = fx.task();
            assert!(task.runs.is_empty());
            assert_eq!(task.status, Status::Ready);
            assert_eq!(fx.launched(), "");
        } else {
            // With the policy off, the same dispatch starts.
            assert!(matches!(result.unwrap(), DispatchOutcome::Started { .. }));
            assert_eq!(fx.task().runs.len(), 1);
        }
    }
}
