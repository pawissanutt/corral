use corral_core::{Density, DispatchPlan, Profile, ProfileChoice, RetryPlan, RouteSource, RunMode};

use super::*;
use crate::board::tests::seeded;

pub(crate) fn fixture() -> crate::board::tests::Fixture {
    let mut fx = seeded();
    for (name, vendor) in [("gpt-sol", "openai"), ("sonnet", "anthropic")] {
        fx.ctx.store.config.profiles.insert(
            name.into(),
            Profile {
                harness: "generic".into(),
                vendor: Some(vendor.into()),
                model: None,
                effort: None,
                args: Vec::new(),
                escalate_to: None,
            },
        );
    }
    fx.ctx.store.config.review.reviewer = Some("cross-vendor".into());
    fx.ctx
        .store
        .config
        .routing
        .reviewer_by_vendor
        .insert("anthropic".into(), "sonnet".into());
    fx.ctx
        .store
        .config
        .routing
        .reviewer_by_vendor
        .insert("openai".into(), "gpt-sol".into());
    fx.ctx.store.config.defaults.profile = Some("gpt-sol".into());
    fx.ctx.store.config.board.card.fields = vec!["profile".into()];
    for id in ["T-002", "T-003"] {
        fx.ctx
            .store
            .update(id, |t| {
                t.profile = Some("gpt-sol".into());
                Ok(())
            })
            .unwrap();
    }
    fx
}

#[test]
fn cached_detail_values_match_the_cli_route_service() {
    // Given distinct implementation and reviewer profiles, including planning.
    let fx = fixture();
    let task = fx
        .ctx
        .store
        .update("T-002", |t| {
            t.plan_required = Some(true);
            Ok(())
        })
        .unwrap();
    let expected = serde_json::to_value(
        corral_ops::route(&fx.ctx, &task.id, &ProfileChoice::default()).unwrap(),
    )
    .unwrap();
    let mut cache = RouteCache::default();
    // When the background worker completes the same task snapshot.
    cache.refresh(&fx.ctx, std::slice::from_ref(&task));
    cache.receive();
    // Then every serialized value, including sources, warnings and plan review, agrees.
    assert_eq!(
        serde_json::to_value(cache.get(&task.id).unwrap().next.as_ref().unwrap()).unwrap(),
        expected
    );
}

