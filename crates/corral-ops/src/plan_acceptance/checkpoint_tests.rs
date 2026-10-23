use std::collections::BTreeMap;

use corral_core::{ExpectedFailure, GateResult, MemberGateAmendment, ScopeGate, TierPolicy};

use super::{plan_checkpoint, plan_checkpoint_automatic, start_due_with};
use crate::plan_acceptance::tests::{Delivery, Fixture, git};
use crate::pm_next::{PmActionKind, PmObservation, observe::observations, pm_card};

const SEEN: &str = r#"dir=$(dirname "$CORRAL_GATE_CONTEXT") && cp "$CORRAL_GATE_CONTEXT" "$dir/seen.json" && echo "$CORRAL_GATE_TIER" > "$dir/seen.tier""#;

fn refs(f: &Fixture) -> String {
    git(
        &f.repo,
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    )
}

fn tiered(f: &Fixture, every: Option<u32>) {
    let mut store = f.ctx.store.clone();
    store.config.review.gate_tiers = Some(BTreeMap::from([(
        "quick".to_owned(),
        vec!["true".to_owned()],
    )]));
    store.config.review.tier_policy = Some(TierPolicy {
        plan_member: Some("quick".into()),
        review_rounds: None,
    });
    store.config.review.plan_checkpoint_every = every;
    store.save_config().unwrap();
}

/// The context autopilot passes: its store re-reads the saved configuration.
fn fresh(f: &Fixture) -> crate::Ctx {
    crate::Ctx {
        store: f.ctx.store.reopen().unwrap(),
        ..f.ctx.clone()
    }
}

