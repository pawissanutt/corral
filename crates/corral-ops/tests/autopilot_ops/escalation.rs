use super::*;
use corral_core::{Harness, Profile, ProfileChoice};

#[path = "escalation/queued_retry.rs"]
mod queued_retry;

fn configured(threshold: u32) -> Fx {
    let mut fx = Fx::new(|p| {
        p.gate = gate(&["false"]);
        p.max_rounds = Some(10);
    });
    fx.ctx.store.config.routing.escalate_after = threshold;
    fx.ctx.store.config.harnesses.insert(
        "fake".into(),
        Harness {
            command: "true {model_args}".into(),
            flags: Some("omo".into()),
            model_flag: None,
            effort_flag: None,
            efforts: vec![],
            prompt: None,
        },
    );
    for (name, vendor, target, effort) in [
        ("fake", "anthropic", Some("strong"), "low"),
        ("strong", "openai", Some("strongest"), "high"),
        ("strongest", "anthropic", None, "max"),
    ] {
        fx.ctx.store.config.profiles.insert(
            name.into(),
            Profile {
                harness: "fake".into(),
                vendor: Some(vendor.into()),
                escalate_to: target.map(str::to_string),
                model: None,
                effort: Some(effort.into()),
                args: vec![],
                metadata: None,
            },
        );
    }
    fx.ctx.store.config.routing.reviewer_by_vendor = [
        ("openai".into(), "strong".into()),
        ("anthropic".into(), "strongest".into()),
    ]
    .into();
    fx.ctx.store.save_config().unwrap();
    fx.ctx
        .store
        .update(&fx.id, |t| {
            let run = t.current_run_mut().unwrap();
            run.vendor = Some("anthropic".into());
            run.effort = Some("low".into());
            // This fixture models an automatic choice, not Fx's explicit launch override.
            run.route = Some(corral_core::RouteSource::Defaults);
            Ok(())
        })
        .unwrap();
    fx
}

fn reject_gate(fx: &Fx) {
    fx.done();
    corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
    let result = corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap().unwrap();
    assert_eq!(result.result, GateResult::Fail);
}

#[test]
fn escalation_chain_waits_for_n_failures_on_each_profile() {
    // Given a three-profile chain with two failures required at each step.
    let fx = configured(2);
    for expected in ["fake", "strong", "strong", "strongest", "strongest"] {
        reject_gate(&fx);
        // When retrying the failed round.
        corral_ops::retry(&fx.ctx, &fx.id, &ProfileChoice::default()).unwrap();
        // Then each link gets its own threshold and the terminal profile stays selected.
        assert_eq!(fx.run().agent, expected);
    }
    assert_eq!(fx.events("profile_switched"), 2);
    assert_eq!(fx.run().effort.as_deref(), Some("max"));
}

#[test]
fn disabled_escalation_keeps_profile_after_failed_rounds() {
    // Given automatic escalation is disabled despite a configured target.
    let fx = configured(0);
    reject_gate(&fx);
    // When retrying.
    corral_ops::retry(&fx.ctx, &fx.id, &ProfileChoice::default()).unwrap();
    // Then the profile and effort are unchanged.
    assert_eq!(fx.run().agent, "fake");
    assert_eq!(fx.run().effort.as_deref(), Some("low"));
    assert_eq!(fx.events("profile_switched"), 0);
}

#[test]
fn task_profile_pin_prevents_automatic_reasoning_escalation() {
    // Given a pinned profile that has reached its configured reasoning threshold.
    let fx = configured(1);
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.profile = Some("fake".into());
            Ok(())
        })
        .unwrap();
    reject_gate(&fx);
    // When retrying without explicit owner redirection.
    corral_ops::retry(&fx.ctx, &fx.id, &ProfileChoice::default()).unwrap();
    // Then the pin survives and no stronger profile is silently selected.
    assert_eq!(fx.run().agent, "fake");
    assert_eq!(fx.events("profile_switched"), 0);
    assert_eq!(fx.task().profile.as_deref(), Some("fake"));
}

#[test]
fn dispatch_override_prevents_automatic_reasoning_escalation() {
    // Given an explicit dispatch override, rather than the automatic fixture route.
    let fx = configured(1);
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.current_run_mut().unwrap().route = Some(corral_core::RouteSource::Override);
            Ok(())
        })
        .unwrap();
    reject_gate(&fx);
    // When requesting a retry.
    corral_ops::retry(&fx.ctx, &fx.id, &ProfileChoice::default()).unwrap();
    // Then a manual execution choice is not changed by the automatic threshold.
    assert_eq!(fx.run().agent, "fake");
    assert_eq!(fx.events("profile_switched"), 0);
}

