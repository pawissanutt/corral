use std::path::Path;
use std::process::{Command, Stdio};

use corral_core::{AgentCmd, PlanReview, PlanRun, PlanRunState, Status, Store, Verdict};
use corral_ops::{PlanVerdict, plan_submit, plan_verdict};
use jiff::Timestamp;
use serde_json::Value;

const PLAN: &str = "---\ngoal: Design the PM flow\nnon_goals: []\ndecisions: []\naffected_paths: [pm.rs]\ncontracts: []\ndependencies: []\nacceptance: [{scenario: decision pending, expected: owner card}]\nverification: [cargo test]\nowner_decisions: [Choose the PM boundary]\n---\nTechnical assessment only.\n";
const NOTES: &str = "Historical reviewer-policy mismatch. Technical assessment is not approval.";

fn cli(root: &Path, args: &[&str]) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("CORRAL_") || name.starts_with("HERDR_") {
            command.env_remove(key);
        }
    }
    let before = Timestamp::now();
    let output = command
        .env("CORRAL_ROOT", root)
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let after = Timestamp::now();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    println!(
        "{}\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout)
    );
    let mut value = serde_json::from_slice(&output.stdout).unwrap();
    validate_readback_ages(&mut value, before, after);
    value
}

fn validate_readback_ages(value: &mut Value, before: Timestamp, after: Timestamp) {
    match value {
        Value::Object(fields) => {
            if let Some(age) = fields.get("age_secs") {
                if let Some(since) = fields["since"].as_str() {
                    let since: Timestamp = since.parse().unwrap();
                    let age = age.as_i64().expect("assessment age must be seconds");
                    let earliest = before.duration_since(since).as_secs().max(0);
                    let latest = after.duration_since(since).as_secs().max(0);
                    assert!(
                        (earliest..=latest).contains(&age),
                        "assessment age {age} is outside {earliest}..={latest}"
                    );
                    // Only a validated live age may vary between separate CLI reads.
                    fields.remove("age_secs");
                } else {
                    assert!(fields["since"].is_null());
                    assert!(
                        age.is_null(),
                        "unknown canonical time must have unknown age"
                    );
                }
            }
            for child in fields.values_mut() {
                validate_readback_ages(child, before, after);
            }
        }
        Value::Array(children) => {
            for child in children {
                validate_readback_ages(child, before, after);
            }
        }
        _ => {}
    }
}

#[test]
fn readback_ages_allow_a_second_boundary_but_preserve_canonical_fields() {
    let mut first = serde_json::json!({
        "key": "T-001/plan_failed/D1:Failed",
        "since": "2026-10-16T10:00:00Z",
        "age_secs": 1
    });
    let mut second = first.clone();
    second["age_secs"] = 2.into();
    let before: Timestamp = "2026-10-16T10:00:01Z".parse().unwrap();
    let after: Timestamp = "2026-10-16T10:00:02Z".parse().unwrap();
    validate_readback_ages(&mut first, before, after);
    validate_readback_ages(&mut second, before, after);
    assert_eq!(first, second);
    second["since"] = "2026-10-16T10:00:01Z".into();
    assert_ne!(first, second);
}

#[test]
#[should_panic(expected = "assessment age 3 is outside 1..=2")]
fn readback_ages_reject_an_age_outside_the_observation_interval() {
    let mut value = serde_json::json!({
        "since": "2026-10-16T10:00:00Z",
        "age_secs": 3
    });
    validate_readback_ages(
        &mut value,
        "2026-10-16T10:00:01Z".parse().unwrap(),
        "2026-10-16T10:00:02Z".parse().unwrap(),
    );
}

