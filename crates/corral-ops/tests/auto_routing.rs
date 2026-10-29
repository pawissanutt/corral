use corral_core::{
    AutoProfileSelection, Harness, Profile, ProfileChoice, ProfileEligibility, ProfileMetadata,
    ProfileRole, RouteSource, SelectionActor, SelectionGuidance, Status, Store, Task, TaskSize,
};
use corral_ops::{Ctx, DispatchOpts};

struct Fixture {
    _dir: tempfile::TempDir,
    ctx: Ctx,
    task: Task,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("app/.git")).unwrap();
        let mut store = Store::init(dir.path()).unwrap();
        store.config.harnesses.insert(
            "fixture".into(),
            Harness {
                command: "true {model_args}".into(),
                flags: Some("generic".into()),
                model_flag: Some("--model".into()),
                effort_flag: None,
                efforts: vec![],
                prompt: None,
            },
        );
        for key in ["general", "deep", "database"] {
            store.config.profiles.insert(key.into(), profile(key));
        }
        store.config.defaults.profile = Some("general".into());
        store.save_config().unwrap();
        let task = store
            .create(
                "Transaction boundary",
                Some("app"),
                Status::Backlog,
                "Preserve atomicity",
            )
            .unwrap();
        let ctx = Ctx {
            herdr: corral_herdr::Herdr::with_bin(
                dir.path().join("must-not-run-herdr"),
                Some("corral-test".into()),
            ),
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
            store,
        };
        Self {
            _dir: dir,
            ctx,
            task,
        }
    }

    fn selection(&self, key: &str, actor: SelectionActor, id: &str) -> AutoProfileSelection {
        let task = self.ctx.store.get(&self.task.id).unwrap();
        AutoProfileSelection {
            version: 1,
            id: id.into(),
            task: task.id.clone(),
            actor,
            selected_at: "2026-10-18T00:00:00Z".parse().unwrap(),
            role: ProfileRole::Implementation,
            profile: key.into(),
            profile_revision: "r1".into(),
            spec_revision: corral_core::plan_contract_revision(&task).unwrap(),
            catalog_revision: self.ctx.store.config.profile_catalog_revision().unwrap(),
            reason: "Transaction correctness dominates the bounded edit".into(),
            closest_alternative: Some("general".into()),
            distinction: Some("Correctness is uncertain despite the small diff".into()),
            review: None,
        }
    }

    fn save(&self, receipt: &AutoProfileSelection) {
        self.ctx
            .store
            .save_auto_profile_selection(receipt, None)
            .unwrap();
    }

    fn route(&self) -> anyhow::Result<corral_ops::RouteExplanation> {
        corral_ops::route(&self.ctx, &self.task.id, &ProfileChoice::default())
    }
}

fn profile(key: &str) -> Profile {
    Profile {
        harness: "fixture".into(),
        model: None,
        vendor: None,
        escalate_to: None,
        effort: None,
        args: vec![],
        metadata: Some(ProfileMetadata {
            display_name: key.into(),
            purpose: "Owner configured domain work".into(),
            guidance: SelectionGuidance {
                use_when: vec!["Persistence correctness is central".into()],
                avoid_when: vec!["Only SQL prose changes".into()],
                distinctions: vec!["General handles repetitive edits".into()],
                ..Default::default()
            },
            instructions: "Inspect transaction boundaries".into(),
            eligibility: ProfileEligibility {
                enabled: true,
                automatic: true,
                roles: [ProfileRole::Implementation].into(),
                projects: Default::default(),
            },
            revision: "r1".into(),
            evidence: None,
            candidates: vec![],
            fallback: None,
        }),
    }
}

#[test]
fn proposer_and_pm_receipts_route_without_manual_pins_or_reclassification() {
    for actor in [SelectionActor::Proposer, SelectionActor::Pm] {
        // Given one current receipt from either supported selector.
        let fx = Fixture::new();
        let receipt = fx.selection("database", actor, "selection-1");
        fx.save(&receipt);
        // When resolving repeatedly without another selector write.
        let first = fx.route().unwrap();
        let second = fx.route().unwrap();
        // Then exact provenance survives and no pin or extra receipt is manufactured.
        assert_eq!(first.implementer, second.implementer);
        assert_eq!(first.implementer.profile, "database");
        assert_eq!(
            first.implementer.route,
            RouteSource::Auto {
                selection: Box::new(receipt)
            }
        );
        assert!(fx.ctx.store.get(&fx.task.id).unwrap().profile.is_none());
        assert_eq!(fx.ctx.store.auto_profile_selections().unwrap().len(), 1);
    }
}

