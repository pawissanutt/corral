//! Receipt-level carry boundaries: no reviewer or gate command is launched.
use super::*;
use crate::gate_tier::{GatePhase, tests::Fx};

fn fixture(tiered: bool) -> (Fx, String, RecoveryRecord) {
    let mut f = crate::gate_tier::tests::fixture(tiered);
    f.ctx.store.config.review.reviewer = Some("gpt-sol".into());
    f.ctx.store.config.review.require_cross_family = Some(true);
    f.ctx.store.config.harnesses.insert(
        "fixture".into(),
        serde_json::from_value(
            serde_json::json!({"command":"true {model_args}","model_flag":"--model"}),
        )
        .unwrap(),
    );
    for (profile, model) in [("gpt-sol", "author-model"), ("gpt-astra", "review-model")] {
        f.ctx.store.config.profiles.insert(
            profile.into(),
            serde_json::from_value(
                serde_json::json!({"harness":"fixture","model":model,"vendor":"openai"}),
            )
            .unwrap(),
        );
    }
    f.ctx.store.save_config().unwrap();
    let id = f.main_bound().id;
    let author = policy::reviewer(&f.ctx.store.config, "gpt-sol").unwrap();
    f.ctx
        .store
        .update(&id, |task| {
            task.status = Status::Review;
            let run = task.current_run_mut().unwrap();
            run.agent = author.profile.clone();
            run.model = Some(author.model.clone());
            run.vendor = Some(author.vendor.clone());
            run.outcome = Some(RunOutcome::Done);
            Ok(())
        })
        .unwrap();
    let gate = receipt(&f, &id, GatePhase::Review, "quick-1", None);
    let input = RecoveryInput {
        action: RecoveryAction::UseReviewer {
            stage: RecoveryReviewStage::CodeReview,
            reviewer: policy::reviewer(&f.ctx.store.config, "gpt-astra").unwrap(),
        },
        exceptions: BTreeSet::from([RecoveryCheck::ReviewerFamilyPolicy]),
        notes: String::new(),
    };
    let preview = recover_preview(&f.ctx, &id, &input).unwrap();
    assert!(
        preview.remaining_blockers.is_empty(),
        "{:?}",
        preview.remaining_blockers
    );
    let record = RecoveryRecord {
        version: RECOVERY_VERSION,
        id: format!("REC-{}", preview.digest),
        authorization: confirmation(&preview).authorization,
        preview,
        outcomes: vec![
            outcome(RecoveryState::Authorized, "owner intent", None),
            outcome(RecoveryState::Applying, "separate review completed", None),
        ],
        links: vec![],
    };
    let (task, run) = f.current(&id);
    let identity = gate.identity.unwrap();
    let mut derived = f.ctx.clone();
    derived.store.config.review.require_cross_family = Some(false);
    let mut route = crate::review_contracts::implementation_route(
        &derived,
        &task,
        &RunHead::of(&run).unwrap(),
        &Reviewer::Profile("gpt-astra".into()),
        true,
        run.vendor.as_deref(),
        &identity,
    )
    .unwrap()
    .route;
    let RecoveryEffect::ReviewAttempt { attempt, .. } = &record.preview.proposed_effect else {
        panic!("review effect")
    };
    route.attempt = Some(attempt.clone());
    f.review(&id, Verdict::Approve, &identity);
    f.ctx
        .store
        .update(&id, |task| {
            let review = task.current_run_mut().unwrap().review.as_mut().unwrap();
            review.reviewer = "gpt-astra".into();
            review.route = Some(Box::new(route.clone()));
            Ok(())
        })
        .unwrap();
    (f, id, record)
}

pub(super) fn receipt(
    f: &Fx,
    id: &str,
    phase: GatePhase,
    attempt: &str,
    origin: Option<GatePromotion>,
) -> GateRecord {
    receipt_result(f, id, phase, attempt, origin, GateResult::Pass)
}

