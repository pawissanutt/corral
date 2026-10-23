//! The review Gate contract of a tiered quick Plan member: one authoritative prompt section
//! under the built-in template and an override without {{gate}}, and a packet `gate_contract`
//! whose gaps ask for a fresh member-tier receipt and never for a full one.
use std::collections::BTreeMap;

use corral_core::{ExpectedFailure, MemberGateAmendment, ScopeGate, TierPolicy, Verdict};
use corral_ops::{DispatchOpts, DispatchOutcome};
use serde_json::{Value, json};

use super::{Fx, candidate, fixture, packet};

const SECTION: &str = "## Gate contract (review, authoritative)";
const REASON: &str = "Fails until T-999 restores the widget cache";

/// Imports a two-member integration Plan, declares the first member's gate as the owner, and
/// dispatches and submits that member.
fn member(fx: &Fx) -> String {
    let ctx = &fx.ctx;
    let source = ctx.store.root().join("plan.md");
    // The built-in run template still names the full gate until worker-pm-tier-instructions.
    std::fs::create_dir_all(ctx.store.prompts_dir()).unwrap();
    std::fs::write(
        ctx.store.prompts_dir().join("run.md"),
        "{{id}} {{title}}\n{{body}}\nRun the gate below once after the final rebase.\n{{gate}}\n",
    )
    .unwrap();
    std::fs::write(
        &source,
        "---\ntitle: Integration Plan\nproject: app\napproved_scope:\n  goal: Ship A and B\n  acceptance: [{scenario: Combine, expected: Both included}]\n  delivery_target: {kind: integration, branch: feature/plan}\ntasks:\n  - {key: a, title: A, profile: author}\n  - {key: b, title: B, profile: author}\n---\n",
    )
    .unwrap();
    let imported =
        corral_ops::import_plan(ctx, &source, false, corral_core::Origin::Owner).unwrap();
    let id = imported.tasks[0].id.clone();
    let record = MemberGateAmendment {
        member: id.clone(),
        expected_revision: 0,
        gate: ScopeGate {
            tier: "quick".into(),
            tests: vec!["test(widget)".into()],
            expected_failures: vec![ExpectedFailure {
                test: "corral-ops widget::cache".into(),
                reason: REASON.into(),
            }],
        },
        reason: "declared at decomposition".into(),
        by: "owner".into(),
    };
    ctx.store
        .amend_member_gate(
            &imported.tracker.id,
            &record,
            |_, _| Ok(()),
            |_, _| Ok(None),
        )
        .unwrap();
    let opts = DispatchOpts {
        profile: Some("author".into()),
        ..DispatchOpts::default()
    };
    let DispatchOutcome::Started { task, .. } = corral_ops::dispatch(ctx, &id, &opts).unwrap()
    else {
        panic!("expected Started");
    };
    let worktree = task.current_run().unwrap().worktree.clone();
    std::fs::write(worktree.join("widget.rs"), "changed\n").unwrap();
    corral_git::commit_all(&worktree, "Change the widget").unwrap();
    let identity = corral_ops::progress_readback(ctx, &id).unwrap();
    corral_ops::save_checkpoint(
        ctx,
        &id,
        &corral_core::CheckpointUpdate {
            task: identity.task,
            run: identity.run,
            spec_revision: identity.spec_revision,
            expected_revision: 0,
            progress_revision: 0,
            head: identity.head,
            base: identity.base,
            tree: identity.tree,
            criterion_ids: vec![],
            evidence_refs: vec![],
            next_action: "Review the candidate".into(),
            decisions: vec![],
            waits: vec![],
            resources: vec![],
        },
    )
    .unwrap();
    corral_ops::done(
        ctx,
        &id,
        "widget changed\nVerified: quick gate\nNot verified: none",
    )
    .unwrap();
    id
}

fn prompt(fx: &Fx, id: &str) -> String {
    let run = fx.ctx.store.get(id).unwrap().current_run().unwrap().clone();
    std::fs::read_to_string(
        fx.ctx
            .store
            .prompts_dir()
            .join(format!("{id}-{}-review.md", run.id)),
    )
    .unwrap()
}