#[test]
fn explicit_retry_model_does_not_trigger_an_automatic_profile_switch() {
    // Given a reasoning threshold and a new explicit execution choice.
    let fx = configured(1);
    reject_gate(&fx);
    // When the owner selects an exact model without redirecting the profile.
    corral_ops::retry(
        &fx.ctx,
        &fx.id,
        &ProfileChoice {
            model: Some("anthropic/explicit".into()),
            ..Default::default()
        },
    )
    .unwrap();
    // Then automatic difficulty policy cannot overrule that execution choice.
    assert_eq!(fx.run().agent, "fake");
    assert_eq!(fx.run().model.as_deref(), Some("anthropic/explicit"));
    assert_eq!(fx.events("profile_switched"), 0);
}

#[test]
fn manual_only_owner_rule_remains_eligible_through_same_profile_retries() {
    // Given an owner-rule run whose profile is deliberately excluded from semantic Auto.
    let mut fx = configured(0);
    fx.ctx.store.config.profiles.get_mut("fake").unwrap().metadata = Some(
        serde_json::from_value(serde_json::json!({
            "display_name":"Owner worker", "purpose":"Owner selected implementation", "revision":"r1",
            "guidance":{}, "instructions":"",
            "eligibility":{"enabled":true,"automatic":false,"roles":["implementation"]}
        })).unwrap(),
    );
    let source = corral_core::RouteSource::Rule {
        index: 1,
        name: Some("Owner choice".into()),
    };
    fx.ctx
        .store
        .config
        .routing
        .rules
        .push(corral_core::RoutingRule {
            name: Some("Owner choice".into()),
            project: vec![],
            kind: vec![],
            size: vec![],
            title_regex: None,
            body_regex: None,
            profile: "fake".into(),
        });
    fx.ctx.store.save_config().unwrap();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.current_run_mut().unwrap().route = Some(source.clone());
            Ok(())
        })
        .unwrap();
    assert_eq!(
        corral_ops::route(&fx.ctx, &fx.id, &ProfileChoice::default())
            .unwrap()
            .implementer
            .route,
        source
    );
    for _ in 0..2 {
        fx.done();
        let previous = fx.run().route.unwrap();
        // When retrying without changing the owner-rule profile or escalating.
        corral_ops::retry(&fx.ctx, &fx.id, &ProfileChoice::default()).unwrap();
        // Then unchanged and recursively wrapped rule provenance keeps its original eligibility.
        assert_eq!(fx.run().agent, "fake");
        assert_eq!(
            fx.run().route,
            Some(corral_core::RouteSource::Retry {
                previous: Box::new(previous),
                switch: None,
            })
        );
        assert_eq!(fx.events("profile_switched"), 0);
        assert!(fx.task().profile.is_none());
    }
}

#[test]
fn ineligible_escalation_target_cannot_supersede_the_current_run() {
    // Given failed work and an owner-disabled stronger profile.
    let mut fx = configured(1);
    fx.ctx
        .store
        .config
        .profiles
        .get_mut("strong")
        .unwrap()
        .metadata = Some(
        serde_json::from_value(serde_json::json!({
            "display_name":"Strong", "purpose":"Difficult implementation", "revision":"r1",
            "guidance":{}, "instructions":"Follow the implementation contract",
            "eligibility":{"enabled":false,"automatic":true,"roles":["implementation"]}
        }))
        .unwrap(),
    );
    fx.ctx.store.save_config().unwrap();
    reject_gate(&fx);
    let before = fx.task();
    // When automatic difficulty routing attempts the configured transition.
    let result = corral_ops::retry(&fx.ctx, &fx.id, &ProfileChoice::default());
    // Then rejection precedes pane closure and run supersession.
    assert!(result.is_err());
    assert_eq!(fx.task(), before);
}

#[test]
fn provider_interruption_is_not_a_reasoning_failure() {
    // Given a provider-interrupted run with a configured stronger profile.
    let fx = configured(1);
    fx.done();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.current_run_mut().unwrap().outcome = Some(RunOutcome::Interrupted);
            Ok(())
        })
        .unwrap();
    // When starting a fresh attempt after availability recovery.
    corral_ops::retry(&fx.ctx, &fx.id, &ProfileChoice::default()).unwrap();
    // Then provider access failure spends no reasoning-failure allowance.
    assert_eq!(fx.run().agent, "fake");
    assert_eq!(fx.task().runs[0].failures, 0);
    assert_eq!(fx.events("profile_switched"), 0);
}