pub(super) fn receipt_result(
    f: &Fx,
    id: &str,
    phase: GatePhase,
    attempt: &str,
    origin: Option<GatePromotion>,
    state: GateResult,
) -> GateRecord {
    let mut gate = f.receipt(id, phase, attempt);
    let (task, run) = f.current(id);
    let run = RunHead::of(&run).unwrap();
    let selected = crate::gate_tier::select(&f.ctx, &task, &run, phase).unwrap();
    let command = selected.commands[0].clone();
    let range = format!("\n$ {command}\n");
    let log = format!("{range}corral gate: {}\n", state.as_str());
    gate.result = state;
    gate.failed_cmd = (state == GateResult::Fail).then(|| command.clone());
    gate.log_sha256 = Some(crate::verification::digest(log.as_bytes()));
    gate.commands = vec![CommandReceipt {
        command: command.clone(),
        exit_code: i32::from(state == GateResult::Fail),
        log_start: 0,
        log_end: range.len() as u64,
        started: Some(gate.at),
        ended: Some(gate.at),
    }];
    gate.tier = Some(Box::new(GateTierReceipt {
        tier: selected.tier.clone(),
        context_sha256: "fixture".into(),
        carried: None,
        promoted_from: origin,
    }));
    let start = crate::gate::evidence::put_start(
        &f.ctx,
        &crate::gate::evidence::Start {
            task: id,
            run: &run.id,
            attempt,
            scope: "run",
            sha: &gate.sha,
            tier: Some(&selected.tier),
            identity: gate.identity.as_ref().unwrap(),
            commands: &[command],
            context: None,
        },
    )
    .unwrap()
    .unwrap();
    f.ctx
        .store
        .update(id, |_| {
            start.stage(&f.ctx.store)?;
            f.ctx.store.log_event(
                Some(id),
                "gate_started",
                serde_json::json!({"run":run.id,"attempt":attempt}),
            )
        })
        .unwrap();
    let paths = crate::gate::gate_paths(&f.ctx, id, &run.id);
    std::fs::create_dir_all(paths.log.parent().unwrap()).unwrap();
    std::fs::write(&paths.log, log).unwrap();
    let result = crate::gate::evidence::put_result(&f.ctx, id, &run.id, attempt, "fixture", &paths)
        .unwrap()
        .unwrap();
    f.ctx
        .store
        .update(id, |task| {
            task.current_run_mut().unwrap().gate = Some(gate.clone());
            result.stage(&f.ctx.store, (state, gate.failed_cmd.as_deref()))?;
            f.ctx.store.log_event(
                Some(id),
                "gate",
                serde_json::json!({"run":run.id,"attempt":attempt,"result":state.as_str()}),
            )
        })
        .unwrap();
    gate
}

#[test]
fn recovered_approval_is_not_rebound_by_carry_or_promotion() {
    if isolated("rebinding::recovered_approval_is_not_rebound_by_carry_or_promotion") {
        return;
    }
    // Two receipt mechanisms, each with a positive control on the exact same fixture.
    for promotion in [false, true] {
        let (f, id, record) = fixture(promotion);
        let (task, run) = f.current(&id);
        let original = run.review.clone().unwrap();
        let origin = promotion.then(|| GatePromotion {
            identity: original.identity.clone().unwrap(),
            gate_at: run.gate.as_ref().unwrap().at,
        });
        if !promotion {
            std::fs::write(f.repo.join("unrelated.txt"), "main moved\n").unwrap();
            corral_git::commit_all(&f.repo, "main moves").unwrap();
            let base = corral_git::rev_sha(&f.repo, "main").unwrap();
            crate::verdict_carry::rebase(&f.ctx, &task, &RunHead::of(&run).unwrap(), &base)
                .unwrap();
        }
        f.ctx
            .store
            .update(&id, |task| {
                task.current_run_mut().unwrap().promotion_origin = origin.clone().map(Box::new);
                Ok(())
            })
            .unwrap();
        receipt(
            &f,
            &id,
            if promotion {
                GatePhase::Landing
            } else {
                GatePhase::Review
            },
            "next",
            origin.clone(),
        );
        let carry = || {
            if promotion {
                crate::verdict_carry::promote(&f.ctx, &id)
            } else {
                crate::verdict_carry::carry(&f.ctx, &id)
            }
        };
        assert!(carry().unwrap(), "positive control: promotion={promotion}");
        f.ctx
            .store
            .update(&id, |task| {
                let run = task.current_run_mut().unwrap();
                run.review = Some(original.clone());
                run.promotion_origin = origin.clone().map(Box::new);
                run.completion.evidence_carry = None;
                Ok(())
            })
            .unwrap();
        f.ctx.store.save_recovery_record(&id, &record).unwrap();
        let before = f.ctx.store.fixture_task_text(&id).unwrap();
        let events = f.ctx.store.events().unwrap().len();
        assert!(
            !carry().unwrap(),
            "recovered approval rebound: promotion={promotion}"
        );
        assert_eq!(f.ctx.store.fixture_task_text(&id).unwrap(), before);
        assert_eq!(f.ctx.store.events().unwrap().len(), events);
        assert_eq!(f.current(&id).1.review.unwrap(), original);
    }
}

#[test]
fn ordinary_approval_refuses_recovered_review_before_preparation() {
    if isolated("rebinding::ordinary_approval_refuses_recovered_review_before_preparation") {
        return;
    }
    let (f, id, record) = fixture(false);
    f.ctx.store.save_recovery_record(&id, &record).unwrap();
    std::fs::write(f.repo.join("other.txt"), "unrelated main advance\n").unwrap();
    corral_git::commit_all(&f.repo, "main moves").unwrap();
    let run = f.current(&id).1;
    let before = f.ctx.store.fixture_task_text(&id).unwrap();
    let head = corral_git::rev_sha(&run.worktree, "HEAD").unwrap();
    let events = f.ctx.store.events().unwrap().len();
    assert!(crate::approve(&f.ctx, &id, crate::ApproveStrategy::Rebase, false, true).is_err());
    assert_eq!(corral_git::rev_sha(&run.worktree, "HEAD").unwrap(), head);
    assert_eq!(f.ctx.store.fixture_task_text(&id).unwrap(), before);
    assert_eq!(f.ctx.store.events().unwrap().len(), events);
}