fn fixture() -> (tempfile::TempDir, corral_ops::Ctx, String) {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.limits.running = 7;
    store.config.review.plan_required = Some(true);
    store.config.agents.insert(
        "reviewer".into(),
        AgentCmd {
            command: "true".into(),
        },
    );
    store.config.review.reviewer = Some("reviewer".into());
    store.save_config().unwrap();
    let task = store
        .create(
            "T-048-shaped pending PM plan",
            None,
            Status::NeedsMe,
            "## Goal\nDesign the PM boundary",
        )
        .unwrap();
    let ctx = corral_ops::resources::FixedResourceProbe::ctx(store);
    let source = tmp.path().join("plan.md");
    std::fs::write(&source, PLAN).unwrap();
    for _ in 0..3 {
        plan_submit(&ctx, &task.id, &source).unwrap();
    }
    // Use the actual policy digest, without launching a reviewer or planner.
    let digest = plan_verdict(
        &ctx,
        &task.id,
        &PlanVerdict {
            plan: "P3",
            attempt: None,
            verdict: Verdict::Escalate,
            notes: NOTES,
        },
    )
    .unwrap()
    .policy_digest;
    ctx.store
        .update(&task.id, |t| {
            t.plans.last_mut().unwrap().reviews = vec![PlanReview {
                delivery: corral_core::inbox::DeliveryMode::Legacy,
                inbox: Default::default(),
                route: None,
                id: "PR1".into(),
                reviewer: "reviewer".into(),
                policy_digest: digest,
                verdict: Some(Verdict::Escalate),
                notes: Some(NOTES.into()),
                at: "2026-10-16T09:00:00Z".parse().unwrap(),
                herdr: None,
                pending_reply: None,
            }];
            t.plan_runs.push(PlanRun {
                execution: None,
                route: None,
                delivery: corral_core::inbox::DeliveryMode::Legacy,
                inbox: Default::default(),
                id: "D1".into(),
                profile: "omo".into(),
                harness: "omo".into(),
                model: Some("fixture-planner".into()),
                vendor: Some("openai".into()),
                effort: None,
                spec_revision: corral_ops::plan_spec_revision(t)?,
                started: "2026-10-16T10:00:00Z".parse().unwrap(),
                ended: Some("2026-10-16T10:00:01Z".parse().unwrap()),
                herdr: None,
                output_dir: tmp.path().join("planner"),
                submitted: None,
                rounds: 0,
                state: PlanRunState::Failed,
                pending_reply: Some("Planner exited before submission (exit 1).".into()),
            });
            Ok(())
        })
        .unwrap();
    (tmp, ctx, task.id)
}

fn readbacks(root: &Path, ctx: &corral_ops::Ctx) -> Value {
    let tasks = ctx.store.list().unwrap();
    let events = ctx.store.events().unwrap();
    let next = cli(root, &["pm", "next", "--json"]);
    let brief = cli(root, &["pm", "brief", "--json"]);
    assert_eq!(brief["next"], next);
    assert_eq!(next, cli(root, &["pm", "next", "--json"]));
    assert_eq!(ctx.store.list().unwrap(), tasks);
    assert_eq!(ctx.store.events().unwrap(), events);
    next
}

#[test]
fn escalated_plan_survives_later_failed_planner_in_next_and_brief() {
    // Given an unaccepted P3/PR1 followed by an unrelated failed D1.
    let (tmp, ctx, id) = fixture();
    // When the built CLI reads both PM surfaces without syncing.
    let card = readbacks(tmp.path(), &ctx);
    // Then the owner sees one closed, evidence-backed decision, not idle wait.
    assert_eq!(card["kind"], "owner");
    assert_eq!(card["task"], id);
    assert_eq!(card["requires_owner"], true);
    let evidence = card["evidence"].to_string();
    for fact in ["P3", "PR1", NOTES, "D1"] {
        assert!(evidence.contains(fact), "missing fixture fact: {fact}");
    }
    let choices = card["options"].as_array().unwrap();
    assert!(choices.iter().any(|o| o["id"] == "review_plan"));
    assert!(!choices.iter().any(|o| o["id"] == "approve_plan"));
    assert!(choices.iter().all(|o| o["requires_owner"] == true));
}

#[test]
fn reviewed_plan_offers_owner_acceptance_without_performing_it() {
    // Given independent approval, but no final owner acceptance.
    let (tmp, ctx, id) = fixture();
    ctx.store
        .update(&id, |t| {
            t.plans.last_mut().unwrap().reviews[0].verdict = Some(Verdict::Approve);
            Ok(())
        })
        .unwrap();
    // When next and brief are read.
    let card = readbacks(tmp.path(), &ctx);
    // Then acceptance is an explicit owner-only choice on the exact plan.
    assert_eq!(card["kind"], "owner");
    assert_eq!(card["task"], id);
    assert_eq!(card["requires_owner"], true);
    let approve = card["options"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == "approve_plan")
        .unwrap();
    assert_eq!(approve["requires_owner"], true);
    let command = approve["command"].as_str().unwrap();
    assert!(command.contains(" plan verdict "));
    assert!(command.contains(" approve --owner --plan 'P3' --notes-file "));
    assert!(ctx.store.get(&id).unwrap().plans[2].approval.is_none());
}