#[test]
fn owner_rule_does_not_exempt_automatic_switch_targets_from_auto_eligibility() {
    // Given an owner-rule run at its threshold and a manual-only stronger target.
    let mut fx = configured(1);
    fx.ctx.store.config.profiles.get_mut("strong").unwrap().metadata = Some(
        serde_json::from_value(serde_json::json!({
            "display_name":"Manual target", "purpose":"Owner selected implementation", "revision":"r1",
            "guidance":{}, "instructions":"",
            "eligibility":{"enabled":true,"automatic":false,"roles":["implementation"]}
        })).unwrap(),
    );
    fx.ctx.store.save_config().unwrap();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.current_run_mut().unwrap().route = Some(corral_core::RouteSource::Rule {
                index: 1,
                name: None,
            });
            Ok(())
        })
        .unwrap();
    reject_gate(&fx);
    let before = fx.task();
    // When difficulty policy tries to switch away from the owner-rule profile.
    let result = corral_ops::retry(&fx.ctx, &fx.id, &ProfileChoice::default());
    // Then Auto-disabled targets still reject before supersession.
    assert!(result.is_err());
    assert_eq!(fx.task(), before);
}

#[test]
fn stale_auto_retry_requires_current_selection_before_superseding_run() {
    // Given an Auto run followed by a changed task contract.
    let fx = configured(1);
    let select = |key: &str, id: &str| {
        let task = fx.task();
        corral_core::AutoProfileSelection {
            version: 1,
            id: id.into(),
            task: task.id.clone(),
            actor: corral_core::SelectionActor::Pm,
            selected_at: now(),
            role: corral_core::ProfileRole::Implementation,
            profile: key.into(),
            profile_revision: "legacy".into(),
            spec_revision: corral_core::plan_contract_revision(&task).unwrap(),
            catalog_revision: fx.ctx.store.config.profile_catalog_revision().unwrap(),
            reason: "Current task uncertainty warrants this configuration".into(),
            closest_alternative: None,
            distinction: None,
            review: None,
        }
    };
    let original = select("fake", "initial");
    fx.ctx
        .store
        .save_auto_profile_selection(&original, None)
        .unwrap();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.current_run_mut().unwrap().route = Some(corral_core::RouteSource::Auto {
                selection: Box::new(original.clone()),
            });
            Ok(())
        })
        .unwrap();
    fx.done();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.body.push_str("\nChanged acceptance");
            Ok(())
        })
        .unwrap();
    let before = fx.task();
    // When retrying before and after an explicit current reclassification.
    let stale = corral_ops::retry(&fx.ctx, &fx.id, &ProfileChoice::default());
    assert!(stale.is_err());
    assert_eq!(fx.task(), before);
    let revised = select("strongest", "reclassified");
    fx.ctx
        .store
        .save_auto_profile_selection(&revised, None)
        .unwrap();
    // The existing recovery contract independently requires a current handoff.
    let identity = corral_ops::progress_readback(&fx.ctx, &fx.id).unwrap();
    corral_ops::save_checkpoint(
        &fx.ctx,
        &fx.id,
        &corral_core::CheckpointUpdate {
            task: identity.task,
            run: identity.run,
            spec_revision: identity.spec_revision,
            expected_revision: identity.checkpoints.last().unwrap().checkpoint.revision,
            progress_revision: identity
                .progress
                .as_ref()
                .map_or(0, |progress| progress.revision),
            head: identity.head,
            base: identity.base,
            tree: identity.tree,
            criterion_ids: vec![],
            evidence_refs: vec![],
            next_action: "Retry using the current selection".into(),
            decisions: vec![],
            waits: vec![],
            resources: vec![],
        },
    )
    .unwrap();
    corral_ops::retry(&fx.ctx, &fx.id, &ProfileChoice::default()).unwrap();
    // Then the replacement uses current Auto provenance, not an implicit manual pin.
    assert_eq!(fx.run().agent, "strongest");
    let run = fx.run();
    assert_eq!(run.route.as_ref().unwrap().auto_selection(), Some(&revised));
    assert!(!run.route.unwrap().is_manual());
    assert!(fx.task().profile.is_none());
}

#[test]
fn automatic_escalation_does_not_bypass_exhausted_repair_budget() {
    // Given failed automatic work whose repair allowance is exhausted.
    let mut fx = configured(1);
    fx.ctx.store.config.review.max_rounds = Some(0);
    fx.ctx.store.save_config().unwrap();
    fx.done();
    // When autopilot processes the failed gate.
    fx.sync();
    // Then the task waits for its owner without starting a stronger run.
    assert_eq!(fx.task().status, Status::NeedsMe);
    assert_eq!(fx.run().id, "R1");
    assert_eq!(fx.run().agent, "fake");
    assert_eq!(fx.events("profile_switched"), 0);
}

