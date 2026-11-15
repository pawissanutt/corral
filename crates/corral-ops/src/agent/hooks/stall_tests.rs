//! Native provider evidence must come from indexed turns, never empty compatibility state.
use super::*;
use crate::agent::bridge::fixture as f;
use corral_core::agent::{AgentBinding, AgentRole, AgentRunState, HostHealth};
use corral_core::work::{self, Counter, HotScope};

#[test]
fn typed_native_failures_classify_without_text_and_old_turns_still_load() {
    use corral_core::availability::{FailureCategory, ProviderFailure};
    let old = json!({"generation":1,"seq":1,"origin":{"kind":"unknown"},"started":"2026-01-01T00:00:00Z",
        "ended":"2026-01-01T00:00:01Z","class":"provider_failed","provider_error":"rate_limit"});
    let legacy: AgentTurn = serde_json::from_value(old.clone()).unwrap();
    assert!(legacy.provider_failure.is_none());
    assert!(
        serde_json::to_value(&legacy)
            .unwrap()
            .get("provider_failure")
            .is_none()
    );
    assert!(matches!(
        stall(1, Some(&legacy)).unwrap().class,
        AvailabilityClass::Transient { .. }
    ));
    for category in [
        "subscription_quota",
        "transient",
        "auth",
        "billing",
        "invalid_request",
        "unknown",
        "local_budget",
        "network_policy",
    ] {
        let mut value = old.clone();
        let paid = category == "billing";
        let evidence = json!({"version":1,"connection":if paid {"openai-api"} else {"openai-subscription"},
            "category":category,"scope":if category == "subscription_quota" {"account"} else {"unknown"},
            "retry_after_secs":if category == "transient" {Some(120)} else {None}});
        let failure: ProviderFailure = serde_json::from_value(evidence.clone()).unwrap();
        failure.validate().unwrap();
        value["provider_error"] = json!(failure.class());
        value["provider_failure"] = evidence;
        let turn: AgentTurn = serde_json::from_value(value).unwrap();
        match category {
            "subscription_quota" => assert!(matches!(
                stall(1, Some(&turn)).unwrap().class,
                AvailabilityClass::QuotaExhausted { account: true, .. }
            )),
            "transient" => assert_eq!(
                stall(1, Some(&turn)).unwrap().class,
                AvailabilityClass::Transient {
                    retry_after_secs: Some(120)
                }
            ),
            _ => {
                assert!(stall(1, Some(&turn)).is_none());
                assert!(terminal_failure(Some(&turn)).is_some());
            }
        }
        if paid {
            let mut bogus = failure;
            bogus.category = FailureCategory::SubscriptionQuota;
            bogus.scope = corral_core::availability::FailureScope::Account;
            assert!(bogus.validate().is_err());
            assert!(bogus.availability().is_none());
        }
    }
}

#[test]
fn native_failure_event_binds_connection_persists_reset_and_replays_once() {
    use crate::agent::{self, test_support as ts};
    let fx = ts::fx();
    let id = ts::worker_task(&fx, "p1");
    let target = agent::BindTarget::Worker {
        task: id.clone(),
        run: "R1".into(),
    };
    let host = ts::bind(&fx, &target, "p1", "failure-session");
    ts::committed(&ts::hello(&fx, &host));
    let send = |seq, payload| {
        let input = ts::envelope(
            &host,
            "event",
            &agent::event_request_id(1, seq),
            payload,
            false,
        );
        ts::send(&fx, &input, "event")
    };
    ts::committed(&send(
        1,
        json!({"kind":"turn_started","turn":1,"provider":"openai-codex","model":"fixture-model"}),
    ));
    let failure = json!({"version":1,"connection":"openai-subscription","category":"subscription_quota","scope":"account",
        "reset_at":"2027-01-15T08:10:00Z","request_ref":"a".repeat(64)});
    let mut wrong = failure.clone();
    wrong["connection"] = json!("anthropic-subscription");
    assert_eq!(
        ts::code(&send(
            2,
            json!({"kind":"provider_error","turn":1,"class":"quota","failure":wrong})
        )),
        Some("bad_request")
    );
    let mut raw = failure.clone();
    raw["raw_body"] = json!("secret");
    assert_eq!(
        ts::code(&send(
            3,
            json!({"kind":"provider_error","turn":1,"class":"quota","failure":raw})
        )),
        Some("bad_request")
    );
    let payload = json!({"kind":"provider_error","turn":1,"class":"quota","failure":failure});
    let receipt = ts::committed(&send(4, payload.clone()));
    assert_eq!(receipt.result["failure"], failure);
    assert!(matches!(
        send(4, payload),
        agent::CallResponse::Receipt { replayed: true, .. }
    ));
    ts::committed(&send(5, json!({"kind":"turn_ended","turn":1})));
    let container = ContainerRef::Worker {
        task: id.clone(),
        run: "R1".into(),
    };
    let head = fx.ctx.store.head(&id).unwrap();
    let turn = head.latest_agent_turn(&container, 1).unwrap().unwrap();
    let evidence = turn.provider_failure.as_ref().unwrap();
    assert_eq!(serde_json::to_value(evidence).unwrap(), failure);
    let stalled = stall(1, Some(&turn)).unwrap();
    assert_eq!(
        stalled.class.reset_at(stalled.at),
        Some("2027-01-15T08:10:00Z".parse().unwrap())
    );
    // Confirmed quota is immediate owner attention, not a same-candidate retry or paid launch.
    let change = advance_agent(
        &fx.ctx,
        &head,
        head.current_run.as_deref().unwrap(),
        stalled.at,
    )
    .unwrap()
    .unwrap();
    assert_eq!(change.to, Status::NeedsMe);
    assert!(fx.ctx.store.get(&id).unwrap().runs.len() == 1);
}

