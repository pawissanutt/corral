//! Promotion refusals: an input that changed since the quick approval, a replaced landing gate
//! whose origin no longer matches, a full command failure and missing evidence never carry the
//! approval or the evidence, and no Git ref moves.
use super::tiers::{Mode, approved, events_of, observe, runs, tiered};
use super::*;
use corral_core::CandidateIdentity;
use serde_json::json;
use sha2::Digest;

fn assert_not_carried(fx: &Fx, main: &str, case: &str) {
    assert!(events_of(fx, "verdict_carried").is_empty(), "{case}");
    assert!(events_of(fx, "approved").is_empty(), "{case}");
    assert_eq!(git(&fx.app, &["rev-parse", "main"]), main, "{case}");
    assert_ne!(fx.task().status, Status::Done, "{case}");
    assert_eq!(fx.run().completion.evidence_carry, None, "{case}");
}

fn edit_review(fx: &Fx, edit: impl FnOnce(&mut ReviewOverride)) {
    let mut store = fx.ctx.store.clone();
    edit(&mut store.config.review);
    store.save_config().unwrap();
}

fn edit_run(fx: &Fx, edit: impl FnOnce(&mut Run)) {
    fx.ctx
        .store
        .update(&fx.id, |task| {
            edit(task.current_run_mut().unwrap());
            Ok(())
        })
        .unwrap();
}

/// A receipt identity made by another corral binary: its inputs digest no longer re-derives.
fn older_binary(fx: &mut Fx, _: &Path) {
    edit_run(fx, |run| {
        let stamp = |identity: &mut CandidateIdentity| identity.inputs_digest = "0".repeat(64);
        stamp(run.gate.as_mut().unwrap().identity.as_mut().unwrap());
        run.completion
            .evidence
            .iter_mut()
            .for_each(|record| stamp(&mut record.identity));
        let ledger = serde_json::to_vec(&run.completion.evidence).unwrap();
        let review = run.review.as_mut().unwrap();
        stamp(review.identity.as_mut().unwrap());
        review.evidence_digest = Some(format!("{:x}", sha2::Sha256::digest(&ledger)));
    });
}

#[test]
fn inputs_changed_between_approval_and_the_full_gate_refuse_the_carry() {
    type Change = fn(&mut Fx, &Path);
    let cases: [(&str, Change); 10] = [
        ("full gate commands", |fx, _| {
            edit_review(fx, |r| r.gate.as_mut().unwrap()[0].push_str(" && true"));
        }),
        ("quick tier commands", |fx, _| {
            edit_review(fx, |r| {
                let tiers = r.gate_tiers.as_mut().unwrap();
                tiers.get_mut("quick").unwrap()[0].push_str(" && true");
            });
        }),
        ("tier_policy.review_rounds", |fx, _| {
            edit_review(fx, |r| {
                r.tier_policy.as_mut().unwrap().review_rounds = Some("full".into());
            });
        }),
        ("reviewer policy", |fx, _| {
            edit_review(fx, |r| r.reviewer = Some("fake".into()));
        }),
        ("task body", |fx, _| {
            fx.ctx
                .store
                .update(&fx.id, |task| {
                    task.body.push_str("Also polish it.\n");
                    Ok(())
                })
                .unwrap();
            // An edited contract voids the grant; the owner grants the edited task again.
            fx.grant(std::slice::from_ref(&fx.id.clone()));
        }),
        ("new evidence record", |fx, out| {
            let attempt = fx.run().gate.unwrap().identity.unwrap().attempt;
            observe(fx, out, &attempt, 9);
        }),
        ("carried observation bytes", |fx, _| {
            edit_run(fx, |run| {
                run.completion.evidence[0].observation.observed = "edited".into();
            });
        }),
        ("carried artifact bytes", |fx, _| {
            let path = fx.run().completion.evidence[0].observation.artifacts[0].clone();
            let file = fx.ctx.store.dir().join(path);
            std::fs::remove_file(&file).unwrap();
            std::fs::write(&file, "edited\n").unwrap();
        }),
        ("verification input .corral/prompts/run.md", |fx, _| {
            let file = fx.ctx.store.prompts_dir().join("run.md");
            let mut text = std::fs::read_to_string(&file).unwrap();
            text.push_str("Keep commits small.\n");
            std::fs::write(file, text).unwrap();
        }),
        ("binary version", older_binary),
    ];
    for (case, change) in cases {
        // Given an audited quick approval whose landing gate has not run yet.
        let (mut fx, out) = tiered(Mode::Quick, true, |r| r.accept = Some(Accept::Auto));
        approved(&fx, &out);
        let main = git(&fx.app, &["rev-parse", "main"]);
        // When one input changes before the full gate.
        change(&mut fx, &out);
        fx.sync();
        // Then nothing is carried: a new review starts or the evidence failure is reported.
        assert_not_carried(&fx, &main, case);
        let reviewed = events_of(&fx, "review_started").len() == 2;
        let reported = fx.stage() == Some(AutoStage::Escalated)
            && fx.reason().contains("required evidence missing or stale");
        assert!(
            reviewed || reported,
            "{case}: {} {:?} {:?} {:?}",
            fx.reason(),
            fx.transitions(),
            runs(&out),
            corral_ops::automation_blocker(&fx.ctx, &fx.task())
        );
    }
}

