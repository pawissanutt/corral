//! G7: in the same tiered Plan, a run template that says "Run the full gate" refuses dispatch with
//! no run created; a review template and then a reviewer profile's instructions that demand it
//! refuse every review launch, explicit and automatic; none of these fixtures lands.
use std::collections::BTreeSet;

use super::plan::{dispatch, imported, summary};
use super::*;
use corral_core::{
    Harness, Profile, ProfileEligibility, ProfileMetadata, ProfileRole, SelectionGuidance, Status,
};
use corral_ops::{DispatchOpts, GateInstructionConflict};

fn conflict(error: &anyhow::Error) -> &GateInstructionConflict {
    error
        .downcast_ref::<GateInstructionConflict>()
        .unwrap_or_else(|| panic!("not an instruction conflict: {error:#}"))
}

fn blocked(fx: &Fx, id: &str) -> Value {
    let blocks: Vec<Value> = fx
        .events(id, "automation_blocked")
        .into_iter()
        .filter(|event| event.get("conflict").is_some())
        .collect();
    blocks
        .last()
        .unwrap_or_else(|| panic!("no instruction block for {id}: {:?}", fx.run(id).autopilot))["conflict"]
        .clone()
}

fn submit(fx: &Fx, id: &str) {
    let text = summary("Added the widget.");
    let files = [(format!("{id}.txt"), "w\n")];
    let files: Vec<(&str, &str)> = files.iter().map(|(f, t)| (f.as_str(), *t)).collect();
    work(
        fx,
        id,
        "quick",
        &Round {
            files: &files,
            summary: &text,
        },
    );
    assert_eq!(fx.task(id).status, Status::Review);
}

fn strict_reviewer() -> Profile {
    Profile {
        harness: "h".into(),
        model: Some("strict-model".into()),
        vendor: Some("other-vendor".into()),
        effort: None,
        args: vec![],
        escalate_to: None,
        metadata: Some(ProfileMetadata {
            display_name: "Strict".into(),
            purpose: "Review".into(),
            guidance: SelectionGuidance::default(),
            instructions: "Check the diff.\nBefore approving, run the full gate.".into(),
            eligibility: ProfileEligibility {
                enabled: true,
                automatic: true,
                roles: BTreeSet::from([ProfileRole::ImplementationReview]),
                projects: BTreeSet::new(),
            },
            revision: "r1".into(),
            evidence: None,
            candidates: vec![],
            fallback: None,
        }),
    }
}

#[test]
fn full_gate_directives_refuse_dispatch_then_each_review_launch_and_nothing_lands() {
    let mut p = imported(false);
    let (a, b) = (p.a.clone(), p.b.clone());
    let main = git(&p.fx.app, &["rev-parse", "main"]);
    let prompts = p.fx.ctx.store.prompts_dir();
    std::fs::create_dir_all(&prompts).unwrap();

    // Given a run.md override containing "Run the full gate".
    std::fs::write(
        prompts.join("run.md"),
        "# {{id}} {{title}}\n{{body}}\nRun the full gate before `done`.\n{{gate}}\n",
    )
    .unwrap();
    // When the member is dispatched, then dispatch is refused and no run exists.
    let error = corral_ops::dispatch(&p.fx.ctx, &a, &DispatchOpts::default()).unwrap_err();
    let found = conflict(&error);
    assert_eq!(
        (found.surface.as_str(), found.line, found.phrase.as_str()),
        ("run_template", 3, "Run the full gate")
    );
    assert!(found.path.ends_with("prompts/run.md"), "{}", found.path);
    assert!(p.fx.task(&a).runs.is_empty());
    assert!(!p.fx.app.join(".worktrees").join(&a).exists());
    assert!(p.fx.events(&a, "dispatched").is_empty());

    // Given the clean built-in template, both members are dispatched; with a review.md override
    // that demands the full gate, the first submits its quick round.
    std::fs::remove_file(prompts.join("run.md")).unwrap();
    std::fs::write(
        prompts.join("review.md"),
        "# Review {{id}}\n\nRun the full gate and approve only if it passes.\n",
    )
    .unwrap();
    dispatch(&p.fx, &a);
    dispatch(&p.fx, &b);
    let target = p.fx.run(&a).plan_target.unwrap().branch;
    let target_head = git(&p.fx.app, &["rev-parse", &target]);
    submit(&p.fx, &a);
    // When a review is launched explicitly, then it is refused before a reviewer starts.
    let error = corral_ops::review_run(&p.fx.ctx, &a, None).unwrap_err();
    let found = conflict(&error);
    assert_eq!((found.surface.as_str(), found.line), ("review_template", 3));
    // When autopilot routes the review, then it is refused and reported.
    p.fx.sync();
    assert_eq!(blocked(&p.fx, &a)["surface"], "review_template");
    assert!(p.fx.events(&a, "review_started").is_empty());

    // Given a clean review template and a configured reviewer profile whose instructions demand
    // it, the second member submits its quick round.
    std::fs::remove_file(prompts.join("review.md")).unwrap();
    let config = &mut p.fx.ctx.store.config;
    config.harnesses.insert(
        "h".into(),
        Harness {
            command: "true {model_args}".into(),
            flags: Some("claude".into()),
            model_flag: None,
            effort_flag: None,
            efforts: vec![],
            prompt: None,
        },
    );
    config.profiles.insert("strict".into(), strict_reviewer());
    config.review.reviewer = Some("strict".into());
    p.fx.ctx.store.save_config().unwrap();
    // The changed reviewer policy voids the grant; the owner confirms it again.
    let tracker = p.tracker.clone();
    p.fx.grant(&[&tracker]);
    submit(&p.fx, &b);
    // When it is named explicitly, then the launch is refused naming the profile and line.
    let error = corral_ops::review_run(&p.fx.ctx, &b, Some("strict")).unwrap_err();
    let found = conflict(&error);
    assert_eq!(
        (found.surface.as_str(), found.path.as_str(), found.line),
        (
            "reviewer_instructions",
            "profiles.strict.metadata.instructions",
            2
        )
    );
    // When autopilot routes to it as the configured reviewer, then that launch is refused too.
    p.fx.sync();
    assert_eq!(blocked(&p.fx, &b)["surface"], "reviewer_instructions");

    // Then nothing was reviewed, approved or landed.
    for id in [&a, &b] {
        assert!(p.fx.events(id, "review_started").is_empty(), "{id}");
        assert!(p.fx.run(id).review.is_none(), "{id}");
        assert!(p.fx.events(id, "approved").is_empty(), "{id}");
        assert_ne!(p.fx.task(id).status, Status::Done, "{id}");
    }
    assert_eq!(git(&p.fx.app, &["rev-parse", "main"]), main);
    assert_eq!(git(&p.fx.app, &["rev-parse", &target]), target_head);
}