#[test]
fn failed_planner_without_pending_plan_requires_owner_attention() {
    // Given failed planning with no submitted plan.
    let (tmp, ctx, id) = fixture();
    ctx.store
        .update(&id, |t| {
            t.plans.clear();
            Ok(())
        })
        .unwrap();
    // When PM readbacks inspect the terminal run.
    let card = readbacks(tmp.path(), &ctx);
    // Then its actual failure is surfaced rather than losing the task.
    assert_eq!(card["kind"], "owner");
    assert_eq!(card["task"], id);
    assert_eq!(card["requires_owner"], true);
    assert!(card["evidence"].to_string().contains("exit 1"));
}

#[test]
fn active_replacement_planner_does_not_resurface_old_plan_decision() {
    // Given an explicitly started replacement after the old escalation.
    let (tmp, ctx, id) = fixture();
    ctx.store
        .update(&id, |t| {
            t.status = Status::Running;
            t.plan_runs[0].state = PlanRunState::Planning;
            t.plan_runs[0].ended = None;
            t.plan_runs[0].pending_reply = None;
            Ok(())
        })
        .unwrap();
    // When PM readbacks inspect current work.
    let card = readbacks(tmp.path(), &ctx);
    // Then they preserve a non-owner wait for the actual active planner.
    assert_eq!(card["kind"], "wait");
    assert_eq!(card["requires_owner"], false);
    assert!(
        !card["options"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["id"] == "approve_plan")
    );
}

#[test]
fn obsolete_or_accepted_plan_does_not_resurrect_historical_escalation() {
    for accepted in [false, true] {
        // Given a newer revision that supersedes P3, without an active planner.
        let (tmp, ctx, id) = fixture();
        ctx.store
            .update(&id, |t| {
                let mut latest = t.plans[2].clone();
                latest.id = "P4".into();
                latest.reviews.clear();
                if accepted {
                    latest.approval = Some(corral_core::PlanApproval {
                        authorization: None,
                        by: "owner".into(),
                        at: "2026-10-16T11:00:00Z".parse().unwrap(),
                        policy_digest: "fixture".into(),
                    });
                } else {
                    latest.spec_revision = "obsolete".into();
                }
                t.plans.push(latest);
                t.plan_runs.clear();
                t.status = Status::Backlog;
                Ok(())
            })
            .unwrap();
        // When both readbacks select relevant state.
        let card = readbacks(tmp.path(), &ctx);
        // Then an old escalation cannot become a current owner decision.
        assert_eq!(card["kind"], "wait");
        assert_eq!(card["task"], Value::Null);
    }
}

#[test]
fn idle_store_retains_plain_wait_with_no_owner_authorization() {
    // Given a genuinely empty isolated store.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let ctx = corral_ops::resources::FixedResourceProbe::ctx(store);
    // When both PM surfaces are queried.
    let card = readbacks(tmp.path(), &ctx);
    // Then the ordinary idle control remains unchanged.
    assert_eq!(card["kind"], "wait");
    assert_eq!(card["task"], Value::Null);
    assert_eq!(card["requires_owner"], false);
    assert_eq!(card["recommended"], "wait");
    assert_eq!(card["options"].as_array().unwrap().len(), 1);
}

#[test]
fn historical_or_stale_reviewer_approval_cannot_offer_owner_acceptance() {
    for stale_policy in [false, true] {
        // Given approval from either an unauthorized or obsolete review.
        let (tmp, ctx, id) = fixture();
        ctx.store
            .update(&id, |t| {
                let review = &mut t.plans[2].reviews[0];
                review.verdict = Some(Verdict::Approve);
                if stale_policy {
                    review.policy_digest = "historical-policy".into();
                } else {
                    review.reviewer = "not-the-configured-reviewer".into();
                }
                Ok(())
            })
            .unwrap();
        // When both CLI readbacks inspect current independent-review policy.
        let card = readbacks(tmp.path(), &ctx);
        // Then neither policy drift nor another profile's verdict is acceptance.
        assert_eq!(card["kind"], "owner");
        let choices = card["options"].as_array().unwrap();
        assert!(!choices.iter().any(|o| o["id"] == "approve_plan"));
        assert!(choices.iter().any(|o| o["id"] == "review_plan"));
    }
}