fn checkpoint_cards(f: &Fixture) -> Vec<String> {
    observations(&f.ctx)
        .unwrap()
        .into_iter()
        .filter_map(|observation| match observation {
            PmObservation::Owner { task, reason }
                if task == f.plan && reason.contains("checkpoint") =>
            {
                Some(reason)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn checkpoint_runs_full_on_the_target_head_and_records_an_advisory_attempt() {
    // Given a tiered integration Plan whose members declared an expected failure.
    let f = Fixture::new(Delivery::Integration, &[SEEN]);
    tiered(&f, None);
    let gate = ScopeGate {
        tier: "quick".into(),
        tests: Vec::new(),
        expected_failures: vec![ExpectedFailure {
            test: "app::suite still_failing".into(),
            reason: "restored later".into(),
        }],
    };
    for member in &f.members {
        let amendment = MemberGateAmendment {
            member: member.clone(),
            expected_revision: 0,
            gate: gate.clone(),
            reason: "declared".into(),
            by: "owner".into(),
        };
        f.ctx
            .store
            .amend_member_gate(&f.plan, &amendment, |_, _| Ok(()), |_, _| Ok(None))
            .unwrap();
    }
    let before = refs(&f);
    let head = git(&f.repo, &["rev-parse", "refs/heads/feature/plan"]);
    // When the owner runs a checkpoint.
    let attempt = plan_checkpoint(&f.ctx, &f.plan).unwrap();
    // Then the full tier ran with an empty expected-failure set on the target head.
    assert_eq!(attempt.id, "C1");
    assert!(attempt.advisory && !attempt.automatic);
    assert_eq!(attempt.gate.result, GateResult::Pass);
    assert_eq!(
        (attempt.head.as_str(), attempt.landings),
        (head.as_str(), 2)
    );
    let dir = f.ctx.store.state_dir().join("plan-gates");
    assert_eq!(
        std::fs::read_to_string(dir.join("seen.tier")).unwrap(),
        "full\n"
    );
    let context: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("seen.json")).unwrap()).unwrap();
    assert_eq!(context["tier"], "full");
    assert_eq!(context["expected_failures"], serde_json::json!([]));
    assert_eq!(attempt.gate.tier.as_ref().unwrap().tier, "full");
    let verify = f.repo.join(".worktrees").join(format!("{}-verify", f.plan));
    assert_eq!(git(&verify, &["rev-parse", "HEAD"]), head);
    // And it moves no ref and never becomes an aggregate attempt or PASS.
    assert_eq!(refs(&f), before);
    let tracker = f.ctx.store.get(&f.plan).unwrap();
    assert!(tracker.aggregate_attempts.is_empty());
    let progress = crate::plan_progress(&f.ctx, &f.plan).unwrap();
    let aggregate = progress.aggregate.unwrap();
    assert_eq!(
        (aggregate.attempt, aggregate.accepted),
        (None, false),
        "a green checkpoint is not an aggregate attempt"
    );
    let error = crate::plan_accept(&f.ctx, &f.plan, "C1", false).unwrap_err();
    assert!(
        format!("{error:#}").contains("no aggregate attempt"),
        "{error:#}"
    );
    let events = f.ctx.store.events().unwrap();
    let event = events
        .iter()
        .rev()
        .find(|event| event.kind == "plan_checkpoint")
        .unwrap();
    assert_eq!(event.detail["advisory"], true);
    assert_eq!(event.detail["result"], "pass");
    assert_eq!(event.detail["tier"], "full");
}

#[test]
fn checkpoint_refuses_a_main_delivery_plan() {
    let f = Fixture::new(Delivery::Main, &["true"]);
    let error = plan_checkpoint(&f.ctx, &f.plan).unwrap_err();
    assert!(
        format!("{error:#}").contains("integration target"),
        "{error:#}"
    );
}

#[test]
fn red_checkpoint_raises_one_pm_card_with_a_prepared_command() {
    // Given a full gate whose unit-test step fails while a marker file exists.
    let marker = tempfile::NamedTempFile::new().unwrap();
    let red = format!(
        "printf '==> tier full\\n\\n==> unit tests\\n' && test ! -e {}",
        marker.path().display()
    );
    let f = Fixture::new(Delivery::Integration, &[&red]);
    let tasks = f.ctx.store.list().unwrap();
    // When a checkpoint runs red.
    let attempt = plan_checkpoint(&f.ctx, &f.plan).unwrap();
    assert_eq!(attempt.gate.result, GateResult::Fail);
    assert_eq!(attempt.failed_step.as_deref(), Some("unit tests"));
    let log = attempt.log.clone().unwrap();
    let command = attempt.fix_command.clone().unwrap();
    assert!(
        command.starts_with(
            "corral new 'Fix feature/plan checkpoint C1: unit tests' -p 'app' --kind bug"
        ),
        "{command}"
    );
    let spec = command.rsplit(' ').next().unwrap().trim_matches('\'');
    assert!(
        std::fs::read_to_string(spec)
            .unwrap()
            .contains(&log.display().to_string())
    );
    // Then exactly one PM card names the step, the log and the prepared command.
    let cards = checkpoint_cards(&f);
    assert_eq!(cards.len(), 1, "{cards:?}");
    for needle in ["`unit tests`", &log.display().to_string(), &command] {
        assert!(cards[0].contains(needle), "{needle} missing: {}", cards[0]);
    }
    let card = pm_card(
        &PmObservation::Owner {
            task: f.plan.clone(),
            reason: cards[0].clone(),
        },
        "corral",
    );
    assert_eq!(card.kind, PmActionKind::Owner);
    assert!(card.requires_owner);
    // And nothing is filed or dispatched.
    let after = f.ctx.store.list().unwrap();
    assert_eq!(after.len(), tasks.len());
    for (old, new) in tasks.iter().zip(&after) {
        assert_eq!(
            (&old.id, old.status, old.runs.len()),
            (&new.id, new.status, new.runs.len())
        );
    }
    // A green checkpoint clears the card.
    drop(marker);
    let attempt = plan_checkpoint(&f.ctx, &f.plan).unwrap();
    assert_eq!(
        (attempt.id.as_str(), attempt.gate.result),
        ("C2", GateResult::Pass)
    );
    assert!(checkpoint_cards(&f).is_empty());
}

#[test]
fn autopilot_starts_one_checkpoint_on_the_nth_landing() {
    // Given plan_checkpoint_every = 2 and two landings on the target.
    let f = Fixture::new(Delivery::Integration, &["true"]);
    tiered(&f, Some(2));
    let progress = super::progress(&fresh(&f), &f.ctx.store.get(&f.plan).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            progress.every,
            progress.landings_since_checkpoint,
            progress.due
        ),
        (Some(2), 2, true)
    );
    // When autopilot looks, it launches one background checkpoint, which runs once.
    let mut launched = Vec::new();
    start_due_with(&fresh(&f), &mut |id| {
        launched.push(id.to_owned());
        Ok(())
    });
    assert_eq!(launched, vec![f.plan.clone()]);
    let attempt = plan_checkpoint_automatic(&f.ctx, &f.plan).unwrap().unwrap();
    assert!(attempt.automatic);
    assert_eq!(attempt.gate.result, GateResult::Pass);
    assert!(
        plan_checkpoint_automatic(&f.ctx, &f.plan)
            .unwrap()
            .is_none()
    );
    // Then the Plan is no longer due, and progress shows no landings since the checkpoint.
    start_due_with(&fresh(&f), &mut |id| {
        launched.push(id.to_owned());
        Ok(())
    });
    assert_eq!(launched.len(), 1);
    let progress = crate::plan_progress(&f.ctx, &f.plan).unwrap();
    let checkpoint = progress.aggregate.unwrap().checkpoint.unwrap();
    assert_eq!(
        (
            checkpoint.landings,
            checkpoint.landings_since_checkpoint,
            checkpoint.last.as_deref(),
            checkpoint.due
        ),
        (2, 0, Some("C1"), false)
    );
}

#[test]
fn autopilot_never_starts_a_checkpoint_before_the_nth_landing_or_when_unset() {
    for every in [Some(3), None] {
        let f = Fixture::new(Delivery::Integration, &["true"]);
        tiered(&f, every);
        let mut launched = Vec::new();
        start_due_with(&fresh(&f), &mut |id| {
            launched.push(id.to_owned());
            Ok(())
        });
        assert!(launched.is_empty(), "{every:?}");
        assert!(
            plan_checkpoint_automatic(&f.ctx, &f.plan)
                .unwrap()
                .is_none()
        );
        let progress = crate::plan_progress(&f.ctx, &f.plan).unwrap();
        let checkpoint = progress.aggregate.unwrap().checkpoint.unwrap();
        assert_eq!(
            (
                checkpoint.every,
                checkpoint.landings_since_checkpoint,
                checkpoint.due
            ),
            (every, 2, false)
        );
    }
}