#[test]
fn explicit_choices_and_owner_rules_precede_even_stale_auto_receipts() {
    for source in ["override", "task", "rule"] {
        // Given a now-stale custom-profile recommendation and a higher owner choice.
        let mut fx = Fixture::new();
        fx.save(&fx.selection("database", SelectionActor::Proposer, "selection-1"));
        fx.ctx
            .store
            .update(&fx.task.id, |task| {
                task.body.push_str("\nChanged specification");
                if source == "task" {
                    task.profile = Some("deep".into());
                }
                Ok(())
            })
            .unwrap();
        if source == "rule" {
            fx.ctx
                .store
                .config
                .profiles
                .get_mut("deep")
                .unwrap()
                .metadata
                .as_mut()
                .unwrap()
                .eligibility
                .automatic = false;
            fx.ctx
                .store
                .config
                .routing
                .rules
                .push(corral_core::RoutingRule {
                    name: Some("owner rule".into()),
                    project: vec![],
                    kind: vec![],
                    size: vec![],
                    difficulty: vec![],
                    title_regex: None,
                    body_regex: None,
                    profile: "deep".into(),
                });
        }
        // When resolving the effective route.
        let decision = corral_ops::route(
            &fx.ctx,
            &fx.task.id,
            &ProfileChoice {
                profile: (source == "override").then(|| "deep".into()),
                ..Default::default()
            },
        )
        .unwrap()
        .implementer;
        // Then owner precedence is explicit, never falsely labelled semantic selection.
        assert_eq!(decision.profile, "deep");
        assert!(matches!(
            decision.route,
            RouteSource::Override | RouteSource::Task | RouteSource::Rule { .. }
        ));
    }
}

#[test]
fn changed_specification_blocks_queued_auto_until_new_receipt() {
    // Given an authorized queued Auto selection.
    let fx = Fixture::new();
    fx.save(&fx.selection("database", SelectionActor::Proposer, "selection-1"));
    corral_ops::enqueue(&fx.ctx, &fx.task.id, &DispatchOpts::default()).unwrap();
    fx.ctx
        .store
        .update(&fx.task.id, |task| {
            task.body.push_str("\nNew acceptance criterion");
            Ok(())
        })
        .unwrap();
    // When explaining or re-admitting the old receipt.
    let route = fx.route();
    let enqueue = corral_ops::enqueue(&fx.ctx, &fx.task.id, &DispatchOpts::default());
    // Then stale selection cannot dispatch, while the queued record remains recoverable.
    assert!(route.is_err());
    assert!(enqueue.is_err());
    let task = fx.ctx.store.get(&fx.task.id).unwrap();
    assert!(task.runs.is_empty());
    assert!(task.dispatch_plan.is_some());
    fx.save(&fx.selection("deep", SelectionActor::Pm, "selection-2"));
    assert_eq!(fx.route().unwrap().implementer.profile, "deep");
}

#[test]
fn edited_guidance_and_new_custom_profiles_invalidate_queued_catalogs() {
    for change in ["guidance", "new_profile"] {
        // Given a queued receipt bound to the complete current catalog.
        let mut fx = Fixture::new();
        fx.save(&fx.selection("database", SelectionActor::Pm, "selection-1"));
        corral_ops::enqueue(&fx.ctx, &fx.task.id, &DispatchOpts::default()).unwrap();
        if change == "guidance" {
            fx.ctx
                .store
                .config
                .profiles
                .get_mut("general")
                .unwrap()
                .metadata
                .as_mut()
                .unwrap()
                .guidance
                .distinctions
                .push("New overlap".into());
        } else {
            fx.ctx
                .store
                .config
                .profiles
                .insert("migrations".into(), profile("migrations"));
        }
        fx.ctx.store.save_config().unwrap();
        // When resolving after the catalog edit.
        let result = fx.route();
        // Then even guidance on an overlapping alternative requires a fresh selection.
        assert!(result.is_err(), "{change}");
    }
}

