use std::collections::BTreeMap;

use corral_core::{
    ExpectedFailure, GateResult, MemberGateAmendment, OperatingAuthority, ScopeGate, TierPolicy,
};

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
    let mut store = f.ctx.store.reopen().unwrap();
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

/// A confirmed Autopilot grant over the Plan with delegated dispatch (and so gate) authority.
fn grant(f: &Fixture) {
    f.grant(OperatingAuthority {
        dispatch: true,
        ..Default::default()
    });
}

fn launches(f: &Fixture) -> Vec<String> {
    let mut launched = Vec::new();
    start_due_with(&fresh(f), &mut |id| {
        launched.push(id.to_owned());
        Ok(())
    });
    launched
}

fn attempts(f: &Fixture) -> Option<String> {
    let progress = crate::plan_progress(&f.ctx, &f.plan).unwrap();
    progress.aggregate.unwrap().checkpoint.unwrap().last
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
    grant(&f);
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

#[test]
fn automatic_checkpoints_honor_pause_assist_and_the_confirmed_grant() {
    // Given a due Plan target (every = 2, two landings) in Assist with no grant.
    let f = Fixture::new(Delivery::Integration, &["true"]);
    tiered(&f, Some(2));
    // Then autopilot neither schedules nor executes a checkpoint.
    assert!(launches(&f).is_empty());
    assert!(
        plan_checkpoint_automatic(&f.ctx, &f.plan)
            .unwrap()
            .is_none()
    );
    assert_eq!(attempts(&f), None);
    // Given a confirmed grant that is paused, the same holds.
    grant(&f);
    f.ctx.store.pause_operating_policy(true).unwrap();
    assert!(launches(&f).is_empty());
    assert!(
        plan_checkpoint_automatic(&f.ctx, &f.plan)
            .unwrap()
            .is_none()
    );
    assert_eq!(attempts(&f), None);
    // When the grant resumes and autopilot schedules, but a pause lands before execution.
    f.ctx.store.pause_operating_policy(false).unwrap();
    let mut launched = Vec::new();
    start_due_with(&fresh(&f), &mut |id| {
        launched.push(id.to_owned());
        f.ctx.store.pause_operating_policy(true).map(drop)
    });
    assert_eq!(launched, vec![f.plan.clone()]);
    // Then the execution boundary refuses the gate.
    assert!(
        plan_checkpoint_automatic(&f.ctx, &f.plan)
            .unwrap()
            .is_none()
    );
    assert_eq!(attempts(&f), None);
    // While paused, the explicit owner checkpoint stays available and is not automatic.
    let manual = plan_checkpoint(&f.ctx, &f.plan).unwrap();
    assert!(!manual.automatic);
    // After resuming, the manual C1 reset the interval, so nothing is due yet.
    f.ctx.store.pause_operating_policy(false).unwrap();
    assert!(launches(&f).is_empty());
}

#[test]
fn authorized_automatic_checkpoint_runs_under_the_grant() {
    let f = Fixture::new(Delivery::Integration, &["true"]);
    tiered(&f, Some(1));
    grant(&f);
    assert_eq!(launches(&f), vec![f.plan.clone()]);
    let attempt = plan_checkpoint_automatic(&f.ctx, &f.plan).unwrap().unwrap();
    assert!(attempt.automatic && attempt.advisory);
    assert_eq!(attempt.gate.result, GateResult::Pass);
}

fn fifo(path: &std::path::Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn red_card_survives_a_running_or_lost_retry_until_a_later_pass() {
    // Given a full gate that fails while `red` exists and, while `hold` exists, announces
    // itself on `started` and waits on `release` (FIFO barriers, no timing).
    let dir = tempfile::tempdir().unwrap();
    let (red, hold) = (dir.path().join("red"), dir.path().join("hold"));
    let (started, release) = (dir.path().join("started"), dir.path().join("release"));
    fifo(&started);
    fifo(&release);
    std::fs::write(&red, "").unwrap();
    let gate = format!(
        "printf '==> tier full\\n\\n==> unit tests\\n' && if test -e {hold}; then echo up > {started} && read go < {release}; fi && test ! -e {red}",
        hold = hold.display(),
        started = started.display(),
        release = release.display(),
        red = red.display(),
    );
    let f = Fixture::new(Delivery::Integration, &[&gate]);
    let c1 = plan_checkpoint(&f.ctx, &f.plan).unwrap();
    assert_eq!(c1.gate.result, GateResult::Fail);
    assert_eq!(checkpoint_cards(&f).len(), 1);
    // When a retry C2 is running and the cause is fixed.
    std::fs::remove_file(&red).unwrap();
    std::fs::write(&hold, "").unwrap();
    std::thread::scope(|scope| {
        let retry = scope.spawn(|| plan_checkpoint(&f.ctx, &f.plan).unwrap());
        let mut up = String::new();
        std::io::Read::read_to_string(&mut std::fs::File::open(&started).unwrap(), &mut up)
            .unwrap();
        assert_eq!(up, "up\n");
        // Then C1's card is still shown while C2 is Running.
        let cards = checkpoint_cards(&f);
        assert_eq!(cards.len(), 1, "{cards:?}");
        assert!(cards[0].contains("checkpoint C1"), "{}", cards[0]);
        std::fs::write(&release, "go\n").unwrap();
        // And C2's pass clears it.
        let c2 = retry.join().unwrap();
        assert_eq!((c2.id.as_str(), c2.gate.result), ("C2", GateResult::Pass));
    });
    assert!(checkpoint_cards(&f).is_empty());
    // Given a red C3 followed by a C4 whose process was lost mid-gate.
    std::fs::remove_file(&hold).unwrap();
    std::fs::write(&red, "").unwrap();
    assert_eq!(
        plan_checkpoint(&f.ctx, &f.plan).unwrap().gate.result,
        GateResult::Fail
    );
    let path = super::journal_path(&f.ctx.store, &f.plan);
    let mut journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let mut lost = journal["attempts"][2].clone();
    lost["id"] = "C4".into();
    lost["gate"]["result"] = "running".into();
    journal["attempts"].as_array_mut().unwrap().push(lost);
    std::fs::write(&path, serde_json::to_vec(&journal).unwrap()).unwrap();
    // Then C3's card stays, both while C4 looks running and once a retry marks it lost.
    let cards = checkpoint_cards(&f);
    assert!(
        cards.len() == 1 && cards[0].contains("checkpoint C3"),
        "{cards:?}"
    );
    std::fs::remove_file(&red).unwrap();
    std::fs::write(&hold, "").unwrap();
    std::thread::scope(|scope| {
        let retry = scope.spawn(|| plan_checkpoint(&f.ctx, &f.plan).unwrap());
        let mut up = String::new();
        std::io::Read::read_to_string(&mut std::fs::File::open(&started).unwrap(), &mut up)
            .unwrap();
        let cards = checkpoint_cards(&f);
        assert!(
            cards.len() == 1 && cards[0].contains("checkpoint C3"),
            "{cards:?}"
        );
        std::fs::write(&release, "go\n").unwrap();
        assert_eq!(retry.join().unwrap().gate.result, GateResult::Pass);
    });
    assert!(checkpoint_cards(&f).is_empty());
}
