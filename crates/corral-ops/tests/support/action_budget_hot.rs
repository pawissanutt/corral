//! The hot-actions kinds of the action budget (plan T-253 section 4, "Action budget"): the gate,
//! the reviewer through a pane and through the PM, the recovered auxiliary reviewer through a
//! pane and through the PM, Land and Carry, each taken by a real sync tick's autopilot. Their
//! stores reach the state just before the action through the real commands, before any growth.
use corral_core::{OperatingAuthority, OperatingSelection, Status, Verdict};

use super::Path;
use super::fixture::{self, Fx};

pub(super) const HOT_KINDS: [super::action_budget_fixture::Kind; 10] = {
    use super::action_budget_fixture::Kind;
    [
        Kind::Gate,
        Kind::ReviewerPane,
        Kind::ReviewerPm,
        Kind::RecoveredPane,
        Kind::RecoveredPm,
        Kind::Land,
        Kind::Carry,
        Kind::Steady,
        Kind::RepairTickLegacy,
        Kind::RepairTickInbox,
    ]
};

/// A Done prerequisite, added before any receipt so the receipts bind the final contract.
fn prerequisite(fx: &Fx, id: &str) {
    let dep = fx
        .ctx
        .store
        .create("Prerequisite quartz", Some("app"), Status::Done, "")
        .unwrap()
        .id;
    fx.ctx
        .store
        .update(id, |t| {
            t.depends_on.push(dep.clone());
            Ok(())
        })
        .unwrap();
}

fn finished(fx: &Fx) -> String {
    let task = fx.task("submission matrix", Status::Ready);
    prerequisite(fx, &task.id);
    // Ten owner comments: the emitted store's added ones keep the head's counts at one width.
    fx.ctx
        .store
        .update(&task.id, |t| {
            for i in 0..10 {
                t.comments.push(corral_core::TaskComment {
                    at: fixture::now(),
                    author: corral_core::CommentAuthor::Owner,
                    text: format!("Base note {i:02}"),
                });
            }
            Ok(())
        })
        .unwrap();
    fx.start(&task.id);
    fx.done(&task.id, "candidate.txt");
    task.id
}

/// The shared PM pane a PM review prompts: an existing one, so no bootstrap is measured.
fn pm_pane(fx: &mut Fx, id: &str) {
    let task = fx.ctx.store.get(id).unwrap();
    let herdr = task.current_run().and_then(|r| r.herdr.as_ref()).unwrap();
    corral_core::records::RecordArea::new("pm")
        .save(
            &fx.ctx.store,
            "state",
            &serde_json::json!({"pane_id": herdr.pane_id}),
        )
        .unwrap();
    fx.ctx.store.config.review.reviewer = Some("pm".into());
    fx.ctx.store.save_config().unwrap();
}

/// A passed gate on the run's head and an approval bound to it, by the configured reviewer.
fn approved(fx: &Fx, id: &str) {
    crate::gate_start(&fx.ctx, id).unwrap();
    assert!(crate::gate_collect(&fx.ctx, id).unwrap().is_some());
    pin_receipts(fx, id);
    let review = crate::review_run(&fx.ctx, id, Some("rev")).unwrap();
    let token = crate::review_callback_token(&review).unwrap();
    crate::verdict_bound(
        &fx.ctx,
        id,
        Verdict::Approve,
        "independently verified",
        Some(&token),
    )
    .unwrap();
    pin_verdict(fx, id);
}

/// An earlier review round of the run: a review that requested changes, the worker's fix and
/// its `corral done`. The run's current review child is that earlier review's.
fn earlier_round(fx: &Fx, id: &str) {
    crate::gate_start(&fx.ctx, id).unwrap();
    assert!(crate::gate_collect(&fx.ctx, id).unwrap().is_some());
    pin_receipts(fx, id);
    let review = crate::review_run(&fx.ctx, id, Some("rev")).unwrap();
    let token = crate::review_callback_token(&review).unwrap();
    crate::verdict_bound(
        &fx.ctx,
        id,
        Verdict::Changes,
        "fix the edge case",
        Some(&token),
    )
    .unwrap();
    pin_verdict(fx, id);
    fx.ctx
        .store
        .update(id, |t| {
            // The changes request sent the run back to its worker.
            t.status = Status::Running;
            let run = t.current_run_mut().unwrap();
            run.ended = None;
            run.outcome = None;
            Ok(())
        })
        .unwrap();
    fx.done(id, "fixed.txt");
}

/// A verdict stamps its record and the finished child with the process wall clock, whose
/// fraction serializes at a varying width: pin them as [`pin_receipts`] pins a gate's.
fn pin_verdict(fx: &Fx, id: &str) {
    let pin = |at: &mut jiff::Timestamp| *at = (*at).min(fixture::now());
    fx.ctx
        .store
        .update(id, |t| {
            let run = t.current_run_mut().unwrap();
            if let Some(review) = run.review.as_mut() {
                pin(&mut review.at);
            }
            for child in &mut run.review_runs {
                pin(&mut child.started);
                if let Some(ended) = child.ended.as_mut() {
                    pin(ended);
                }
                if let Some(review) = child.review.as_mut() {
                    pin(&mut review.at);
                }
            }
            Ok(())
        })
        .unwrap();
}