#[test]
fn eligibility_cannot_be_overridden_by_reason_or_manual_pin() {
    for excluded in ["disabled", "role", "project", "automatic"] {
        // Given a valid receipt followed by an owner eligibility restriction.
        let mut fx = Fixture::new();
        fx.save(&fx.selection("database", SelectionActor::Proposer, "selection-1"));
        let eligibility = &mut fx
            .ctx
            .store
            .config
            .profiles
            .get_mut("database")
            .unwrap()
            .metadata
            .as_mut()
            .unwrap()
            .eligibility;
        match excluded {
            "disabled" => eligibility.enabled = false,
            "role" => eligibility.roles = [ProfileRole::Planning].into(),
            "project" => eligibility.projects = ["another-project".into()].into(),
            "automatic" => eligibility.automatic = false,
            _ => unreachable!(),
        }
        // When resolving the saved recommendation.
        let result = fx.route();
        // Then prose never re-enables an excluded profile.
        assert!(result.is_err(), "{excluded}");
        if excluded != "automatic" {
            fx.ctx
                .store
                .update(&fx.task.id, |task| {
                    task.profile = Some("database".into());
                    Ok(())
                })
                .unwrap();
            assert!(fx.route().is_err(), "{excluded}");
        }
    }
}

#[test]
fn queued_task_pin_reresolves_future_binding_and_preserves_pin() {
    // Given a task pin and queue record, then an approved profile binding edit.
    let mut fx = Fixture::new();
    corral_ops::enqueue(&fx.ctx, &fx.task.id, &DispatchOpts::default()).unwrap();
    corral_ops::implementer_set(&fx.ctx, &fx.task.id, "database").unwrap();
    let profile = fx.ctx.store.config.profiles.get_mut("database").unwrap();
    profile.model = Some("vendor/revised".into());
    profile.metadata.as_mut().unwrap().revision = "r2".into();
    fx.ctx.store.save_config().unwrap();
    // When resolving a future run without replacing the manual choice.
    let route = fx.route().unwrap().implementer;
    // Then the stable pin uses the new binding, not the queue's previous model.
    assert_eq!(route.profile, "database");
    assert_eq!(route.model.as_deref(), Some("vendor/revised"));
    assert_eq!(route.route, RouteSource::Task);
    assert_eq!(
        fx.ctx.store.get(&fx.task.id).unwrap().profile.as_deref(),
        Some("database")
    );
}

#[test]
fn queued_planning_pin_accepts_planning_only_profile_and_preserves_intent() {
    // Given queued planning work and a profile eligible only for Planning.
    let mut fx = Fixture::new();
    fx.ctx
        .store
        .config
        .profiles
        .get_mut("general")
        .unwrap()
        .metadata = None;
    let mut planner = profile("plan-only");
    planner.metadata.as_mut().unwrap().eligibility.roles = [ProfileRole::Planning].into();
    fx.ctx
        .store
        .config
        .profiles
        .insert("plan-only".into(), planner);
    fx.ctx.store.save_config().unwrap();
    let queued = corral_ops::enqueue(
        &fx.ctx,
        &fx.task.id,
        &DispatchOpts {
            plan: true,
            profile: Some("general".into()),
            base: Some("topic-base".into()),
            mode: corral_core::RunMode::Headless,
            ..Default::default()
        },
    )
    .unwrap();
    let original = queued.dispatch_plan.unwrap();
    let catalog =
        corral_ops::selection_catalog(&fx.ctx.store, &fx.task.id, ProfileRole::Planning).unwrap();
    assert!(catalog.eligible.contains_key("plan-only"));
    // When the public setter pins the planning-only profile.
    let pinned = corral_ops::implementer_set(&fx.ctx, &fx.task.id, "plan-only").unwrap();
    // Then the pin and subsequent planning resolution preserve queue intent.
    assert_eq!(pinned.profile.as_deref(), Some("plan-only"));
    assert_eq!(
        pinned.dispatch_plan.as_ref().unwrap().authorization,
        original.authorization
    );
    let resolved = fx.route().unwrap().implementer;
    assert_eq!(resolved.profile, "plan-only");
    assert_eq!(resolved.route, RouteSource::Task);
    let admitted = corral_ops::enqueue(&fx.ctx, &fx.task.id, &DispatchOpts::default()).unwrap();
    for task in [&pinned, &admitted] {
        let plan = task.dispatch_plan.as_ref().unwrap();
        assert_eq!(plan.route.profile, "plan-only");
        assert_eq!(plan.route.route, RouteSource::Task);
        assert_eq!(plan.choice.source, Some(RouteSource::Task));
        assert_eq!(plan.mode, original.mode);
        assert_eq!(plan.base, original.base);
        assert!(plan.plan && plan.explicit_plan);
        assert!(task.launch.is_none() && task.plan_runs.is_empty());
    }
}

