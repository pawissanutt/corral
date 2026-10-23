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

/// Bound on every wait of the held-gate harness; a regression fails instead of hanging.
const WAIT: std::time::Duration = std::time::Duration::from_secs(60);

enum Held<R> {
    Ready(std::net::TcpStream),
    Done(R),
}

/// Unblocks the harness's pending `accept` on every exit path, including a panic.
struct Unblock(std::net::SocketAddr);

impl Drop for Unblock {
    fn drop(&mut self) {
        // Connecting is the release itself; a listener that already accepted ignores it.
        drop(std::net::TcpStream::connect(self.0));
    }
}

/// A gate command prefix: while `port` exists, connect to the loopback port it names, announce
/// `up` and block until the harness closes the connection. Closing never fails the gate.
fn held(port: &std::path::Path) -> String {
    format!(
        "if test -e {port}; then exec 3<>/dev/tcp/127.0.0.1/$(cat {port}) && echo up >&3 && read -r go <&3; fi; ",
        port = port.display()
    )
}

/// Wait for `run` to finish. Every gate connection that arrives meanwhile, early or late, is
/// closed at once, which releases that gate, so no accepted socket outlives the scope.
fn finish<R>(rx: &std::sync::mpsc::Receiver<Held<R>>) -> anyhow::Result<R> {
    loop {
        match rx.recv_timeout(WAIT) {
            Ok(Held::Done(result)) => return Ok(result),
            Ok(Held::Ready(stream)) => drop(stream),
            Err(error) => anyhow::bail!("held run did not finish within {WAIT:?}: {error}"),
        }
    }
}

/// Run `run` while accepting one gate connection on `listener`. When the gate announces `up`
/// within `ready`, `observe` runs while it is held; otherwise `not_held` runs. The gate is
/// then released, and `run` awaited, inside the scope on every path, before any caller
/// assertion.
fn hold_with<R: Send, C>(
    listener: std::net::TcpListener,
    ready: std::time::Duration,
    run: impl FnOnce() -> R + Send,
    observe: impl FnOnce() -> anyhow::Result<C>,
    not_held: impl FnOnce(),
) -> (anyhow::Result<C>, anyhow::Result<R>) {
    use std::io::BufRead;
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let accepted = tx.clone();
        scope.spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                drop(accepted.send(Held::Ready(stream)));
            }
        });
        scope.spawn(move || drop(tx.send(Held::Done(run()))));
        let _unblock = Unblock(addr);
        let observed = match rx.recv_timeout(ready) {
            Ok(Held::Ready(stream)) => {
                let mut line = String::new();
                let announced = stream
                    .set_read_timeout(Some(WAIT))
                    .and_then(|()| std::io::BufReader::new(&stream).read_line(&mut line));
                let observed = match announced {
                    Ok(_) if line == "up\n" => observe(),
                    Ok(_) => Err(anyhow::anyhow!("gate announced {line:?}")),
                    Err(error) => Err(anyhow::anyhow!("gate announcement: {error}")),
                };
                // Closing the connection releases the held gate.
                drop(stream);
                observed
            }
            Ok(Held::Done(result)) => {
                return (
                    Err(anyhow::anyhow!("run finished before its gate was held")),
                    Ok(result),
                );
            }
            Err(error) => {
                not_held();
                Err(anyhow::anyhow!(
                    "gate was not held within {ready:?}: {error}"
                ))
            }
        };
        (observed, finish(&rx))
    })
}

/// Run a checkpoint whose gate is held and read the PM cards while it is Running.
fn while_held(
    f: &Fixture,
    port: &std::path::Path,
) -> (anyhow::Result<Vec<String>>, super::PlanCheckpoint) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    std::fs::write(port, listener.local_addr().unwrap().port().to_string()).unwrap();
    let (cards, attempt) = hold_with(
        listener,
        WAIT,
        || plan_checkpoint(&f.ctx, &f.plan),
        || observations(&f.ctx).map(|_| checkpoint_cards(f)),
        || {},
    );
    std::fs::remove_file(port).unwrap();
    (cards, attempt.unwrap().unwrap())
}

#[test]
fn held_harness_releases_a_gate_that_connects_after_the_readiness_timeout() {
    use std::io::{Read, Write};
    // Given a simulated gate that connects only once the harness gave up waiting for it.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (late_tx, late_rx) = std::sync::mpsc::channel::<()>();
    // Released means the harness closed the socket: EOF, or a reset because the late `up`
    // was never read. A socket kept past the scope would surface as a read timeout instead.
    let gate = move || -> std::io::Result<bool> {
        late_rx
            .recv_timeout(WAIT)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let mut stream = std::net::TcpStream::connect(addr)?;
        stream.write_all(b"up\n")?;
        // Bounded only so a regression fails instead of hanging; release must come first.
        stream.set_read_timeout(Some(WAIT))?;
        match stream.read(&mut [0; 1]) {
            Ok(0) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => Ok(true),
            Ok(_) => Ok(false),
            Err(error) => Err(error),
        }
    };
    // When readiness times out and the gate connects late.
    let (observed, finished) = hold_with(
        listener,
        std::time::Duration::from_millis(100),
        gate,
        || -> anyhow::Result<()> { panic!("the late gate is never observed as held") },
        move || late_tx.send(()).unwrap(),
    );
    // Then the harness reports the readiness timeout and closed the late connection inside
    // the scope, so the gate saw the close rather than timing out.
    let error = observed.unwrap_err().to_string();
    assert!(error.contains("gate was not held within"), "{error}");
    assert!(finished.unwrap().unwrap());
}

#[test]
fn red_card_survives_a_running_or_lost_retry_until_a_later_pass() {
    // Given a full gate that fails while `red` exists and is held while `port` exists.
    let dir = tempfile::tempdir().unwrap();
    let (red, port) = (dir.path().join("red"), dir.path().join("port"));
    std::fs::write(&red, "").unwrap();
    let gate = format!(
        "printf '==> tier full\\n\\n==> unit tests\\n'; {}test ! -e {}",
        held(&port),
        red.display(),
    );
    let f = Fixture::new(Delivery::Integration, &[&gate]);
    let c1 = plan_checkpoint(&f.ctx, &f.plan).unwrap();
    assert_eq!(c1.gate.result, GateResult::Fail);
    assert_eq!(checkpoint_cards(&f).len(), 1);
    // When a retry C2 is running after the cause was fixed.
    std::fs::remove_file(&red).unwrap();
    let (cards, c2) = while_held(&f, &port);
    // Then C1's card is still shown while C2 is Running, and C2's pass clears it.
    let cards = cards.unwrap();
    assert_eq!(cards.len(), 1, "{cards:?}");
    assert!(cards[0].contains("checkpoint C1"), "{}", cards[0]);
    assert_eq!((c2.id.as_str(), c2.gate.result), ("C2", GateResult::Pass));
    assert!(checkpoint_cards(&f).is_empty());
    // Given a red C3 followed by a C4 whose process was lost mid-gate.
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
    // Then C3's card stays while C4 looks running, and while C5 runs after marking C4 lost.
    let cards = checkpoint_cards(&f);
    assert!(
        cards.len() == 1 && cards[0].contains("checkpoint C3"),
        "{cards:?}"
    );
    std::fs::remove_file(&red).unwrap();
    let (cards, c5) = while_held(&f, &port);
    let cards = cards.unwrap();
    assert!(
        cards.len() == 1 && cards[0].contains("checkpoint C3"),
        "{cards:?}"
    );
    assert_eq!((c5.id.as_str(), c5.gate.result), ("C5", GateResult::Pass));
    assert!(checkpoint_cards(&f).is_empty());
}