#[test]
fn successful_retry_does_not_escalate() {
    // Given a finished run with no rejected round.
    let fx = configured(1);
    fx.done();
    // When requesting a fresh retry.
    corral_ops::retry(&fx.ctx, &fx.id, &ProfileChoice::default()).unwrap();
    // Then a retry alone is not a failure.
    assert_eq!(fx.run().agent, "fake");
}

#[test]
fn manual_changes_keep_live_session_even_at_threshold() {
    // Given a rejected run with a live pane and an available escalation.
    let fx = configured(1);
    reject_gate(&fx);
    let previous = fx.run();
    // When the owner requests ordinary changes.
    corral_ops::request_changes(&fx.ctx, &fx.id, "Fix the edge case").unwrap();
    // Then the same run, worktree, pane and profile continue.
    let run = fx.run();
    assert_eq!(
        (run.id, run.worktree, run.herdr, run.agent),
        (
            previous.id,
            previous.worktree,
            previous.herdr,
            previous.agent
        )
    );
    assert_eq!(fx.task().status, Status::Running);
}

#[test]
fn manual_escalation_starts_fresh_even_when_automatic_escalation_is_disabled() {
    // Given a reviewed change and an explicit escalation target.
    let fx = configured(0);
    fx.change("widget.txt", "before\n");
    fx.done();
    let previous = fx.run();
    // When the owner explicitly escalates.
    corral_ops::request_changes_escalated(&fx.ctx, &fx.id, "Handle empty widgets").unwrap();
    // Then a fresh run uses the target defaults, keeps the request and records the switch.
    let run = fx.run();
    assert_eq!(run.id, "R2");
    assert_eq!(run.agent, "strong");
    assert_eq!(run.effort.as_deref(), Some("high"));
    assert_ne!(run.worktree, previous.worktree);
    assert_eq!(
        run.autopilot.unwrap().reason.as_deref(),
        Some("Handle empty widgets")
    );
    assert_eq!(fx.task().runs[0].outcome, Some(RunOutcome::Superseded));
    assert_eq!(run.switches[0].from, "fake");
    assert_eq!(run.switches[0].to, "strong");
    assert_eq!(fx.events("profile_switched"), 1);
}

#[test]
fn missing_escalation_target_leaves_existing_run_intact() {
    // Given a bad target that has not passed config validation.
    let mut fx = configured(1);
    fx.ctx
        .store
        .config
        .profiles
        .get_mut("fake")
        .unwrap()
        .escalate_to = Some("missing".into());
    fx.done();
    let before = fx.task();
    // When explicitly escalating.
    let result = corral_ops::request_changes_escalated(&fx.ctx, &fx.id, "Fix it");
    // Then resolution fails before the live run is superseded.
    assert!(result.is_err());
    assert_eq!(fx.task(), before);
}

#[test]
fn autopilot_gate_failure_escalates_instead_of_prompting_live_agent() {
    // Given a live implementing pane and a failing gate.
    let fx = configured(1);
    fx.done();
    // When autopilot processes the result.
    fx.sync();
    // Then it starts a fresh escalated run rather than prompting the old pane.
    assert_eq!(fx.run().agent, "strong");
    assert_eq!(fx.run().id, "R2");
    assert_eq!(fx.task().rounds, 1);
    assert!(fx.herdr_log("prompts.log").is_empty());
}

#[test]
fn autopilot_review_changes_escalate_to_next_profile() {
    // Given a reviewer requesting changes after a passing gate.
    let mut fx = configured(1);
    fx.ctx.store.config.review.gate = gate(&["true"]);
    fx.ctx.store.config.review.reviewer = Some("cross-vendor".into());
    fx.ctx.store.save_config().unwrap();
    fx.done();
    fx.sync();
    fx.verdict(Verdict::Changes, "Handle empty widgets");
    // When autopilot processes the verdict.
    fx.sync();
    // Then it selects the next profile and persists the failure on the old run.
    assert_eq!(fx.run().agent, "strong");
    assert_eq!(fx.task().runs[0].failures, 1);
    assert_eq!(fx.task().rounds, 1);
}