#[test]
fn queued_role_ineligible_pins_leave_task_and_dispatch_plan_unchanged() {
    for (planning, role, enabled) in [
        (true, ProfileRole::Implementation, true),
        (false, ProfileRole::Planning, true),
        (true, ProfileRole::Planning, false),
    ] {
        // Given queued work and a wrong-role or disabled pin candidate.
        let mut fx = Fixture::new();
        fx.ctx
            .store
            .config
            .profiles
            .get_mut("general")
            .unwrap()
            .metadata = None;
        let mut candidate = profile("ineligible");
        let eligibility = &mut candidate.metadata.as_mut().unwrap().eligibility;
        eligibility.roles = [role].into();
        eligibility.enabled = enabled;
        fx.ctx
            .store
            .config
            .profiles
            .insert("ineligible".into(), candidate);
        fx.ctx.store.save_config().unwrap();
        let before = corral_ops::enqueue(
            &fx.ctx,
            &fx.task.id,
            &DispatchOpts {
                plan: planning,
                profile: Some("general".into()),
                ..Default::default()
            },
        )
        .unwrap();
        // When the public setter attempts the genuinely ineligible pin.
        let result = corral_ops::implementer_set(&fx.ctx, &fx.task.id, "ineligible");
        // Then rejection preserves the task and its queue plan atomically.
        assert!(result.is_err());
        assert_eq!(fx.ctx.store.get(&fx.task.id).unwrap(), before);
    }
}

#[test]
fn default_first_queue_consumes_a_later_proposer_receipt_at_admission() {
    // Given a default queue admitted before the proposer classified it.
    let fx = Fixture::new();
    let queued = corral_ops::enqueue(&fx.ctx, &fx.task.id, &DispatchOpts::default()).unwrap();
    assert_eq!(
        queued.dispatch_plan.unwrap().route.route,
        RouteSource::Defaults
    );
    let receipt = fx.selection("database", SelectionActor::Proposer, "later-selection");
    corral_ops::selection_save(&fx.ctx.store, &receipt).unwrap();
    // When the same public admission resolver processes the queued task.
    let admitted = corral_ops::enqueue(&fx.ctx, &fx.task.id, &DispatchOpts::default()).unwrap();
    // Then the current recommendation, not the earlier default, supplies the run.
    let route = admitted.dispatch_plan.unwrap().route;
    assert_eq!(route.profile, "database");
    assert_eq!(route.route.auto_selection(), Some(&receipt));
    assert_eq!(fx.route().unwrap().implementer, route);
    assert!(admitted.runs.is_empty());
    assert!(admitted.profile.is_none());
}

#[test]
fn default_first_queue_rejects_later_receipts_after_spec_or_catalog_edits() {
    for change in ["spec", "catalog"] {
        // Given a receipt saved after default admission but not yet consumed.
        let mut fx = Fixture::new();
        corral_ops::enqueue(&fx.ctx, &fx.task.id, &DispatchOpts::default()).unwrap();
        let receipt = fx.selection("database", SelectionActor::Proposer, "later-selection");
        corral_ops::selection_save(&fx.ctx.store, &receipt).unwrap();
        if change == "spec" {
            fx.ctx
                .store
                .update(&fx.task.id, |task| {
                    task.body.push_str("\nAdditional acceptance criterion");
                    Ok(())
                })
                .unwrap();
        } else {
            fx.ctx
                .store
                .config
                .profiles
                .get_mut("general")
                .unwrap()
                .metadata
                .as_mut()
                .unwrap()
                .guidance
                .distinctions
                .push("New distinction".into());
            fx.ctx.store.save_config().unwrap();
        }
        let before = fx.ctx.store.get(&fx.task.id).unwrap();
        // When explaining or admitting work through the older default queue.
        let explanation = fx.route();
        let admission = corral_ops::enqueue(&fx.ctx, &fx.task.id, &DispatchOpts::default());
        // Then both reject the stale receipt without admitting a run or rewriting the queue.
        assert!(explanation.is_err(), "{change}");
        assert!(admission.is_err(), "{change}");
        assert_eq!(fx.ctx.store.get(&fx.task.id).unwrap(), before);
    }
}