#[test]
fn reviewer_auth_failure_is_selected_from_real_current_generation_with_constant_work() {
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let fx = f::fixture(false, 5);
        let target = f::reviewing(&fx);
        let container = target.container();
        let mut host = AgentRunState::new(AgentBinding {
            role: AgentRole::Reviewer,
            store: "fixture".into(),
            project: "corral".into(),
            generation: 2,
            health: HostHealth::Healthy,
            callback: None,
            closing: false,
        });
        let at: Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
        for n in 0..2 * scale {
            host.turns.push(serde_json::from_value(json!({"generation":1,"seq":n,"origin":{"kind":"unknown"},"started":at,"ended":null,"class":null,"provider_error":null})).unwrap());
            host.turns.push(serde_json::from_value(json!({"generation":2,"seq":n,"origin":{"kind":"unknown"},"started":at,"ended":at,"class":"completed","provider_error":null})).unwrap());
        }
        host.turns.push(serde_json::from_value(json!({"generation":2,"seq":9999,"origin":{"kind":"unknown"},"started":at,"ended":at,"class":"provider_failed","provider_error":"auth"})).unwrap());
        // A later append in an older generation cannot conceal current authentication failure.
        host.turns.push(serde_json::from_value(json!({"generation":1,"seq":99999,"origin":{"kind":"unknown"},"started":at,"ended":null,"class":null,"provider_error":null})).unwrap());
        fx.ctx
            .store
            .update(&fx.id, |task| {
                task.runs[0].review_runs[0].agent_host = Some(Box::new(host));
                Ok(())
            })
            .unwrap();
        let head = fx.ctx.store.head(&fx.id).unwrap();
        let hot = HotScope::enter();
        let before = work::snapshot();
        let Drive::Escalated(reason) = advance_reviewer(&fx.ctx, &head, &container, at).unwrap()
        else {
            panic!("authentication failure was hidden")
        };
        assert!(reason.contains("corral agent login") && reason.contains("no paid fallback"));
        let after = work::snapshot();
        measured.push(Counter::ALL.map(|c| after.total(c) - before.total(c)));
        assert_eq!(
            after.total(Counter::FullTaskHydrations) - before.total(Counter::FullTaskHydrations),
            0
        );
        drop(hot);
        assert_eq!(fx.events("provider_escalated").len(), 1);
        assert!(fx.events("provider_retry").is_empty());
    }
    assert_eq!(measured[0], measured[1]);
}