#[test]
fn cross_vendor_review_uses_run_snapshot_and_records_selection() {
    // Given an anthropic run whose profile has since been edited to openai.
    let mut fx = configured(1);
    fx.ctx.store.config.profiles.get_mut("fake").unwrap().vendor = Some("openai".into());
    fx.done();
    // When selecting cross-vendor review.
    let review = corral_ops::review_run(&fx.ctx, &fx.id, Some("cross-vendor")).unwrap();
    // Then the run snapshot, not the edited profile, chooses the openai reviewer.
    assert_eq!(review.reviewer, "strong");
    assert_eq!(fx.run().switches[0].from, "fake");
    assert_eq!(fx.run().switches[0].to, "strong");
    assert_eq!(fx.events("profile_switched"), 1);
}

#[test]
fn queued_retry_retains_escalated_profile_and_defaults() {
    // Given a failed run with no free slot for its replacement.
    let mut fx = configured(1);
    reject_gate(&fx);
    fx.ctx.store.config.limits.running = 1;
    fx.ctx.store.save_config().unwrap();
    let blocker = fx
        .ctx
        .store
        .create("Occupied slot", Some("app"), Status::NeedsMe, "")
        .unwrap();
    let outcome = corral_ops::retry(&fx.ctx, &fx.id, &ProfileChoice::default()).unwrap();
    assert!(matches!(outcome, DispatchOutcome::Queued { .. }));
    corral_ops::move_task(&fx.ctx, &blocker.id, Status::Dropped, false).unwrap();
    // When capacity also includes the two reserved finishing slots.
    fx.ctx.store.config.limits.running = 3;
    fx.ctx.store.save_config().unwrap();
    fx.sync();
    // Then the queued plan survives default dispatch routing.
    assert_eq!(fx.run().agent, "strong");
    assert_eq!(fx.run().effort.as_deref(), Some("high"));
    assert_eq!(fx.run().switches[0].to, "strong");
}

#[test]
fn collecting_one_failed_gate_twice_counts_one_failure() {
    // Given a recorded failed gate.
    let fx = configured(2);
    reject_gate(&fx);
    // When collecting the same result again.
    corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap();
    // Then it counts once, not once per sync.
    assert_eq!(fx.run().failures, 1);
}

#[test]
fn explicit_retry_profile_overrides_automatic_escalation() {
    // Given a failed run that would ordinarily choose strong.
    let fx = configured(1);
    reject_gate(&fx);
    // When the caller selects another profile.
    corral_ops::retry(
        &fx.ctx,
        &fx.id,
        &ProfileChoice {
            profile: Some("strongest".into()),
            ..Default::default()
        },
    )
    .unwrap();
    // Then the explicit selection wins and is logged.
    assert_eq!(fx.run().agent, "strongest");
    assert_eq!(fx.run().switches[0].to, "strongest");
}

#[test]
fn escalated_retry_uses_target_model_not_previous_override() {
    // Given an earlier model override and a different target model.
    let mut fx = configured(1);
    fx.ctx
        .store
        .config
        .profiles
        .get_mut("strong")
        .unwrap()
        .model = Some("openai/target".into());
    fx.ctx
        .store
        .update(&fx.id, |t| {
            t.current_run_mut().unwrap().model = Some("anthropic/override".into());
            Ok(())
        })
        .unwrap();
    reject_gate(&fx);
    // When retrying without a new model override.
    corral_ops::retry(&fx.ctx, &fx.id, &ProfileChoice::default()).unwrap();
    // Then the escalated profile's model is selected.
    assert_eq!(fx.run().model.as_deref(), Some("openai/target"));
}

#[test]
fn cross_vendor_review_reverses_after_implementation_escalates() {
    // Given an implementation escalated from anthropic to openai.
    let fx = configured(1);
    reject_gate(&fx);
    corral_ops::retry(&fx.ctx, &fx.id, &ProfileChoice::default()).unwrap();
    fx.done();
    // When reviewing the new run.
    let record = corral_ops::review_run(&fx.ctx, &fx.id, Some("cross-vendor")).unwrap();
    // Then its reviewer is anthropic, not the old run's reviewer.
    assert_eq!(record.reviewer, "strongest");
    assert_eq!(fx.run().vendor.as_deref(), Some("openai"));
}

#[test]
fn retry_still_works_when_previous_worktree_was_removed() {
    // Given a rejected run whose old worktree has already been cleaned up.
    let fx = configured(1);
    reject_gate(&fx);
    corral_git::worktree_remove(&fx.app, &fx.run().worktree, false).unwrap();
    // When retrying from the base.
    corral_ops::retry(&fx.ctx, &fx.id, &ProfileChoice::default()).unwrap();
    // Then unavailable old diff context does not prevent fresh work.
    assert_eq!(fx.run().agent, "strong");
    assert!(fx.run().worktree.is_dir());
}