#[test]
fn unreserved_default_and_rule_queues_resolve_current_model_and_effort_bindings() {
    for source in ["default", "project", "rule"] {
        for old in [None, Some("vendor/old")] {
            // Given an unreserved ordinary route with unset or concrete bindings.
            let mut fx = Fixture::new();
            let harness = fx.ctx.store.config.harnesses.get_mut("fixture").unwrap();
            harness.effort_flag = Some("--effort".into());
            harness.efforts = vec!["low".into(), "high".into()];
            let profile = fx.ctx.store.config.profiles.get_mut("general").unwrap();
            profile.model = old.map(str::to_string);
            profile.effort = old.map(|_| "low".into());
            match source {
                "project" => {
                    fx.ctx
                        .store
                        .config
                        .projects
                        .get_mut("app")
                        .unwrap()
                        .default_profile = Some("general".into())
                }
                "rule" => fx
                    .ctx
                    .store
                    .config
                    .routing
                    .rules
                    .push(corral_core::RoutingRule {
                        name: Some("owner binding".into()),
                        project: vec![],
                        kind: vec![],
                        size: vec![],
                        difficulty: vec![],
                        title_regex: None,
                        body_regex: None,
                        profile: "general".into(),
                    }),
                "default" => {}
                _ => unreachable!(),
            }
            fx.ctx.store.save_config().unwrap();
            let before =
                corral_ops::enqueue(&fx.ctx, &fx.task.id, &DispatchOpts::default()).unwrap();
            let source = before.dispatch_plan.unwrap().route.route;
            let profile = fx.ctx.store.config.profiles.get_mut("general").unwrap();
            profile.model = Some("vendor/new".into());
            profile.effort = Some("high".into());
            profile.metadata.as_mut().unwrap().revision = "r2".into();
            fx.ctx.store.save_config().unwrap();
            // When the future run is resolved through public admission.
            let admitted =
                corral_ops::enqueue(&fx.ctx, &fx.task.id, &DispatchOpts::default()).unwrap();
            // Then copied defaults do not become implicit execution overrides.
            let route = admitted.dispatch_plan.unwrap().route;
            assert_eq!(route.model.as_deref(), Some("vendor/new"));
            assert_eq!(route.effort.as_deref(), Some("high"));
            assert_eq!(route.route, source);
            assert!(admitted.runs.is_empty());
        }
    }
}

#[test]
fn queued_explicit_execution_overrides_survive_binding_and_selection_changes() {
    for overridden in [
        "profile",
        "model",
        "effort",
        "both",
        "model_only",
        "effort_only",
    ] {
        // Given a queue with explicit owner input, not merely resolved defaults.
        let mut fx = Fixture::new();
        let harness = fx.ctx.store.config.harnesses.get_mut("fixture").unwrap();
        harness.effort_flag = Some("--effort".into());
        harness.efforts = vec!["low".into(), "high".into()];
        fx.ctx
            .store
            .config
            .profiles
            .get_mut("general")
            .unwrap()
            .model = Some("vendor/old".into());
        fx.ctx.store.save_config().unwrap();
        let opts = DispatchOpts {
            profile: (!matches!(overridden, "model_only" | "effort_only"))
                .then(|| "general".into()),
            model: matches!(overridden, "model" | "both" | "model_only")
                .then(|| "vendor/explicit".into()),
            effort: matches!(overridden, "effort" | "both" | "effort_only").then(|| "low".into()),
            ..Default::default()
        };
        corral_ops::enqueue(&fx.ctx, &fx.task.id, &opts).unwrap();
        let profile = fx.ctx.store.config.profiles.get_mut("general").unwrap();
        profile.model = Some("vendor/new".into());
        profile.effort = Some("high".into());
        fx.ctx.store.save_config().unwrap();
        corral_ops::selection_save(
            &fx.ctx.store,
            &fx.selection("database", SelectionActor::Pm, "later-selection"),
        )
        .unwrap();
        // When future admission refreshes the approved stable binding.
        let admitted = corral_ops::enqueue(&fx.ctx, &fx.task.id, &DispatchOpts::default()).unwrap();
        // Then only fields actually overridden remain fixed, and Auto cannot replace the profile.
        let route = admitted.dispatch_plan.unwrap().route;
        assert_eq!(route.profile, "general");
        assert_eq!(route.route, RouteSource::Override);
        assert_eq!(
            route.model,
            opts.model.or_else(|| Some("vendor/new".into()))
        );
        assert_eq!(route.effort, opts.effort.or_else(|| Some("high".into())));
    }
}