#[test]
fn a_full_receipt_with_another_environment_is_never_promoted_onto() {
    let (fx, out) = tiered(Mode::Quick, true, |r| r.accept = Some(Accept::Owner));
    let (_, origin) = approved(&fx, &out);
    corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
    let other = corral_ops::gate_wait(&fx.ctx, &fx.id).unwrap();
    assert_eq!(other.result, GateResult::Pass);
    // Given a full receipt whose recorded environment is not the approved one.
    edit_run(&fx, |run| {
        let identity = run.gate.as_mut().unwrap().identity.as_mut().unwrap();
        let session = identity
            .environment
            .iter_mut()
            .find(|(name, _)| name == "CORRAL_HERDR_SESSION")
            .unwrap();
        session.1 = Some("another-shell".into());
    });
    corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, false, true).unwrap();
    // Then that receipt was invalidated, and only a gate with the approved environment promotes.
    let attempt = other.identity.unwrap().attempt;
    let invalidated = events_of(&fx, "gate_invalidated");
    assert_eq!(invalidated.len(), 1);
    assert_eq!(invalidated[0]["identity"]["attempt"], attempt.as_str());
    let carried = events_of(&fx, "verdict_carried");
    assert_eq!(carried.len(), 1);
    let target = &carried[0]["new_identity"];
    assert_ne!(target["attempt"], attempt.as_str());
    assert_eq!(target["environment"], json!(origin.identity.environment));
    assert_eq!(runs(&out), ["quick", "full", "full"]);
}

