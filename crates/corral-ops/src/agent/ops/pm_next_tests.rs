//! Native next/brief priority, project scope and executor identity regression.
use super::*;

#[test]
fn next_and_brief_share_workset_priority_without_leaking_foreign_actions() {
    let (mut fx, host) = fixture();
    fx.ctx.resource_probe = crate::resources::FixedResourceProbe::arc();
    let held = task(&fx, "corral", "staged, not scheduled");
    fx.ctx
        .store
        .update(&held.id, |t| {
            t.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    let outside = fx
        .ctx
        .store
        .create("foreign", Some("other"), Status::Ready, "")
        .unwrap();
    let inside = fx
        .ctx
        .store
        .create("local", Some("corral"), Status::Ready, "")
        .unwrap();
    let preview = fx
        .ctx
        .store
        .preview_operating_policy(&corral_core::OperatingSelection {
            tasks: [outside.id.clone(), inside.id.clone()].into(),
            authority: corral_core::OperatingAuthority {
                dispatch: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    fx.ctx.store.config = fx
        .ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", Timestamp::now())
        .unwrap();
    let before = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    let bytes: Vec<_> = [&held.id, &inside.id]
        .iter()
        .map(|id| ts::task_bytes(&fx, id))
        .collect();
    // The global CLI prefers the earlier foreign action; project reads must select again
    // within their approved projects, not filter only the globally selected result.
    assert_eq!(
        crate::pm_next(&fx.ctx).unwrap().task.as_deref(),
        Some(outside.id.as_str())
    );
    for foreign_done in [false, true] {
        if foreign_done {
            fx.ctx
                .store
                .update(&outside.id, |t| {
                    t.status = Status::Done;
                    Ok(())
                })
                .unwrap();
        }
        let cli = serde_json::to_value(crate::pm_next(&fx.ctx).unwrap()).unwrap();
        for op in ["read_next", "read_brief"] {
            let response = send(&fx, &host, op, "next-read", json!({}));
            let result = ts::committed(&response).result;
            let next = if op == "read_brief" {
                &result["next"]
            } else {
                &result
            };
            assert_eq!(next["task"], inside.id);
            assert_eq!(next["kind"], "dispatch");
            assert_eq!(next["requires_owner"], false);
            assert!(
                !serde_json::to_string(&result)
                    .unwrap()
                    .contains(&outside.id)
            );
            if foreign_done {
                assert_eq!(*next, cli);
            }
            assert_eq!(before, pm_state::load(&fx.ctx.store).unwrap().unwrap());
        }
    }
    // A grant only in another project is not an empty workspace policy: it must not
    // resurrect unrelated held backlog cards when intersected with this PM's projects.
    let preview = fx
        .ctx
        .store
        .preview_operating_policy(&corral_core::OperatingSelection {
            tasks: [outside.id.clone()].into(),
            authority: corral_core::OperatingAuthority {
                dispatch: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    fx.ctx.store.config = fx
        .ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", Timestamp::now())
        .unwrap();
    for op in ["read_next", "read_brief"] {
        let response = send(&fx, &host, op, "foreign-only", json!({}));
        let result = ts::committed(&response).result;
        let next = if op == "read_brief" {
            &result["next"]
        } else {
            &result
        };
        assert_eq!(next["kind"], "wait");
        assert_eq!(next["task"], Value::Null);
    }
    // Mixed-project idea options must be hidden without changing the executable ID
    // that the shared workspace executor revalidates.
    std::fs::create_dir(fx.ctx.store.root().join(".git")).unwrap();
    fx.ctx.store.config.limits.running = 3;
    fx.ctx.store.config.limits.review = 1;
    for (project, mode) in [
        ("corral", corral_core::PromoteIdeas::Auto),
        ("other", corral_core::PromoteIdeas::Suggest),
    ] {
        fx.ctx
            .store
            .config
            .projects
            .get_mut(project)
            .unwrap()
            .dispatch
            .promote_ideas = Some(mode);
    }
    fx.ctx.store.save_config().unwrap();
    let idea = fx
        .ctx
        .store
        .create(
            "local idea",
            Some("corral"),
            Status::Idea,
            "## Acceptance\nFinite local task works.",
        )
        .unwrap();
    let foreign_idea = fx
        .ctx
        .store
        .create(
            "foreign idea",
            Some("other"),
            Status::Idea,
            "## Acceptance\nFinite foreign task works.",
        )
        .unwrap();
    fx.ctx
        .store
        .update(&idea.id, |t| {
            t.priority = "P0".into();
            Ok(())
        })
        .unwrap();
    let preview = fx
        .ctx
        .store
        .preview_operating_policy(&corral_core::OperatingSelection {
            tasks: [idea.id.clone()].into(),
            authority: corral_core::OperatingAuthority {
                dispatch: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    fx.ctx.store.config = fx
        .ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", Timestamp::now())
        .unwrap();
    let canonical = crate::pm_next(&fx.ctx).unwrap();
    assert_eq!(canonical.kind, crate::PmActionKind::IdeaPromotion);
    assert_eq!(canonical.shortlist.len(), 2);
    for op in ["read_next", "read_brief"] {
        let response = send(&fx, &host, op, "ideas", json!({}));
        let result = ts::committed(&response).result;
        let next = if op == "read_brief" {
            &result["next"]
        } else {
            &result
        };
        assert_eq!(next["task"], idea.id);
        assert_eq!(next["command"], canonical.command);
        assert_eq!(next["requires_owner"], false);
        assert_eq!(next["shortlist"].as_array().unwrap().len(), 1);
        assert!(
            !serde_json::to_string(&result)
                .unwrap()
                .contains(&foreign_idea.id)
        );
    }
    assert_eq!(bytes[0], ts::task_bytes(&fx, &held.id));
    assert_eq!(bytes[1], ts::task_bytes(&fx, &inside.id));
}

#[test]
fn argumentless_cards_bound_evidence_and_pending_questions_without_changing_identity() {
    let (mut fx, host) = fixture();
    fx.ctx.resource_probe = crate::resources::FixedResourceProbe::arc();
    let id = ts::worker_task(&fx, "worker");
    let worker = ts::bind(
        &fx,
        &BindTarget::Worker {
            task: id.clone(),
            run: "R1".into(),
        },
        "worker",
        "worker-session",
    );
    ts::committed(&ts::hello(&fx, &worker));
    ts::committed(&send(
        &fx,
        &worker,
        "ask",
        "ask",
        json!({"kind":"clarification","text":"Which route?","blocking":false}),
    ));
    fx.ctx
        .store
        .update(&id, |task| {
            let state = task.runs[0].agent_host.as_mut().unwrap();
            let question = state.questions[0].clone();
            for n in 2..90 {
                let mut pending = question.clone();
                pending.key = format!("{id}/R1/Q{n}");
                state.questions.push(pending);
            }
            Ok(())
        })
        .unwrap();
    let canonical = crate::pm_next(&fx.ctx).unwrap();
    let cli = serde_json::to_value(&canonical).unwrap();
    let budget = corral_core::agent::RESULT_MAX_BYTES;
    let unchanged = super::super::super::pm_bound::bounded_next(&canonical, budget).unwrap();
    assert_eq!(unchanged, cli);
    // Exercise the pure projection with multibyte and JSON-escaped evidence.
    // No gate subprocess or large persisted run record is needed for this defect.
    let mut large = canonical.clone();
    large.evidence.push("λ\\\"\n".repeat(budget));
    for op in ["read_next", "read_brief"] {
        let result = if op == "read_next" {
            super::super::super::pm_bound::bounded_next(&large, budget).unwrap()
        } else {
            super::super::super::pm_bound::bounded_brief(
                &large,
                json!({"project":"corral","counts":{},"questions":[]}),
                budget,
            )
            .unwrap()
        };
        assert!(serde_json::to_vec(&result).unwrap().len() <= budget);
        let next = if op == "read_brief" {
            &result["next"]
        } else {
            &result
        };
        for field in [
            "kind",
            "task",
            "why",
            "command",
            "verify",
            "options",
            "recommended",
            "requires_owner",
        ] {
            assert_eq!(next[field], cli[field], "{op}: {field}");
        }
        assert!(next["bounded"]["evidence_chars_omitted"].as_u64().unwrap() > 0);
        assert!(
            next["evidence"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item.as_str().unwrap().contains("[truncated:"))
        );
    }
    let response = send(&fx, &host, "read_brief", "many-questions", json!({}));
    let result = ts::committed(&response).result;
    assert!(serde_json::to_vec(&result).unwrap().len() <= budget);
    assert_eq!(result["questions_total"], 89);
    let omitted = result["questions_omitted"].as_u64().unwrap();
    assert!(omitted > 0);
    assert_eq!(
        result["questions"].as_array().unwrap().len() as u64 + omitted,
        89
    );
    for field in [
        "kind",
        "task",
        "command",
        "options",
        "recommended",
        "requires_owner",
    ] {
        assert_eq!(result["next"][field], cli[field], "{field}");
    }
}