#[test]
fn legacy_queues_refresh_defaults_without_losing_unknown_explicit_override_intent() {
    for explicit in [false, true] {
        // Given an older serialized plan with no explicit-input field.
        let mut fx = Fixture::new();
        fx.ctx
            .store
            .config
            .profiles
            .get_mut("general")
            .unwrap()
            .model = Some("vendor/old".into());
        fx.ctx.store.save_config().unwrap();
        let queued = corral_ops::enqueue(
            &fx.ctx,
            &fx.task.id,
            &DispatchOpts {
                model: explicit.then(|| "vendor/explicit".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let mut encoded = serde_json::to_value(queued.dispatch_plan.unwrap()).unwrap();
        encoded.as_object_mut().unwrap().remove("overrides");
        let legacy: corral_core::DispatchPlan = serde_json::from_value(encoded).unwrap();
        assert!(legacy.overrides.is_none());
        fx.ctx
            .store
            .update(&fx.task.id, |task| {
                task.dispatch_plan = Some(legacy.clone());
                Ok(())
            })
            .unwrap();
        fx.ctx
            .store
            .config
            .profiles
            .get_mut("general")
            .unwrap()
            .model = Some("vendor/new".into());
        fx.ctx.store.save_config().unwrap();
        // When admitting a future run after an approved binding update.
        let admitted = corral_ops::enqueue(&fx.ctx, &fx.task.id, &DispatchOpts::default()).unwrap();
        // Then defaults refresh, while unknown legacy explicit inputs are retained conservatively.
        let route = admitted.dispatch_plan.unwrap().route;
        assert_eq!(
            route.model.as_deref(),
            Some(if explicit {
                "vendor/explicit"
            } else {
                "vendor/new"
            })
        );
    }
}

#[test]
fn unclassified_work_has_visible_default_provenance() {
    // Given legacy unclassified work with no receipt.
    let fx = Fixture::new();
    // When resolving its route.
    let route = fx.route().unwrap().implementer;
    // Then the configured default is not attributed to either selector actor.
    assert_eq!(route.route, RouteSource::Defaults);
    assert!(matches!(
        serde_json::to_value(route).unwrap()["route"]["source"].as_str(),
        Some("defaults")
    ));
}

#[derive(serde::Deserialize)]
struct SelectionFixture {
    title: String,
    body: String,
    size: TaskSize,
    profile: String,
    alternative: String,
    reason: String,
    distinction: String,
    owner_rule: Option<String>,
    #[serde(default)]
    stale_catalog: bool,
}

#[test]
fn structured_selection_fixtures_preserve_reasoning_independent_of_task_size() {
    let cases: Vec<SelectionFixture> =
        serde_json::from_str(include_str!("fixtures/profile-selections.json")).unwrap();
    for case in cases {
        // Given authored selector examples, not a simulated semantic model.
        let mut fx = Fixture::new();
        fx.ctx
            .store
            .update(&fx.task.id, |task| {
                task.title = case.title.clone();
                task.body = case.body.clone();
                task.size = Some(case.size);
                Ok(())
            })
            .unwrap();
        let mut receipt = fx.selection(&case.profile, SelectionActor::Proposer, "fixture");
        receipt.reason = case.reason;
        receipt.closest_alternative = Some(case.alternative);
        receipt.distinction = Some(case.distinction);
        fx.save(&receipt);
        if let Some(profile) = &case.owner_rule {
            fx.ctx
                .store
                .config
                .routing
                .rules
                .push(corral_core::RoutingRule {
                    name: Some("fixture owner rule".into()),
                    project: vec![],
                    kind: vec![],
                    size: vec![],
                    difficulty: vec![],
                    title_regex: None,
                    body_regex: None,
                    profile: profile.clone(),
                });
        }
        if case.stale_catalog {
            fx.ctx
                .store
                .config
                .profiles
                .insert("migrations".into(), profile("migrations"));
        }
        // When consuming the structured selection.
        let result = fx.route();
        // Then size does not replace the selected intent or custom profile.
        if case.stale_catalog {
            assert!(result.is_err());
            continue;
        }
        let route = result.unwrap().implementer;
        if let Some(profile) = case.owner_rule {
            assert_eq!(route.profile, profile);
            assert!(matches!(route.route, RouteSource::Rule { index: 1, .. }));
            continue;
        }
        assert_eq!(route.profile, case.profile);
        assert_eq!(
            route.route,
            RouteSource::Auto {
                selection: Box::new(receipt)
            }
        );
    }
}