#[test]
fn replaced_landing_gates_refuse_the_carry_when_the_origin_or_inputs_no_longer_match() {
    type Change = fn(&Fx, &Path);
    let cases: [(&str, Change); 5] = [
        ("approval changed after B", |fx, _| {
            let b = fx.run().gate.unwrap();
            edit_run(fx, |run| {
                let review = run.review.as_mut().unwrap();
                review.identity = b.identity.clone();
                review.sha = b.sha.clone();
            });
        }),
        ("origin differs from the approval", |fx, _| {
            edit_run(fx, |run| {
                let origin = run.promotion_origin.as_mut().unwrap();
                origin.identity.attempt = "another-attempt".into();
            });
        }),
        ("B failed its full commands", |fx, out| {
            let gate = corral_ops::gate_collect(&fx.ctx, &fx.id).unwrap().unwrap();
            assert_eq!(gate.result, GateResult::Fail);
            assert_eq!(fx.run().promotion_origin, None);
            std::fs::remove_file(out.join("full-fails")).unwrap();
        }),
        ("disallowed input changed", |fx, _| {
            let file = fx.ctx.store.prompts_dir().join("run.md");
            std::fs::write(file, "{{id}}\n{{gate}}\n").unwrap();
        }),
        ("origin differs from C's copy", |_, _| {}),
    ];
    for (case, change) in cases {
        // Given landing gate B of an audited quick approval, replaced by C before promotion.
        let (fx, out) = tiered(Mode::Quick, true, |r| r.accept = Some(Accept::Owner));
        let (_, origin) = approved(&fx, &out);
        let main = git(&fx.app, &["rev-parse", "main"]);
        if case == "B failed its full commands" {
            std::fs::write(out.join("full-fails"), "").unwrap();
        }
        corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
        change(&fx, &out);
        corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
        let c = corral_ops::gate_wait(&fx.ctx, &fx.id).unwrap();
        assert_eq!(c.result, GateResult::Pass, "{case}: {:?}", c.failed_cmd);
        if case == "origin differs from C's copy" {
            assert_eq!(c.tier.unwrap().promoted_from, Some(origin));
            edit_run(&fx, |run| {
                let tier = run.gate.as_mut().unwrap().tier.as_mut().unwrap();
                let copy = tier.promoted_from.as_mut().unwrap();
                copy.gate_at = copy
                    .gate_at
                    .checked_sub(jiff::SignedDuration::from_secs(1))
                    .unwrap();
            });
        }
        // When the owner approves C.
        let error = corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, false, true)
            .unwrap_err();
        // Then nothing is carried and a new review is required.
        assert!(
            format!("{error:#}").contains("corral review-run"),
            "{case}: {error:#}"
        );
        assert_not_carried(&fx, &main, case);
    }
    // An audited required criterion with no record: the empty ledger reports evidence_missing.
    let (fx, out) = tiered(Mode::Quick, true, |r| r.accept = Some(Accept::Auto));
    approved(&fx, &out);
    let main = git(&fx.app, &["rev-parse", "main"]);
    edit_run(&fx, |run| {
        run.completion.evidence.clear();
        let review = run.review.as_mut().unwrap();
        review.evidence_digest = Some(format!("{:x}", sha2::Sha256::digest(b"[]")));
    });
    fx.sync();
    assert_not_carried(&fx, &main, "empty ledger");
    assert_eq!(runs(&out), ["quick", "full"]);
    assert_eq!(fx.stage(), Some(AutoStage::Escalated));
    assert!(
        fx.reason().contains("evidence_missing:C1"),
        "{}",
        fx.reason()
    );
}

#[test]
fn a_quick_approval_is_never_promoted_onto_a_full_fail() {
    // Given a quick approval whose landing full gate fails on the same head.
    let (fx, out) = tiered(Mode::Quick, true, |r| r.accept = Some(Accept::Owner));
    approved(&fx, &out);
    let main = git(&fx.app, &["rev-parse", "main"]);
    std::fs::write(out.join("full-fails"), "").unwrap();
    for _ in 0..2 {
        let error = corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, false, true)
            .unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains("promotion needs a full PASS"), "{error}");
        assert!(error.contains("corral review-run"), "{error}");
        assert_not_carried(&fx, &main, "full fail");
    }
    assert_eq!(runs(&out), ["quick", "full"]);
    // When a review bound to the failed full receipt completes, the owner may override it.
    let failed = fx.run().gate.unwrap();
    corral_ops::review_run(&fx.ctx, &fx.id, None).unwrap();
    fx.verdict(Verdict::Approve, "The known full failure is acceptable.");
    assert_eq!(fx.run().review.unwrap().identity, failed.identity);
    corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, false, true).unwrap();
    assert_eq!(fx.task().status, Status::Done);
    assert!(events_of(&fx, "verdict_carried").is_empty());
    let landed = events_of(&fx, "approved").pop().unwrap();
    assert_eq!(landed["acceptance_mode"], "owner_override");
    assert_eq!(git(&fx.app, &["rev-parse", "main"]), failed.sha);

    // A full FAIL on a head produced by corral's replay stays refused, even after a review.
    let (fx, out) = tiered(Mode::Quick, true, |r| r.accept = Some(Accept::Owner));
    approved(&fx, &out);
    fx.commit_on_main("other.txt", "main moved\n", "main advanced");
    let main = git(&fx.app, &["rev-parse", "main"]);
    std::fs::write(out.join("full-fails"), "").unwrap();
    let error =
        corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::Rebase, false, true).unwrap_err();
    assert!(
        format!("{error:#}").contains("integrated gate failed"),
        "{error:#}"
    );
    for review in [false, true] {
        if review {
            corral_ops::review_run(&fx.ctx, &fx.id, None).unwrap();
            fx.verdict(Verdict::Approve, "Reviewed the failed replay.");
        }
        let error =
            corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::Rebase, false, true).unwrap_err();
        assert!(
            format!("{error:#}").contains("gate failed; inspect"),
            "{error:#}"
        );
        assert_not_carried(&fx, &main, "full fail after replay");
    }
}