#[test]
fn native_quota_assesses_worker_and_reviewer_but_other_stalls_do_no_extra_work() {
    use crate::agent::{BindTarget, test_support as ts};
    use crate::availability::continuation;
    use corral_core::{CandidateBinding, ExecutionSnapshot};
    for reviewer in [false, true] {
        let mut fx = ts::fx();
        let profile = serde_json::from_value(json!({
            "harness":"corral-pi","vendor":"openai","model":"same-model",
            "metadata":{"display_name":"fixture","purpose":"fixture","guidance":{},
                "instructions":"fixture","revision":"fixture","evidence":"synthetic",
                "eligibility":{"enabled":true,"automatic":true,"roles":["implementation","implementation_review"],"projects":[]},
                "candidates":[
                    {"id":"sub","harness":"corral-pi","connection":"openai-subscription","provider":"openai","model":"same-model","args":[],"evidence":"synthetic","cost":{"kind":"subscription"}},
                    {"id":"api","harness":"corral-pi","connection":"openai-api","provider":"openai","model":"same-model","args":[],"evidence":"synthetic","cost":{"kind":"metered","currency":"USD","maximum_micros":1}}
                ],
                "fallback":{"automatic":true,"paid_roles":["worker","reviewer"],"approved_providers":["openai"],
                    "max_switches":1,"allow_unknown_subscription_cost":true,"approval":"synthetic",
                    "spending":{"currency":"USD","maximum_micros":1}}
            }
        })).unwrap();
        fx.ctx.store.config.profiles.insert("pi".into(), profile);
        fx.ctx.store.save_config().unwrap();
        let resolved = fx
            .ctx
            .store
            .config
            .resolve_candidate(
                "pi",
                &fx.ctx.store.config.profiles["pi"]
                    .metadata
                    .as_ref()
                    .unwrap()
                    .candidates[0],
            )
            .unwrap();
        let binding = resolved.connection_binding().unwrap().unwrap();
        let mut snapshot = ExecutionSnapshot::new(&fx.ctx.store.config, resolved);
        snapshot.candidate = Some(CandidateBinding {
            candidate: "sub".into(),
            episode: None,
            reason: None,
            skipped: vec![],
            pinned: false,
        });
        let (id, target) = if reviewer {
            ts::reviewer_task(&fx)
        } else {
            let id = ts::worker_task(&fx, "p1");
            (
                id.clone(),
                BindTarget::Worker {
                    task: id,
                    run: "R1".into(),
                },
            )
        };
        fx.ctx
            .store
            .update(&id, |task| {
                if reviewer {
                    task.runs[0].review = Some(
                        serde_json::from_value(json!({
                            "sha":"abc","reviewer":"pi","at":"2026-10-23T00:00:00Z"
                        }))
                        .unwrap(),
                    );
                }
                let run = if reviewer {
                    &mut task.runs[0].review_runs[0]
                } else {
                    &mut task.runs[0]
                };
                run.harness = Some("corral-pi".into());
                run.model = Some("same-model".into());
                run.execution = Some(snapshot.clone());
                Ok(())
            })
            .unwrap();
        let container = target.container();
        let host = ts::bind(
            &fx,
            &target,
            if reviewer { "p2" } else { "p1" },
            "quota-session",
        );
        ts::committed(&ts::hello(&fx, &host));
        let at = corral_core::clock::now();
        continuation::with_state(&fx.ctx, &container, |state| {
            state.generations[0].connection = Some(binding.clone());
            state.binding.health = HostHealth::Healthy;
            state.turns.push(serde_json::from_value(json!({
                "generation":1,"seq":1,"origin":{"kind":"unknown"},"started":at,"ended":at,"class":"provider_failed",
                "provider_error":"quota","provider_failure":{"version":1,"connection":"openai-subscription",
                    "category":"subscription_quota","scope":"account"}
            })).unwrap());
            Ok(())
        }).unwrap();
        let head = fx.ctx.store.head(&id).unwrap();
        let run = if reviewer {
            head.current_run
                .as_deref()
                .unwrap()
                .current_child
                .as_deref()
                .unwrap()
        } else {
            head.current_run.as_deref().unwrap()
        };
        let stalled = stall(1, head.latest_agent_turn(&container, 1).unwrap().as_ref()).unwrap();
        let mut state = None;
        advance_state(&mut state, Some(&stalled), true, at);
        let mut state = state.unwrap();
        let fallback = fx
            .ctx
            .store
            .config
            .profiles
            .get_mut("pi")
            .unwrap()
            .metadata
            .as_mut()
            .unwrap()
            .fallback
            .as_mut()
            .unwrap();
        let paid_roles = std::mem::take(&mut fallback.paid_roles);
        {
            let _hot = HotScope::enter();
            let before = work::snapshot();
            assert!(!select_quota_fallback(&fx.ctx, &head, run, &container, &state, at).unwrap());
            assert_eq!(work::snapshot(), before);
        }
        fx.ctx
            .store
            .config
            .profiles
            .get_mut("pi")
            .unwrap()
            .metadata
            .as_mut()
            .unwrap()
            .fallback
            .as_mut()
            .unwrap()
            .paid_roles = paid_roles;
        state.class = Some(AvailabilityClass::Transient {
            retry_after_secs: None,
        });
        {
            let _hot = HotScope::enter();
            let before = work::snapshot();
            assert!(!select_quota_fallback(&fx.ctx, &head, run, &container, &state, at).unwrap());
            assert_eq!(work::snapshot(), before);
        }
        if reviewer {
            assert!(matches!(
                advance_reviewer(&fx.ctx, &head, &container, at).unwrap(),
                Drive::Fallback(_)
            ));
        } else {
            assert!(advance_agent(&fx.ctx, &head, run, at).unwrap().is_none());
        }
        let episode = if reviewer {
            crate::availability::review_episode(&id, head.current_run.as_deref().unwrap(), "pi")
        } else {
            crate::availability::run_episode_head(&id, run)
        };
        let selected = crate::availability::load(&fx.ctx, &episode)
            .unwrap()
            .unwrap();
        assert_eq!(selected.attempts.last().unwrap().candidate, "api");
        assert!(selected.attempts.last().unwrap().run.is_none());
        assert!(
            STALLS
                .load(&fx.ctx.store, &run_key(&id, &container).unwrap())
                .unwrap()
                .unwrap()
                .fallback_selected
        );
    }
}