fn gate_gaps(bound: &Value) -> Vec<Value> {
    bound["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|gap| gap["code"].as_str().unwrap().starts_with("gate_"))
        .cloned()
        .collect()
}

fn assert_one_section(prompt: &str) {
    assert_eq!(prompt.matches(SECTION).count(), 1, "{prompt}");
    assert!(prompt.contains("- Tier: quick"), "{prompt}");
    assert!(prompt.contains("echo quick-ok"), "{prompt}");
    assert!(!prompt.contains("echo gate-ok"), "{prompt}");
    assert!(
        prompt.contains(&format!("`corral-ops widget::cache`: {REASON}")),
        "{prompt}"
    );
    assert!(prompt.contains("`test(widget)`"), "{prompt}");
}

#[test]
fn quick_member_review_carries_one_gate_contract_and_needs_only_its_own_tier() {
    // Given a tiered project and an owner-declared quick member gate.
    let fx = fixture(|config| {
        config.review.gate_tiers = Some(BTreeMap::from([(
            "quick".to_owned(),
            vec!["echo quick-ok".to_owned()],
        )]));
        config.review.tier_policy = Some(TierPolicy {
            plan_member: Some("quick".into()),
            review_rounds: Some("quick".into()),
        });
    });
    let id = member(&fx);

    // When its review launches from the built-in template before any gate ran.
    corral_ops::review_run(&fx.ctx, &id, None).unwrap();

    // Then the prompt carries one Gate contract section and the packet the full contract.
    assert_one_section(&prompt(&fx, &id));
    let first = packet(&fx, &id);
    let contract = &first["bound"]["gate_contract"];
    assert_eq!(contract["tier"], "quick");
    assert_eq!(contract["commands"], json!(["echo quick-ok"]));
    assert_eq!(contract["tests"], json!(["test(widget)"]));
    assert_eq!(
        contract["expected_failures"],
        json!([{"test": "corral-ops widget::cache", "reason": REASON}])
    );
    assert_eq!(contract["landing_tier"], "quick");
    assert_eq!(contract["destination"]["kind"], "plan_target");
    assert_eq!(contract["destination"]["branch"], "feature/plan");
    assert!(contract["scope"].is_array());
    assert_eq!(
        contract["receipts"],
        json!([
            {"tier": "quick", "required": true, "status": "missing", "fresh": false, "attempt": null},
            {"tier": "full", "required": false, "status": "missing", "fresh": false, "attempt": null},
        ])
    );
    // And it reports the missing member-tier receipt as the only gate gap.
    let gaps = gate_gaps(&first["bound"]);
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert_eq!(gaps[0]["code"], "gate_missing");
    assert!(gaps[0]["detail"].as_str().unwrap().contains("quick-tier"));

    // When the member's quick gate passes and the review is relaunched.
    corral_ops::gate_start(&fx.ctx, &id).unwrap();
    corral_ops::gate_wait(&fx.ctx, &id).unwrap();
    corral_ops::review_run(&fx.ctx, &id, None).unwrap();

    // Then the fresh quick receipt clears every gap although no full receipt exists.
    let gated = packet(&fx, &id);
    let receipts = &gated["bound"]["gate_contract"]["receipts"];
    assert_eq!(receipts[0]["status"], "pass");
    assert_eq!(receipts[0]["fresh"], true);
    assert_eq!(receipts[1]["status"], "missing");
    assert_eq!(gated["bound"]["gaps"], json!([]));
    let approved =
        corral_ops::verdict(&fx.ctx, &id, Verdict::Approve, "quick scope demonstrated").unwrap();
    assert_eq!(
        approved.verdict,
        Some(Verdict::Approve),
        "{:?}",
        approved.notes
    );

    // When an override template without {{gate}} renders the next review.
    std::fs::write(
        fx.ctx.store.prompts_dir().join("review.md"),
        "# Custom review {{id}}\n\nJudge the change.\n",
    )
    .unwrap();
    corral_ops::review_run(&fx.ctx, &id, None).unwrap();

    // Then it still carries exactly one Gate contract section. The override is a verification
    // input, so the quick receipt is no longer fresh and the gap names the member tier.
    assert_one_section(&prompt(&fx, &id));
    let overridden = packet(&fx, &id);
    assert!(overridden["bound"]["gate_contract"].is_object());
    let gaps = gate_gaps(&overridden["bound"]);
    assert_eq!(gaps.len(), 1, "{gaps:?}");
    assert_eq!(gaps[0]["code"], "gate_stale");
    assert!(
        gaps[0]["detail"]
            .as_str()
            .unwrap()
            .contains("fresh quick-tier")
    );
}

fn tiered(config: &mut corral_core::Config) {
    config.review.gate_tiers = Some(BTreeMap::from([(
        "quick".to_owned(),
        vec!["echo quick-ok".to_owned()],
    )]));
    config.review.tier_policy = Some(TierPolicy {
        plan_member: Some("quick".into()),
        review_rounds: Some("quick".into()),
    });
}

fn clean_run_template(fx: &Fx) {
    let dir = fx.ctx.store.prompts_dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("run.md"),
        "{{id}} {{title}}\n{{body}}\nRun the gate below once after the final rebase.\n{{gate}}\n",
    )
    .unwrap();
}