#[test]
fn unchanged_revisions_do_not_resolve_again_and_config_changes_invalidate() {
    // Given one cached route.
    let mut fx = fixture();
    let tasks = vec![fx.ctx.store.get("T-002").unwrap()];
    let mut cache = RouteCache::default();
    cache.refresh(&fx.ctx, &tasks);
    cache.receive();
    let (tx, requests) = mpsc::channel();
    cache.tx = tx;
    // When refresh sees the same task/config, then no work or result is queued.
    cache.refresh(&fx.ctx, &tasks);
    assert!(matches!(
        requests.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert_eq!(cache.get("T-002").unwrap().card.label, "gpt-sol -> sonnet");
    // When only the config changes, then its revision forces a new route.
    fx.ctx.store.config.review.reviewer = Some("none".into());
    cache.refresh(&fx.ctx, &tasks);
    assert!(cache.get("T-002").is_none());
    let request = requests
        .try_recv()
        .expect("config change queues a new snapshot");
    assert_eq!(
        resolve(&request.ctx, &request.tasks[0].0).card.label,
        "gpt-sol -> none"
    );
}

#[test]
fn stale_task_results_cannot_overwrite_newer_routes() {
    // Given two queued revisions of one task.
    let fx = fixture();
    let mut cache = RouteCache::default();
    let original = fx.ctx.store.get("T-002").unwrap();
    cache.refresh(&fx.ctx, &[original]);
    let changed = corral_ops::reviewer_set(&fx.ctx, "T-002", Some("none")).unwrap();
    cache.refresh(&fx.ctx, std::slice::from_ref(&changed));
    // When both results arrive, regardless of whether refresh consumed the first.
    while cache.get("T-002").is_none() {
        cache.receive();
    }
    cache.poll();
    // Then only the latest task revision is displayed.
    assert_eq!(cache.get("T-002").unwrap().card.label, "gpt-sol -> none");
    assert_eq!(cache.revisions["T-002"].0, changed.updated);
}

#[test]
fn active_cards_keep_recorded_identity_after_profiles_and_rules_change() {
    // Given a running candidate whose profile was deleted and whose vendor was recorded.
    let fx = fixture();
    let task = fx
        .ctx
        .store
        .update("T-004", |t| {
            let run = t.current_run_mut().unwrap();
            run.agent = "retired-implementer".into();
            run.harness = Some("original-harness".into());
            run.vendor = Some("openai".into());
            run.route = Some(RouteSource::Rule {
                index: 2,
                name: Some("old rule".into()),
            });
            t.profile = Some("sonnet".into());
            Ok(())
        })
        .unwrap();
    // When resolving the board label.
    let route = resolve(&fx.ctx, &task);
    // Then the implementation pair uses the launch vendor, not the next dispatch.
    assert_eq!(route.card.label, "retired-implementer -> sonnet");
    let recorded = route.recorded.unwrap().unwrap();
    assert_eq!(recorded.implementer.harness, "original-harness");
    assert_eq!(
        recorded.implementer.route,
        task.current_run().unwrap().route.clone().unwrap()
    );
    assert_eq!(route.next.unwrap().implementer.profile, "sonnet");
}

#[test]
fn plan_required_cards_use_the_independent_plan_reviewer() {
    // Given code review disabled but cross-family plan review explicitly required.
    let mut fx = fixture();
    fx.ctx.store.config.agents.clear();
    fx.ctx
        .store
        .config
        .profiles
        .retain(|name, _| matches!(name.as_str(), "gpt-sol" | "sonnet"));
    let harness = fx.ctx.store.config.harnesses.get_mut("generic").unwrap();
    harness.command = "false {model_args}".into();
    harness.model_flag = Some("--model".into());
    for (name, model) in [
        ("gpt-sol", "openai/planner"),
        ("sonnet", "anthropic/reviewer"),
    ] {
        fx.ctx.store.config.profiles.get_mut(name).unwrap().model = Some(model.into());
    }
    fx.ctx.store.config.review.require_cross_family = Some(true);
    let task = fx
        .ctx
        .store
        .update("T-002", |t| {
            t.plan_required = Some(true);
            t.reviewer = Some("none".into());
            let route = fx
                .ctx
                .store
                .config
                .route_task(t, &ProfileChoice::default(), |_| None)?;
            t.dispatch_plan = Some(DispatchPlan {
                plan: true,
                explicit_plan: false,
                choice: RetryPlan {
                    profile: "gpt-sol".into(),
                    model: None,
                    effort: None,
                    switch: None,
                },
                route,
                base: None,
                mode: RunMode::Interactive,
            });
            Ok(())
        })
        .unwrap();
    // When previewing before plan approval.
    let route = resolve(&fx.ctx, &task);
    // Then the card exposes the plan reviewer rather than 'none' code review.
    assert_eq!(route.card.label, "gpt-sol -> sonnet");
    assert_eq!(route.next.unwrap().review.reviewer.selection, "none");
}

#[test]
fn warning_labels_render_on_backlog_and_ready_in_both_densities() {
    // Given fallback, same-vendor and missing-vendor-map warnings.
    let fx = fixture();
    for (density, compact_cards) in [
        (Density::Comfortable, false),
        (Density::Rich, false),
        (Density::Compact, false),
        (Density::Comfortable, true),
    ] {
        for status in [Status::Backlog, Status::Ready] {
            for (profile, reviewer) in [
                (Some("gpt-sol"), Some("gpt-sol")),
                (None, Some("none")),
                (Some("generic"), Some("cross-vendor")),
            ] {
                let mut task = fx.ctx.store.get("T-002").unwrap();
                task.status = status;
                task.profile = profile.map(str::to_string);
                task.reviewer = reviewer.map(str::to_string);
                let route = resolve(&fx.ctx, &task);
                assert!(route.card.warning);
                for enabled in [false, true] {
                    let mut config = fx.ctx.store.config.clone();
                    config.ui.route_warnings = enabled;
                    config.ui.compact_cards = compact_cards;
                    config.board.card.density = density;
                    // When rendering the actual card presentation.
                    let lines = crate::board::card_lines(
                        &task,
                        std::slice::from_ref(&task),
                        &config,
                        48,
                        false,
                        task.updated,
                        crate::board::CardExtra {
                            route: Some(&route.card),
                            ..Default::default()
                        },
                    );
                    let text = crate::render::to_plain(&lines);
                    // Then only the warning glyph is conditional; identity and pair survive.
                    assert_eq!(text.contains('⚠'), enabled, "{text}");
                    assert!(text.contains(&task.id), "{text}");
                    assert!(text.contains(&route.card.label), "{text}");
                }
            }
        }
    }
}

#[test]
fn implementer_pin_preserves_queued_mode_base_and_planning_intent() {
    // Given a Ready queue route with explicit planning/headless/base choices.
    let fx = fixture();
    let task = fx
        .ctx
        .store
        .update("T-003", |t| {
            t.dispatch_plan = Some(DispatchPlan {
                plan: true,
                explicit_plan: true,
                choice: RetryPlan {
                    profile: "gpt-sol".into(),
                    model: None,
                    effort: None,
                    switch: None,
                },
                route: fx
                    .ctx
                    .store
                    .config
                    .route_task(t, &ProfileChoice::default(), |_| None)?,
                base: Some("topic-base".into()),
                mode: RunMode::Headless,
            });
            Ok(())
        })
        .unwrap();
    // When the persistent pin changes the implementation profile.
    let saved = corral_ops::implementer_set(&fx.ctx, &task.id, "sonnet").unwrap();
    // Then only profile selection changes, preserving admission intent.
    let plan = saved.dispatch_plan.unwrap();
    assert_eq!(saved.profile.as_deref(), Some("sonnet"));
    assert_eq!(plan.route.profile, "sonnet");
    assert_eq!(plan.choice.profile, "sonnet");
    assert_eq!(plan.mode, RunMode::Headless);
    assert_eq!(plan.base.as_deref(), Some("topic-base"));
    assert!(plan.plan && plan.explicit_plan);
}

#[test]
fn narrow_compact_routes_keep_task_identity_and_warning() {
    // Given a long unresolved route on a standard six-column compact board.
    let fx = fixture();
    let task = fx.ctx.store.get("T-003").unwrap();
    let route = CardRoute {
        label: "sonnet -> cross-vendor".into(),
        warning: true,
    };
    let mut config = fx.ctx.store.config.clone();
    config.board.card.density = Density::Compact;
    config.ui.route_warnings = true;
    // When the configured profile field consumes more room than the card has.
    let lines = crate::board::card_lines(
        &task,
        std::slice::from_ref(&task),
        &config,
        22,
        false,
        task.updated,
        crate::board::CardExtra {
            route: Some(&route),
            ..Default::default()
        },
    );
    // Then shortening the pair never removes the card's task id or warning flag.
    let text = crate::render::to_plain(&lines);
    assert!(
        text.contains("003") && text.contains('⚠') && text.contains("sonnet ->"),
        "{text}"
    );
}

#[test]
fn active_planner_pair_survives_a_future_implementer_pin() {
    // Given a live OpenAI planner and a future implementation pin changed to Anthropic.
    let fx = fixture();
    let mut task = fx.ctx.store.get("T-002").unwrap();
    task.status = Status::NeedsMe;
    task.profile = Some("sonnet".into());
    task.plan_required = Some(true);
    task.plan_runs.push(corral_core::PlanRun {
        id: "P1".into(),
        profile: "gpt-sol".into(),
        harness: "generic".into(),
        model: Some("openai/original-planner".into()),
        vendor: Some("openai".into()),
        effort: None,
        spec_revision: "fixture".into(),
        started: task.updated,
        ended: None,
        herdr: None,
        output_dir: fx.ctx.store.root().join("plan-fixture"),
        submitted: None,
        rounds: 0,
        state: corral_core::PlanRunState::Planning,
        pending_reply: None,
    });
    // When the board resolves its planning card.
    let route = resolve(&fx.ctx, &task);
    // Then the recorded planner's vendor still selects its reviewer.
    assert_eq!(route.card.label, "gpt-sol -> sonnet");
    assert_eq!(route.next.unwrap().implementer.profile, "sonnet");
}