/// The gate script stamps its command receipts with its own process clock, whose fraction
/// serializes at a varying width: pin them, so every store's run documents have one size.
fn pin_receipts(fx: &Fx, id: &str) {
    fx.ctx
        .store
        .update(id, |t| {
            let run = t.current_run_mut().unwrap();
            if let Some(gate) = run.gate.as_mut() {
                let mut doc = serde_json::to_value(&gate.commands)?;
                for receipt in doc.as_array_mut().into_iter().flatten() {
                    for key in ["started", "ended"] {
                        if receipt.get(key).is_some_and(|v| v.is_string()) {
                            receipt[key] = serde_json::to_value(fixture::now())?;
                        }
                    }
                }
                gate.commands = serde_json::from_value(doc)?;
            }
            Ok(())
        })
        .unwrap();
}

/// The authority a kind's tick acts under: Carry's tick may not accept, so it stops after the
/// carried verdict instead of landing.
pub(super) fn grant(fx: &mut Fx, id: &str, kind: super::action_budget_fixture::Kind) {
    let accept = kind != super::action_budget_fixture::Kind::Carry;
    let selection = OperatingSelection {
        tasks: [id.to_owned()].into(),
        authority: OperatingAuthority {
            dispatch: true,
            accept,
            plan_accept: false,
            aggregate_accept: false,
        },
        ..Default::default()
    };
    let preview = fx.ctx.store.preview_operating_policy(&selection).unwrap();
    fx.ctx.store.config = fx
        .ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", fixture::now())
        .unwrap();
}

/// The steps a kind takes after growth and the grant: a recovered kind's never-sent attempt.
pub(super) fn prepared(fx: &Fx, id: &str, kind: super::action_budget_fixture::Kind) {
    use super::action_budget_fixture::Kind;
    match kind {
        Kind::RecoveredPane => super::refuse_prepared(fx, id, Path::ReviewerPane, true),
        Kind::RecoveredPm => super::refuse_prepared(fx, id, Path::ReviewerPm, true),
        _ => {}
    }
}

/// The kind's acted task in the state just before its action.
pub(super) fn setup(kind: super::action_budget_fixture::Kind) -> (Fx, String) {
    use super::action_budget_fixture::Kind;
    match kind {
        Kind::Gate | Kind::Steady => {
            let fx = Fx::new();
            let id = finished(&fx);
            // The started gate keeps running, so the tick's next step waits on it.
            std::fs::write(fx.bin.join("hold-gates"), "").unwrap();
            (fx, id)
        }
        Kind::ReviewerPane | Kind::ReviewerPm => {
            let mut fx = Fx::new();
            let id = finished(&fx);
            // The review config is final before the gate binds its policy.
            if kind == Kind::ReviewerPm {
                pm_pane(&mut fx, &id);
            }
            earlier_round(&fx, &id);
            crate::gate_start(&fx.ctx, &id).unwrap();
            assert!(crate::gate_collect(&fx.ctx, &id).unwrap().is_some());
            pin_receipts(&fx, &id);
            (fx, id)
        }
        Kind::RecoveredPane | Kind::RecoveredPm => {
            let path = if kind == Kind::RecoveredPm {
                Path::ReviewerPm
            } else {
                Path::ReviewerPane
            };
            // The refused attempt is prepared after growth and the grant ([`prepared`]): its
            // authorization binds the task's final contract.
            let (fx, id) = path.setup();
            prerequisite(&fx, &id);
            (fx, id)
        }
        Kind::Land => {
            let fx = Fx::new();
            let id = finished(&fx);
            approved(&fx, &id);
            (fx, id)
        }
        Kind::Carry => {
            let fx = Fx::new();
            let id = finished(&fx);
            approved(&fx, &id);
            // main moves on; corral's own clean replay rebases the approved candidate onto it,
            // and the integrated gate passes on the replayed head.
            let repo = fx.ctx.store.project_path("app").unwrap();
            std::fs::write(repo.join("upstream.txt"), "independent upstream change\n").unwrap();
            fixture::git(&repo, &["add", "-A"]);
            fixture::git(
                &repo,
                &["-c", "commit.gpgsign=false", "commit", "-qm", "upstream"],
            );
            let task = fx.ctx.store.get(&id).unwrap();
            let run = task.current_run().unwrap().clone();
            let base = corral_git::rev_sha(&repo, "main").unwrap();
            assert_eq!(
                crate::verdict_carry::rebase(&fx.ctx, &task, &run, &base).unwrap(),
                corral_git::RebaseOutcome::Rebased
            );
            crate::gate_start(&fx.ctx, &id).unwrap();
            assert!(crate::gate_collect(&fx.ctx, &id).unwrap().is_some());
            pin_receipts(&fx, &id);
            (fx, id)
        }
        Kind::RepairTickLegacy | Kind::RepairTickInbox => {
            let path = if kind == Kind::RepairTickInbox {
                Path::RepairInbox
            } else {
                Path::RepairLegacy
            };
            let (fx, id) = path.setup();
            prerequisite(&fx, &id);
            crate::gate_start(&fx.ctx, &id).unwrap();
            assert!(crate::gate_collect(&fx.ctx, &id).unwrap().is_some());
            pin_receipts(&fx, &id);
            let review = crate::review_run(&fx.ctx, &id, Some("rev")).unwrap();
            let token = crate::review_callback_token(&review).unwrap();
            crate::verdict_bound(
                &fx.ctx,
                &id,
                Verdict::Changes,
                "fix the edge case",
                Some(&token),
            )
            .unwrap();
            pin_verdict(&fx, &id);
            (fx, id)
        }
        other => panic!("{other:?} is not a hot-actions kind"),
    }
}