#[test]
fn changed_plan_bytes_only_offer_inspection_not_invalid_verdict_commands() {
    // Given a source changed after independent review.
    let (tmp, ctx, id) = fixture();
    ctx.store
        .update(&id, |t| {
            t.plans[2].reviews[0].verdict = Some(Verdict::Approve);
            Ok(())
        })
        .unwrap();
    std::fs::write(tmp.path().join("plan.md"), "Changed plan bytes").unwrap();
    // When readiness checks the exact source and snapshot.
    let card = readbacks(tmp.path(), &ctx);
    // Then the owner sees the blocked decision without invalid review/approval options.
    assert_eq!(card["kind"], "owner");
    assert_eq!(card["task"], id);
    assert_eq!(card["options"].as_array().unwrap().len(), 1);
    assert_eq!(card["recommended"], "inspect_plan");
}

#[test]
fn current_review_wait_survives_unrelated_terminal_planner() {
    // Given a fresh independent review pending on P3, alongside failed D1.
    let (tmp, ctx, id) = fixture();
    ctx.store
        .update(&id, |t| {
            t.plans[2].reviews[0].verdict = None;
            t.plans[2].reviews[0].herdr = Some(corral_core::HerdrRef {
                workspace_id: "live-review".into(),
                pane_id: "live-review:p1".into(),
                socket: None,
            });
            t.status = Status::Review;
            Ok(())
        })
        .unwrap();
    // When the real CLI selects a card.
    let card = readbacks(tmp.path(), &ctx);
    // Then it preserves the review wait rather than recommending another review.
    assert_eq!(card["kind"], "review_in_flight");
    assert_eq!(card["task"], id);
    assert_eq!(card["requires_owner"], false);
    assert_eq!(card["options"].as_array().unwrap().len(), 1);
    assert_eq!(card["recommended"], "wait");
}

#[test]
fn blocked_active_planner_surfaces_its_question_not_the_old_review() {
    // Given an active replacement blocked on new owner input.
    let (tmp, ctx, id) = fixture();
    let question = "Choose the new planner scope.";
    ctx.store
        .update(&id, |t| {
            t.plan_runs[0].state = PlanRunState::Planning;
            t.plan_runs[0].ended = None;
            t.plan_runs[0].pending_reply = Some(question.into());
            Ok(())
        })
        .unwrap();
    // When both PM surfaces resolve the latest relevant work.
    let card = readbacks(tmp.path(), &ctx);
    // Then the live planner's owner input wins over historical P3.
    assert_eq!(card["kind"], "owner");
    assert_eq!(card["task"], id);
    assert_eq!(card["requires_owner"], true);
    assert!(card["evidence"].to_string().contains(question));
    assert!(
        !card["options"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["id"] == "approve_plan")
    );
}

#[test]
fn owner_verdict_options_execute_on_the_exact_isolated_plan_with_notes() {
    for verdict in ["approve", "changes"] {
        // Given current independent approval and an explicit fixture owner choice.
        let (tmp, ctx, id) = fixture();
        ctx.store
            .update(&id, |t| {
                t.plans[2].reviews[0].verdict = Some(Verdict::Approve);
                Ok(())
            })
            .unwrap();
        let card = cli(tmp.path(), &["pm", "next", "--json"]);
        let notes = ctx
            .store
            .state_dir()
            .join(format!("{id}-P3-owner-plan-notes.md"));
        let choice = card["options"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| {
                o["id"]
                    == if verdict == "approve" {
                        "approve_plan"
                    } else {
                        "request_plan_changes"
                    }
            })
            .unwrap();
        assert_eq!(choice["requires_owner"], true);
        assert_eq!(
            choice["command"],
            format!(
                "'{}' --root '{}' plan verdict '{id}' {verdict} --owner --plan 'P3' --notes-file '{}' --json",
                env!("CARGO_BIN_EXE_corral"),
                tmp.path().display(),
                notes.display()
            )
        );
        std::fs::write(
            &notes,
            "Resolved: Choose the PM boundary => Keep the existing card system.\n",
        )
        .unwrap();
        // When the owner executes the card's exact argv, only in this isolated store.
        let result = cli(
            tmp.path(),
            &[
                "plan",
                "verdict",
                &id,
                verdict,
                "--owner",
                "--plan",
                "P3",
                "--notes-file",
                notes.to_str().unwrap(),
                "--json",
            ],
        );
        // Then the supported command records the owner's choice without implementation.
        assert_eq!(result["verdict"], verdict);
        assert_eq!(result["reviewer"], "owner");
        let task = ctx.store.get(&id).unwrap();
        assert_eq!(task.plans[2].approval.is_some(), verdict == "approve");
        assert!(task.runs.is_empty());
        assert_eq!(task.plan_runs.len(), 1);
        assert_eq!(task.plan_runs[0].state, PlanRunState::Failed);
    }
}
