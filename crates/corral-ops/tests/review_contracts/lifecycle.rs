//! R1 re-review regressions: callback ownership across reviewer replacement, the stronger
//! transition across restart and recovery, and route validation on retained resume.
use super::*;

fn stronger(fx: &Fx, id: &str, record: &ReviewRecord) -> StrongerOutcome {
    corral_ops::request_stronger_review(
        &fx.ctx,
        id,
        &attempt(record),
        "The lock ordering argument needs design-level analysis.",
    )
    .unwrap()
}

fn stale(result: anyhow::Result<ReviewRecord>) {
    let error = result.unwrap_err();
    assert!(format!("{error:#}").contains("stale review"), "{error:#}");
}

#[test]
fn ordinary_replacement_rejects_the_previous_reviewer_callback() {
    let fx = fixture(|_| {});
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    let first = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    let replaced = corral_ops::review_run(&fx.ctx, &id, Some("design")).unwrap();
    assert_eq!(replaced.reviewer, "design");
    assert_ne!(attempt(&replaced), attempt(&first));
    stale(corral_ops::verdict_bound(
        &fx.ctx,
        &id,
        Verdict::Approve,
        "old",
        Some(&attempt(&first)),
    ));
    let same = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    assert_eq!(same.reviewer, "impl");
    stale(corral_ops::verdict_bound(
        &fx.ctx,
        &id,
        Verdict::Approve,
        "old",
        Some(&attempt(&replaced)),
    ));
    assert_eq!(recorded(&fx, &id).verdict, None);
    let saved =
        corral_ops::verdict_bound(&fx.ctx, &id, Verdict::Changes, "fix", Some(&attempt(&same)))
            .unwrap();
    assert_eq!(saved.verdict, Some(Verdict::Changes));
}

fn strong_review(fx: &Fx, id: &str) -> (String, ReviewRecord) {
    let routine = corral_ops::review_run(&fx.ctx, id, None).unwrap();
    let StrongerOutcome::Rerouted { superseded } = stronger(fx, id, &routine) else {
        panic!("expected reroute");
    };
    (
        superseded,
        corral_ops::review_run(&fx.ctx, id, None).unwrap(),
    )
}

#[test]
fn restarted_strong_review_stays_strong_and_cannot_escalate_again() {
    let fx = fixture(|_| {});
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    let (superseded, _strong) = strong_review(&fx, &id);
    let restarted = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    let route = route(&restarted);
    assert_eq!(restarted.reviewer, "design");
    assert_eq!(route.requirement.capability, ReviewCapability::Strong);
    assert_eq!(route.escalated_from.as_deref(), Some(superseded.as_str()));
    let again = stronger(&fx, &id, &restarted);
    assert!(
        matches!(again, StrongerOutcome::Escalated { .. }),
        "{again:?}"
    );
}

#[test]
fn recovered_lost_reviewer_keeps_the_strong_requirement() {
    let fx = fixture(|_| {});
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    let (superseded, _strong) = strong_review(&fx, &id);
    fx.ctx
        .store
        .update(&id, |t| {
            let run = t.current_run_mut().unwrap();
            run.review.as_mut().unwrap().process_lost = true;
            Ok(())
        })
        .unwrap();
    let recovered = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    assert_eq!(
        route(&recovered).requirement.capability,
        ReviewCapability::Strong
    );
    assert_eq!(
        route(&recovered).escalated_from.as_deref(),
        Some(superseded.as_str())
    );
}

#[test]
fn changed_candidate_starts_with_its_own_routine_assessment() {
    let fx = fixture(|_| {});
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    let (_superseded, _strong) = strong_review(&fx, &id);
    let worktree = fx
        .ctx
        .store
        .get(&id)
        .unwrap()
        .current_run()
        .unwrap()
        .worktree
        .clone();
    std::fs::write(worktree.join("src/widget.rs"), "changed again\n").unwrap();
    corral_git::commit_all(&worktree, "new candidate").unwrap();
    let fresh = corral_ops::review_run(&fx.ctx, &id, None).unwrap();
    assert_eq!(
        route(&fresh).requirement.capability,
        ReviewCapability::Routine
    );
    assert_eq!(route(&fresh).escalated_from, None);
}

fn install_hook(fx: &Fx, name: &str, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    let hook = fx._tmp.path().join(name);
    std::fs::write(&hook, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn assessment_refused(result: anyhow::Result<ReviewRecord>) {
    let error = result.unwrap_err();
    assert!(
        format!("{error:#}").contains("review assessment changed"),
        "{error:#}"
    );
}

#[test]
fn retained_dedicated_review_is_not_resubmitted_after_its_route_changed() {
    let fx = fixture(|_| {});
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    let config = fx.ctx.store.dir().join("corral.toml");
    install_hook(
        &fx,
        "on-report",
        &format!("sed -i 's/= \"impl\"/= \"design\"/' '{}'", config.display()),
    );
    assessment_refused(corral_ops::review_run(&fx.ctx, &id, None));
    assert!(fx._tmp.path().join("on-report.done").exists());
    assessment_refused(corral_ops::review_run(&fx.ctx, &id, None));
    let events = fx.ctx.store.events().unwrap();
    assert!(!events.iter().any(|e| e.kind == "review_started"));
}

#[test]
fn retained_pm_review_stays_refused_after_its_assessment_inputs_changed() {
    let mut fx = fixture(|config| config.pm.agent = "impl".into());
    let id = candidate(&fx, "logic", &[], "src/widget.rs");
    std::fs::write(
        fx.ctx.store.state_dir().join("pm.json"),
        r#"{"pane_id":"w9:p9"}"#,
    )
    .unwrap();
    let tasks = fx.ctx.store.dir().join("tasks");
    install_hook(
        &fx,
        "on-pane-get",
        &format!(
            "sed -i 's/^  kind: logic$/  kind: ui/' '{}'/{id}-*.md",
            tasks.display()
        ),
    );
    fx.ctx.store.reload_config_if_changed().unwrap();
    let first = corral_ops::review_run(&fx.ctx, &id, Some("pm"));
    assert!(fx._tmp.path().join("on-pane-get.done").exists());
    assert_eq!(fx.ctx.store.get(&id).unwrap().kind.as_deref(), Some("ui"));
    let refused = format!("{:#}", first.unwrap_err());
    assert!(refused.contains("preparation retained"), "{refused}");
    let resumed = corral_ops::review_run(&fx.ctx, &id, Some("pm"));
    assert!(resumed.is_err(), "refused PM preparation was resubmitted");
    let events = fx.ctx.store.events().unwrap();
    assert!(!events.iter().any(|e| e.kind == "review_started"));
}
