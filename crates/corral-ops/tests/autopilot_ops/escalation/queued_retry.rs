use super::*;
use corral_core::{AutoProfileSelection, ProfileRole, RouteSource, SelectionActor};

fn selection(fx: &Fx, profile: &str, id: &str) -> AutoProfileSelection {
    let task = fx.task();
    AutoProfileSelection {
        version: 1,
        id: id.into(),
        task: task.id.clone(),
        actor: SelectionActor::Pm,
        selected_at: now(),
        role: ProfileRole::Implementation,
        profile: profile.into(),
        profile_revision: "legacy".into(),
        spec_revision: corral_core::plan_contract_revision(&task).unwrap(),
        catalog_revision: fx.ctx.store.config.profile_catalog_revision().unwrap(),
        reason: "Current task selection".into(),
        closest_alternative: None,
        distinction: None,
        review: None,
    }
}

#[test]
fn task_pinned_queued_retry_refreshes_only_unspecified_bindings() {
    for explicit in ["none", "model", "effort", "both"] {
        // Given a task pin and a completed run, with replacement admission paused.
        let mut fx = configured(1);
        // Built-in aliases and no-op probes keep discovery local and deterministic.
        for harness in fx.ctx.store.config.harnesses.values_mut() {
            harness.command = "true {model_args}".into();
        }
        for agent in fx.ctx.store.config.agents.values_mut() {
            agent.command = "true".into();
        }
        fx.ctx.store.config.harnesses.get_mut("fake").unwrap().flags = Some("claude".into());
        fx.ctx.store.config.profiles.get_mut("fake").unwrap().model = Some("haiku".into());
        fx.ctx.store.save_config().unwrap();
        corral_ops::implementer_set(&fx.ctx, &fx.id, "fake").unwrap();
        fx.done();
        fx.ctx.store.config.dispatch.paused = true;
        fx.ctx.store.save_config().unwrap();
        let choice = ProfileChoice {
            model: matches!(explicit, "model" | "both").then(|| "opus".into()),
            effort: matches!(explicit, "effort" | "both").then(|| "max".into()),
            ..Default::default()
        };
        let DispatchOutcome::Queued { task, .. } =
            corral_ops::retry(&fx.ctx, &fx.id, &choice).unwrap()
        else {
            panic!("expected a paused replacement queue");
        };
        assert_eq!(task.runs.len(), 1);
        assert!(task.launch.is_none());
        assert_eq!(
            task.dispatch_plan.as_ref().unwrap().overrides.as_ref(),
            Some(&choice)
        );
        let history = task.runs;
        let profile = fx.ctx.store.config.profiles.get_mut("fake").unwrap();
        profile.model = Some("sonnet".into());
        profile.effort = Some("high".into());
        fx.ctx.store.save_config().unwrap();
        // When the approved binding changes before public queue admission.
        let admitted = corral_ops::enqueue(&fx.ctx, &fx.id, &DispatchOpts::default()).unwrap();
        // Then only genuine execution overrides freeze, and task/retry provenance survives.
        let plan = admitted.dispatch_plan.as_ref().unwrap();
        let model = choice.model.as_deref().unwrap_or("sonnet");
        let effort = choice.effort.as_deref().unwrap_or("high");
        assert_eq!(plan.route.model.as_deref(), Some(model));
        assert_eq!(plan.route.effort.as_deref(), Some(effort));
        assert!(matches!(plan.route.route, RouteSource::Retry { .. }));
        if explicit == "none" {
            assert!(plan.route.route.is_task_pin());
        }
        assert_eq!(admitted.profile.as_deref(), Some("fake"));
        assert_eq!(admitted.runs, history);
        let route = plan.route.clone();
        fx.ctx.store.config.dispatch.paused = false;
        fx.ctx.store.save_config().unwrap();
        corral_ops::dispatch(&fx.ctx, &fx.id, &DispatchOpts::default()).unwrap();
        let task = fx.task();
        let run = task.current_run().unwrap();
        assert_eq!(run.model.as_deref(), Some(model));
        assert_eq!(run.effort.as_deref(), Some(effort));
        assert_eq!(run.route.as_ref(), Some(&route.route));
        assert_eq!(task.runs[..history.len()], history);
    }
}

#[test]
fn newer_auto_receipt_reconciles_pending_switch_and_eventual_run_history() {
    for target in ["strongest", "fake"] {
        // Given an Auto run whose reasoning failure queued a stronger replacement.
        let mut fx = configured(1);
        let original = selection(&fx, "fake", "original");
        corral_ops::selection_save(&fx.ctx.store, &original).unwrap();
        fx.ctx
            .store
            .update(&fx.id, |task| {
                task.current_run_mut().unwrap().route = Some(RouteSource::Auto {
                    selection: Box::new(original.clone()),
                });
                Ok(())
            })
            .unwrap();
        reject_gate(&fx);
        fx.ctx.store.config.dispatch.paused = true;
        fx.ctx.store.save_config().unwrap();
        let DispatchOutcome::Queued { task, .. } =
            corral_ops::retry(&fx.ctx, &fx.id, &ProfileChoice::default()).unwrap()
        else {
            panic!("expected a paused escalation queue");
        };
        let initial = task
            .dispatch_plan
            .as_ref()
            .unwrap()
            .choice
            .switch
            .as_ref()
            .unwrap();
        assert_eq!(initial.from, "fake");
        assert_eq!(initial.to, "strong");
        let history = task.runs;
        let revised = selection(&fx, target, "reclassified");
        corral_ops::selection_save(&fx.ctx.store, &revised).unwrap();
        // When a newer semantic selection changes the destination before admission.
        let admitted = corral_ops::enqueue(&fx.ctx, &fx.id, &DispatchOpts::default()).unwrap();
        // Then route, pending transition and the eventually reserved run agree.
        let plan = admitted.dispatch_plan.as_ref().unwrap();
        assert_eq!(plan.route.profile, target);
        assert_eq!(plan.route.route.auto_selection(), Some(&revised));
        if target == "fake" {
            assert!(plan.choice.switch.is_none());
        } else {
            let switch = plan.choice.switch.as_ref().unwrap();
            assert_eq!(switch.from, "fake");
            assert_eq!(switch.to, target);
        }
        let RouteSource::Retry { switch, .. } = &plan.route.route else {
            panic!("expected retry provenance");
        };
        assert_eq!(switch, &plan.choice.switch);
        assert_eq!(admitted.runs, history);
        let switches: Vec<_> = plan.choice.switch.clone().into_iter().collect();
        let route = plan.route.clone();
        fx.ctx.store.config.dispatch.paused = false;
        fx.ctx.store.save_config().unwrap();
        corral_ops::dispatch(&fx.ctx, &fx.id, &DispatchOpts::default()).unwrap();
        let task = fx.task();
        let run = task.current_run().unwrap();
        assert_eq!(run.agent, target);
        assert_eq!(run.switches, switches);
        assert_eq!(run.route.as_ref(), Some(&route.route));
        assert_eq!(task.runs[..history.len()], history);
    }
}