fn grant(fx: &mut Fx, id: &str) {
    fx.ctx.store.config.dispatch.paused = false;
    fx.ctx.store.save_config().unwrap();
    let selection = corral_core::OperatingSelection {
        tasks: [id.to_owned()].into_iter().collect(),
        authority: corral_core::OperatingAuthority {
            dispatch: true,
            accept: false,
            plan_accept: false,
            aggregate_accept: false,
        },
        ..corral_core::OperatingSelection::default()
    };
    let preview = fx.ctx.store.preview_operating_policy(&selection).unwrap();
    fx.ctx
        .store
        .confirm_operating_policy(&preview, "owner", jiff::Timestamp::now())
        .unwrap();
    fx.ctx.store.reload_config_if_changed().unwrap();
}

fn kinds(fx: &Fx, id: &str) -> Vec<String> {
    fx.ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.task.as_deref() == Some(id))
        .filter(|e| {
            matches!(
                e.kind.as_str(),
                "automation_blocked" | "automation_resumed" | "review_started"
            )
        })
        .map(|e| e.kind)
        .collect()
}

#[test]
fn autopilot_reports_an_unchanged_review_instruction_conflict_once_across_syncs() {
    // Given an automatic, gated main-bound quick candidate whose routed reviewer's
    // instructions demand the full gate.
    let mut fx = fixture(tiered);
    clean_run_template(&fx);
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    grant(&mut fx, &id);
    let instructions = |fx: &mut Fx, text: &str| {
        let profile = fx.ctx.store.config.profiles.get_mut("impl").unwrap();
        profile.metadata.as_mut().unwrap().instructions = text.into();
        fx.ctx.store.save_config().unwrap();
    };
    instructions(&mut fx, "Run the full gate.");

    // When autopilot syncs three times with nothing changed.
    for _ in 0..3 {
        corral_ops::sync(&fx.ctx).unwrap();
    }

    // Then the conflict is reported once, never falsely resumed, and no reviewer starts.
    assert_eq!(kinds(&fx, &id), ["automation_blocked"]);
    let blocked = fx
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .find(|e| e.task.as_deref() == Some(id.as_str()) && e.kind == "automation_blocked")
        .unwrap();
    assert_eq!(
        blocked.detail["conflict"]["surface"],
        "reviewer_instructions"
    );
    assert_eq!(
        blocked.detail["conflict"]["path"],
        "profiles.impl.metadata.instructions"
    );

    // When the conflict is resolved and autopilot syncs again.
    instructions(&mut fx, "Custom guidance for haiku");
    corral_ops::sync(&fx.ctx).unwrap();

    // Then the block resumes once and the review launches.
    assert_eq!(
        kinds(&fx, &id),
        ["automation_blocked", "automation_resumed", "review_started"]
    );
}
