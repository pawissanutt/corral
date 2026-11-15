use super::*;
use crate::agent::test_support::{self as ts, Fx, Host};
use crate::agent::{BindTarget, CallResponse};
use crate::gate_triage::{FailureClass, admit, check_retry, observe, require_pass, status};
use corral_core::{GateRecord, GateResult, OperatingActionKind, Status};
use std::path::Path;

fn gate(result: &str, head: &str, attempt: &str) -> GateRecord {
    serde_json::from_value(json!({
        "result": result, "sha": head, "at": "2026-01-01T00:00:00Z",
        "identity": {"version":2,"task":"T-001","run":"R1","attempt":attempt,
            "head":head,"tree":"tree","integration_base":"base","dispatch_base":"base",
            "spec_digest":"spec","policy_digest":"policy","inputs_digest":"inputs"}
    }))
    .unwrap()
}

struct Fixture {
    fx: Fx,
    host: Host,
    task: String,
    report: GateDiagnosis,
    quick: GateRecord,
    failed: GateRecord,
}

fn git(repo: &Path, args: &[&str]) {
    let output = crate::test_support::command("git", repo)
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture(assistant: bool, delegated: bool) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("app");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "repair B",
        ],
    );
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.projects.insert(
        "app".into(),
        corral_core::Project {
            path: "app".into(),
            dispatch: Default::default(),
            default_profile: None,
            base: None,
            build_exit: None,
            review: Default::default(),
            artifact_retention: None,
            post_land: Vec::new(),
        },
    );
    store.save_config().unwrap();
    let task = store
        .create(
            "T956 repaired candidate",
            Some("app"),
            Status::Review,
            "scope",
        )
        .unwrap()
        .id;
    if delegated {
        let preview = store
            .preview_operating_policy(&corral_core::OperatingSelection {
                tasks: [task.clone()].into(),
                authority: corral_core::OperatingAuthority {
                    dispatch: true,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        store.config = store
            .confirm_operating_policy(&preview, "fixture owner", store.now())
            .unwrap();
    }
    let fixed = corral_git::rev_sha(&repo, "HEAD").unwrap();
    let failed = gate("fail", "broken-A", "full-A");
    let mut quick = gate("pass", &fixed, "quick-B");
    quick.identity.as_mut().unwrap().gate_tier = Some(Box::new(corral_core::GateTierBinding {
        tier: "quick".into(),
        selection_sha256: "quick-selection".into(),
        source: "fixture".into(),
    }));
    observe(&store, &task, &failed).unwrap();
    admit(&store, &task, Some(&failed), &fixed, "quick-B").unwrap();
    observe(&store, &task, &quick).unwrap();
    store.update(&task, |task| {
        task.status = Status::NeedsMe;
        task.runs.push(serde_json::from_value(json!({"id":"R1", "agent":"fake",
            "worktree":repo, "branch":"main", "base":"main", "base_sha":"base",
            "started":task.created, "outcome":"done", "gate":quick,
            "review":{"verdict":"approve", "reviewer":"independent", "sha":fixed, "at":task.created},
            "autopilot":{"stage":"escalated", "at":task.created}}))?);
        Ok(())
    }).unwrap();
    let fx = Fx {
        _tmp: tmp,
        ctx: Ctx::from_store(store),
    };
    let proof = fx._tmp.path().join("proof.md");
    std::fs::write(
        &proof,
        "Full A failed from the repaired candidate defect. Quick B is not full validation.",
    )
    .unwrap();
    let artifact = crate::add_artifact(
        &fx.ctx,
        &task,
        &proof,
        Some(corral_core::ArtifactKind::Report),
        None,
    )
    .unwrap();
    let target = if assistant {
        let id = fx
            .ctx
            .store
            .create_assistant("pi", Some("app"), Some("a-pane"), None)
            .unwrap()
            .id;
        fx.ctx
            .store
            .update_assistant(&id, |session| {
                session.project = Some("app".into());
                Ok(())
            })
            .unwrap();
        BindTarget::Assistant { session: id }
    } else {
        BindTarget::Pm {
            project: "app".into(),
        }
    };
    let host = ts::bind(&fx, &target, "triage-pane", "triage-session");
    ts::committed(&ts::hello(&fx, &host));
    Fixture {
        fx,
        host,
        task,
        quick,
        failed,
        report: GateDiagnosis {
            attempt: "full-A".into(),
            class: FailureClass::Candidate,
            explanation: "Repair B fixes the evidenced candidate defect in full A".into(),
            evidence: vec![artifact.path],
            baseline: None,
            retry: false,
        },
    }
}

fn send(s: &Fixture, host: &Host, request: &str, payload: Value) -> CallResponse {
    ts::send(
        &s.fx,
        &ts::envelope(host, "gate_triage", request, payload, false),
        "gate_triage",
    )
}

fn propose(s: &Fixture, key: &str, report: &GateDiagnosis) -> CallResponse {
    let mut host = s.host.clone();
    host.identity.turn = Some(1);
    send(
        s,
        &host,
        &tool_request_id(&host.identity.session_id, key),
        json!({"action":"propose", "task":s.task, "diagnosis":report}),
    )
}

fn decision(s: &Fixture, proposal: &Value, approved: bool) -> (String, Value) {
    let id = proposal["proposal"].as_str().unwrap();
    (
        tool_request_id(
            &s.host.identity.session_id,
            &format!("gate-triage-decision:{}:{}", s.host.identity.generation, id),
        ),
        json!({"action":"decide", "proposal":id, "digest":proposal["digest"], "approved":approved}),
    )
}

fn unchanged(s: &Fixture, before: &corral_core::Task, credit: &GateTriage) {
    let after = s.fx.ctx.store.get(&s.task).unwrap();
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(&after).unwrap()
    );
    let state = status(&s.fx.ctx, &s.task).unwrap();
    assert_eq!(state.diagnostic_attempt, credit.diagnostic_attempt);
    assert_eq!(state.diagnostic_history, credit.diagnostic_history);
    assert_eq!(state.retry_requested, credit.retry_requested);
    assert_eq!(state.owner_investigations, credit.owner_investigations);
    assert_eq!(state.failures[0].gate, s.failed);
}

#[test]
fn t956_in_chat_confirmation_unblocks_full_repair_without_grant_retry_or_launch() {
    for assistant in [false, true] {
        let s = fixture(assistant, false);
        let before = s.fx.ctx.store.get(&s.task).unwrap();
        let credit = status(&s.fx.ctx, &s.task).unwrap();
        assert!(
            check_retry(
                &s.fx.ctx.store,
                &s.task,
                Some(&s.quick),
                &s.quick.sha,
                Some("full-B")
            )
            .is_err()
        );
        let proposed = ts::committed(&propose(&s, "attribution", &s.report)).result;
        assert_eq!(proposed["outcome"], "proposed");
        assert_eq!(
            proposed["diagnosis"],
            serde_json::to_value(&s.report).unwrap()
        );
        unchanged(&s, &before, &credit);
        assert!(
            status(&s.fx.ctx, &s.task).unwrap().failures[0]
                .diagnoses
                .is_empty()
        );
        assert!(matches!(
            propose(&s, "attribution", &s.report),
            CallResponse::Receipt { replayed: true, .. }
        ));
        let (request, payload) = decision(&s, &proposed, true);
        let recorded = ts::committed(&send(&s, &s.host, &request, payload.clone()));
        assert_eq!(recorded.result["outcome"], "recorded");
        unchanged(&s, &before, &credit);
        assert!(require_pass(&s.fx.ctx.store, &s.task, Some(&s.failed)).is_err());
        assert!(matches!(
            send(&s, &s.host, &request, payload),
            CallResponse::Receipt { replayed: true, .. }
        ));
        assert!(
            !check_retry(
                &s.fx.ctx.store,
                &s.task,
                Some(&s.quick),
                &s.quick.sha,
                Some("full-B")
            )
            .unwrap()
        );
        admit(
            &s.fx.ctx.store,
            &s.task,
            Some(&s.quick),
            &s.quick.sha,
            "full-B",
        )
        .unwrap();
        require_pass(&s.fx.ctx.store, &s.task, Some(&s.quick)).unwrap();
        let state = observe(&s.fx.ctx.store, &s.task, &s.quick).unwrap();
        assert!(
            state.failures[0].resolved_by.is_none(),
            "quick B must not clear full A"
        );
        assert!(
            crate::operating::require(
                &s.fx.ctx.store,
                &s.fx.ctx.store.head(&s.task).unwrap(),
                OperatingActionKind::Gate
            )
            .is_err()
        );
        let full = gate("pass", &s.quick.sha, "full-B");
        require_pass(&s.fx.ctx.store, &s.task, Some(&full)).unwrap();
        let state = observe(&s.fx.ctx.store, &s.task, &full).unwrap();
        assert_eq!(state.failures[0].resolved_by.as_ref(), Some(&full));
        assert_eq!(state.failures[0].gate.result, GateResult::Fail);
        assert_eq!(state.diagnostic_history, ["quick-B"]);
    }
}

#[test]
fn delegated_attribution_records_without_confirmation_or_stage_changes() {
    let s = fixture(false, true);
    let before = s.fx.ctx.store.get(&s.task).unwrap();
    let credit = status(&s.fx.ctx, &s.task).unwrap();
    let recorded = ts::committed(&propose(&s, "delegated", &s.report));
    assert_eq!(recorded.result["outcome"], "recorded");
    unchanged(&s, &before, &credit);
    assert_eq!(
        status(&s.fx.ctx, &s.task).unwrap().failures[0].diagnoses,
        std::slice::from_ref(&s.report)
    );
}

#[test]
fn forged_model_decisions_wrong_digest_and_foreign_session_are_refused() {
    let s = fixture(false, false);
    let proposed = ts::committed(&propose(&s, "attribution", &s.report)).result;
    let (request, payload) = decision(&s, &proposed, true);
    let mut model = s.host.clone();
    model.identity.turn = Some(1);
    assert_eq!(
        ts::code(&send(&s, &model, &request, payload.clone())),
        Some("not_owner_input")
    );
    let mut nested = s.host.clone();
    nested.identity.parent = Some(corral_core::agent::CallParent {
        tool_call_id: "model-tool".into(),
        cell: "cell".into(),
        call: "cell.1".into(),
    });
    let nested_request = tool_request_id(
        &nested.identity.session_id,
        &nested.identity.parent.as_ref().unwrap().child_key(),
    );
    assert_eq!(
        ts::code(&send(&s, &nested, &nested_request, payload.clone())),
        Some("not_owner_input")
    );
    let mut unauthenticated = s.host.clone();
    unauthenticated.host_cap = "0".repeat(64);
    assert_eq!(
        ts::code(&send(&s, &unauthenticated, &request, payload.clone())),
        Some("auth_failed")
    );
    let forged_request = tool_request_id(&s.host.identity.session_id, "model-decision");
    assert_eq!(
        ts::code(&send(&s, &s.host, &forged_request, payload.clone())),
        Some("not_owner_input")
    );
    let mut wrong = payload.clone();
    wrong["digest"] = json!("wrong");
    assert_eq!(
        ts::code(&send(&s, &s.host, &request, wrong)),
        Some("payload_mismatch")
    );
    let mut foreign = s.host.clone();
    foreign.identity.session_id = "other-session".into();
    let foreign_request = tool_request_id(
        "other-session",
        &format!(
            "gate-triage-decision:{}:{}",
            foreign.identity.generation,
            proposed["proposal"].as_str().unwrap()
        ),
    );
    assert!(ts::code(&send(&s, &foreign, &foreign_request, payload)).is_some());
    assert!(
        status(&s.fx.ctx, &s.task).unwrap().failures[0]
            .diagnoses
            .is_empty()
    );
}

#[test]
fn decline_replay_and_conflicting_answer_never_record_or_reprompt() {
    let s = fixture(false, false);
    let proposed = ts::committed(&propose(&s, "attribution", &s.report)).result;
    let (request, payload) = decision(&s, &proposed, false);
    assert_eq!(
        ts::committed(&send(&s, &s.host, &request, payload.clone())).result["outcome"],
        "declined"
    );
    assert!(matches!(
        send(&s, &s.host, &request, payload),
        CallResponse::Receipt { replayed: true, .. }
    ));
    let (_, approve) = decision(&s, &proposed, true);
    assert_eq!(
        ts::code(&send(&s, &s.host, &request, approve)),
        Some("payload_mismatch")
    );
    assert!(
        status(&s.fx.ctx, &s.task).unwrap().failures[0]
            .diagnoses
            .is_empty()
    );
}

#[test]
fn stale_candidate_spec_policy_prior_triage_and_live_holds_refuse_confirmation() {
    for change in ["candidate", "spec", "policy", "triage", "hold", "recovery"] {
        let mut s = fixture(false, false);
        let proposed = ts::committed(&propose(&s, "attribution", &s.report)).result;
        match change {
            "candidate" => {
                let repo = &s.fx.ctx.store.get(&s.task).unwrap().runs[0].worktree;
                git(
                    repo,
                    &[
                        "-c",
                        "user.name=Test",
                        "-c",
                        "user.email=test@example.invalid",
                        "-c",
                        "commit.gpgsign=false",
                        "commit",
                        "--allow-empty",
                        "-qm",
                        "changed",
                    ],
                );
            }
            "spec" => {
                s.fx.ctx
                    .store
                    .update(&s.task, |task| {
                        task.title.push_str(" changed");
                        Ok(())
                    })
                    .unwrap();
            }
            "policy" => {
                s.fx.ctx
                    .store
                    .config
                    .review
                    .gate
                    .get_or_insert_with(Vec::new)
                    .push("changed-policy".into());
                s.fx.ctx.store.save_config().unwrap();
            }
            "triage" => {
                let mut state = status(&s.fx.ctx, &s.task).unwrap();
                state.retry_requested = true;
                s.fx.ctx.store.save_record(AREA, &s.task, &state).unwrap();
            }
            "hold" => {
                crate::hold::hold(&s.fx.ctx, &s.task, Some("live stop")).unwrap();
            }
            "recovery" => {
                s.fx.ctx
                    .store
                    .update(&s.task, |task| {
                        task.runs[0].pending_reply = Some("unsettled owner delivery".into());
                        Ok(())
                    })
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let (request, payload) = decision(&s, &proposed, true);
        let response = send(&s, &s.host, &request, payload);
        assert!(ts::code(&response).is_some(), "{change}: {response:?}");
        assert!(
            status(&s.fx.ctx, &s.task).unwrap().failures[0]
                .diagnoses
                .is_empty()
        );
    }
}

#[test]
fn malformed_evidence_retry_and_holds_refuse_without_proposals() {
    let s = fixture(false, false);
    let mut report = s.report.clone();
    report.evidence = vec!["not-attached".into()];
    assert_eq!(
        ts::code(&propose(&s, "invalid-evidence", &report)),
        Some("fixed_rule")
    );
    report = s.report.clone();
    report.retry = true;
    assert_eq!(
        ts::code(&propose(&s, "retry", &report)),
        Some("bad_request")
    );
    crate::hold::hold(&s.fx.ctx, &s.task, Some("stop")).unwrap();
    assert_eq!(
        ts::code(&propose(&s, "held", &s.report)),
        Some("fixed_rule")
    );
    assert!(
        status(&s.fx.ctx, &s.task).unwrap().failures[0]
            .diagnoses
            .is_empty()
    );
}

#[test]
fn reattribution_requires_new_bound_owner_confirmation_even_inside_grant() {
    let s = fixture(false, true);
    ts::committed(&propose(&s, "initial", &s.report));
    let mut report = s.report.clone();
    report.class = FailureClass::Unknown;
    report.explanation = "Retained evidence requires revising the original attribution".into();
    let proposed = ts::committed(&propose(&s, "replacement", &report)).result;
    assert_eq!(proposed["outcome"], "proposed");
    assert_eq!(
        status(&s.fx.ctx, &s.task).unwrap().failures[0].diagnoses,
        std::slice::from_ref(&s.report)
    );
    let (request, payload) = decision(&s, &proposed, true);
    ts::committed(&send(&s, &s.host, &request, payload));
    let state = status(&s.fx.ctx, &s.task).unwrap();
    assert_eq!(state.failures[0].diagnoses, [s.report.clone(), report]);
    assert_eq!(state.diagnostic_history, ["quick-B"]);
    assert!(
        check_retry(
            &s.fx.ctx.store,
            &s.task,
            Some(&s.quick),
            &s.quick.sha,
            Some("full-B")
        )
        .is_err()
    );
}

#[test]
fn pending_proposal_can_be_explicitly_revisited_with_fresh_confirmation_only() {
    let s = fixture(false, false);
    let original = ts::committed(&propose(&s, "cancelled-dialog", &s.report)).result;
    let fresh = ts::committed(&propose(&s, "owner-explicitly-revisits", &s.report)).result;
    assert_ne!(fresh["proposal"], original["proposal"]);
    assert_ne!(fresh["digest"], original["digest"]);
    assert!(
        status(&s.fx.ctx, &s.task).unwrap().failures[0]
            .diagnoses
            .is_empty()
    );
    let (request, payload) = decision(&s, &fresh, true);
    ts::committed(&send(&s, &s.host, &request, payload));
    let old: Proposal =
        s.fx.ctx
            .store
            .record(PROPOSALS, original["proposal"].as_str().unwrap())
            .unwrap()
            .unwrap();
    assert_eq!(old.decision, None);
    let (request, payload) = decision(&s, &original, true);
    assert_eq!(
        ts::code(&send(&s, &s.host, &request, payload)),
        Some("stale_revision")
    );
}

#[test]
fn fence_and_hold_races_are_rechecked_inside_the_receipted_commit() {
    for fence in [false, true] {
        let s = fixture(false, false);
        let proposal = ts::committed(&propose(&s, "attribution", &s.report)).result;
        let (request, payload) = decision(&s, &proposal, true);
        let store = s.fx.ctx.store.clone();
        let task = s.task.clone();
        let container = admission::container_of(&s.host.identity);
        let generation = s.host.identity.generation;
        crate::agent::receipts::FAILPOINT.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move |name| {
                if name == "agent_admitted_before_commit" {
                    if fence {
                        crate::agent::admission::fence_generation(
                            &store,
                            &container,
                            generation,
                            "owner stopped",
                        )
                        .unwrap();
                    } else {
                        crate::hold::hold(
                            &Ctx::from_store(store.clone()),
                            &task,
                            Some("racing owner stop"),
                        )
                        .unwrap();
                    }
                }
            }));
        });
        let response = send(&s, &s.host, &request, payload);
        crate::agent::receipts::FAILPOINT.with_borrow_mut(|hook| *hook = None);
        assert_eq!(
            ts::code(&response),
            Some(if fence { "agent_fenced" } else { "fixed_rule" })
        );
        assert!(
            status(&s.fx.ctx, &s.task).unwrap().failures[0]
                .diagnoses
                .is_empty()
        );
    }
}
