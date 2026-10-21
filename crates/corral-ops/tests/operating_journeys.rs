//! End-to-end operating-policy journeys (T-142 C1-C7) over isolated stores, a fake terminal
//! engine, real disposable Git repositories and a fixed sync clock. Every wait is an explicit
//! completion signal (a worker `done`, a gate script run by the test, a bound verdict); no sleeps.
#![cfg(unix)]

#[path = "operating_launch/fixture.rs"]
mod fixture;

use std::collections::BTreeSet;
use std::process::{Command, Stdio};

use corral_core::{
    DecisionRequest, GateResult, ObligationSource, OperatingActionKind, OperatingAuthority,
    OperatingMode, OperatingSelection, Status, Store, Verdict,
};
use corral_ops::{ApproveStrategy, OperatingStatus, PmActionOutcome};
use fixture::{Fx, now};

const ALL: OperatingAuthority = OperatingAuthority {
    dispatch: true,
    accept: true,
    plan_accept: true,
};

fn sync(fx: &Fx) {
    corral_ops::sync_at(&fx.fresh(), now()).unwrap();
}

fn status(fx: &Fx, id: &str) -> Status {
    fx.ctx.store.get(id).unwrap().status
}

fn depends(fx: &Fx, task: &str, on: &str) {
    fx.ctx
        .store
        .update(task, |task| {
            task.depends_on.push(on.into());
            Ok(())
        })
        .unwrap();
}

fn confirm(fx: &mut Fx, selection: &OperatingSelection) {
    let preview = fx.ctx.store.preview_operating_policy(selection).unwrap();
    fx.ctx.store.config = fx
        .ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", now())
        .unwrap();
}

/// The fixture's independent reviewer returns an approving verdict bound to its attempt.
fn approve_review(fx: &Fx, id: &str) {
    let task = fx.ctx.store.get(id).unwrap();
    let review = task.current_run().unwrap().review.as_ref().unwrap();
    corral_ops::verdict_bound(
        &fx.fresh(),
        id,
        Verdict::Approve,
        "Independent fixture approval",
        Some(&corral_ops::review_callback_token(review).unwrap()),
    )
    .unwrap();
}

fn register_question(fx: &Fx, task: &str) {
    corral_ops::register_decision(
        &fx.ctx,
        task,
        DecisionRequest {
            version: 1,
            id: "journey-choice".into(),
            run: "R1".into(),
            question: "Which variant should ship?".into(),
            requested_action: "Choose the variant".into(),
            owner: "owner".into(),
            source: ObligationSource::Adapter,
            deadline: None,
        },
        now(),
    )
    .unwrap();
}

/// Bring a task to an independently approved Review candidate through explicit owner tools.
fn reviewed_candidate(fx: &Fx, id: &str, file: &str) {
    fx.start(id);
    fx.done(id, file);
    corral_ops::gate_start(&fx.ctx, id).unwrap();
    corral_ops::gate_collect(&fx.ctx, id).unwrap().unwrap();
    let review = corral_ops::review_run(&fx.ctx, id, Some("rev")).unwrap();
    corral_ops::verdict_bound(
        &fx.ctx,
        id,
        Verdict::Approve,
        "Independently reviewed",
        Some(&corral_ops::review_callback_token(&review).unwrap()),
    )
    .unwrap();
}

/// The reviewed run's OmO worker asked a native question the owner has not settled.
fn native_question(fx: &mut Fx, id: &str) {
    let agent = fx.temp.path().join("omo-agent");
    fx.ctx.store.config.sync.omo_agent_dir = Some(agent.clone());
    fx.ctx.store.save_config().unwrap();
    let task = fx
        .ctx
        .store
        .update(id, |task| {
            task.current_run_mut().unwrap().harness = Some("omo".into());
            Ok(())
        })
        .unwrap();
    let worktree = task
        .current_run()
        .unwrap()
        .worktree
        .to_string_lossy()
        .into_owned();
    let encoded = worktree.trim_start_matches('/').replace(['/', ':'], "-");
    let sessions = agent.join("sessions").join(format!("--{encoded}--"));
    std::fs::create_dir_all(&sessions).unwrap();
    let lines = [
        serde_json::json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":"journey|native-choice","name":"request_user_input","arguments":{"questions":[{"question":"Which variant should ship?","options":["A","B"]}]}}]}}),
        serde_json::json!({"type":"custom","customType":"ask-user:question","data":{"requestId":"journey|native-choice","headers":["Variant"]}}),
    ];
    std::fs::write(
        sessions.join("question.jsonl"),
        lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>(),
    )
    .unwrap();
}

fn advance_main(fx: &Fx) {
    let repo = fx.temp.path().join("app");
    std::fs::write(repo.join("main-moved.txt"), "unrelated main movement\n").unwrap();
    fixture::git(&repo, &["add", "-A"]);
    fixture::git(
        &repo,
        &["-c", "commit.gpgsign=false", "commit", "-qm", "main moved"],
    );
}

/// Complete a gate the fake engine was told to hold, exactly as its pane would have.
fn finish_held_gate(fx: &Fx) {
    let gate = fx
        .records("submissions.jsonl")
        .into_iter()
        .rev()
        .find(|line| line.contains("/state/gates/"))
        .expect("a held gate submission");
    let args: Vec<String> = serde_json::from_str(&gate).unwrap();
    let status = Command::new("sh")
        .arg("-c")
        .arg(&args[3])
        .env("CORRAL_ROOT", fx.temp.path())
        .env_remove("CORRAL_GATE_ATTEMPT")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
}

/// One PM wake: present each newly required decision once, act on fresh authorized actions
/// until none makes progress, then reconcile through the fixed-clock sync.
fn pm_pass(fx: &Fx, presented: &mut Vec<(String, String)>) {
    let mut acted = BTreeSet::new();
    loop {
        let ctx = fx.fresh();
        let workset = corral_ops::pm_workset(&ctx).unwrap();
        for decision in workset
            .decisions
            .iter()
            .filter(|d| d.presentation.is_none())
        {
            corral_ops::pm_present(&ctx, &decision.id, "owner-pane:journey", true, now()).unwrap();
            presented.push((decision.task.clone(), decision.id.clone()));
        }
        let Some(action) = workset.actions.iter().find(|a| !acted.contains(&a.id)) else {
            break;
        };
        acted.insert(action.id.clone());
        if !matches!(
            corral_ops::pm_act(&ctx, &action.id).unwrap().outcome,
            PmActionOutcome::Progressed
        ) {
            break;
        }
    }
    sync(fx);
}

#[test]
fn legacy_settings_cancelled_preview_and_finite_activation() {
    // C1/C2/C6: every legacy automation combination loads as Assist and never rewrites config.
    let legacy: [&[(&str, &str)]; 5] = [
        &[("review.autopilot", "true")],
        &[("review.accept", "\"auto\"")],
        &[("dispatch.auto_backlog", "true")],
        &[("dispatch.promote_ideas", "\"auto\"")],
        &[
            ("review.autopilot", "true"),
            ("review.accept", "\"auto\""),
            ("dispatch.auto_backlog", "true"),
            ("dispatch.promote_ideas", "\"auto\""),
        ],
    ];
    for settings in legacy {
        // Given legacy-enabled automation and eligible Ready, Backlog and Idea work.
        let mut fx = Fx::new();
        Store::open(fx.temp.path())
            .unwrap()
            .edit_config(|doc| {
                for (key, value) in settings {
                    doc.set_str(key, value)?;
                }
                Ok(())
            })
            .unwrap();
        let included = fx.task("included", Status::Ready);
        let excluded = fx.task("excluded", Status::Ready);
        let backlog = fx.task("backlog", Status::Backlog);
        let idea = fx.task("idea", Status::Idea);
        let config = std::fs::read(fx.ctx.store.config_path()).unwrap();
        // When fresh processes load and sync it repeatedly.
        sync(&fx);
        sync(&fx);
        // Then the effective mode is Assist and nothing starts or is rewritten.
        let policy = fx.fresh().store.effective_operating_policy().unwrap();
        assert_eq!(policy.mode, OperatingMode::Assist, "{settings:?}");
        for id in [&included.id, &excluded.id, &backlog.id, &idea.id] {
            assert!(!policy.allows(id, OperatingActionKind::Dispatch));
            assert!(!policy.allows(id, OperatingActionKind::Accept));
            assert!(
                fx.ctx.store.get(id).unwrap().runs.is_empty(),
                "{settings:?}"
            );
        }
        assert!(fx.records("submissions.jsonl").is_empty(), "{settings:?}");
        assert_eq!(std::fs::read(fx.ctx.store.config_path()).unwrap(), config);

        // When the owner previews a finite grant and cancels it.
        let selection = OperatingSelection {
            tasks: [included.id.clone(), excluded.id.clone()].into(),
            exclusions: [excluded.id.clone()].into(),
            authority: ALL,
            ..Default::default()
        };
        let preview = fx.ctx.store.preview_operating_policy(&selection).unwrap();
        assert!(preview.policy.scope.tasks.contains_key(&included.id));
        assert!(!preview.policy.scope.tasks.contains_key(&excluded.id));
        drop(preview);
        sync(&fx);
        // Then no grant, receipt or start exists.
        assert_eq!(std::fs::read(fx.ctx.store.config_path()).unwrap(), config);
        assert!(fx.ctx.store.operating_state().unwrap().approval.is_none());
        assert!(fx.records("submissions.jsonl").is_empty());

        // When the owner confirms the same finite scope, then files new work.
        confirm(&mut fx, &selection);
        let filed = fx.task("filed after confirmation", Status::Ready);
        for _ in 0..2 {
            sync(&fx);
        }
        // Then only the exact confirmed task starts; exclusions, backlog, ideas and new work wait.
        assert_eq!(status(&fx, &included.id), Status::Running, "{settings:?}");
        for id in [&excluded.id, &backlog.id, &idea.id, &filed.id] {
            assert!(
                fx.ctx.store.get(id).unwrap().runs.is_empty(),
                "{settings:?}"
            );
        }
        assert_eq!(fx.records("submissions.jsonl").len(), 1, "{settings:?}");
        let policy = fx.fresh().store.effective_operating_policy().unwrap();
        assert_eq!(policy.mode, OperatingMode::Autopilot);
        assert!(!policy.allows(&filed.id, OperatingActionKind::Dispatch));
    }
}

#[test]
fn assist_chain_needs_an_explicit_owner_action_for_every_stage() {
    // C2 (Assist): a dependency chain with default auto_backlog and no grant.
    let fx = Fx::new();
    let first = fx.task("first", Status::Ready);
    let second = fx.task("second", Status::Backlog);
    depends(&fx, &second.id, &first.id);
    sync(&fx);
    assert!(fx.records("submissions.jsonl").is_empty());
    // When the owner starts only the first task and its worker completes.
    fx.start(&first.id);
    fx.done(&first.id, "first.txt");
    sync(&fx);
    sync(&fx);
    // Then no gate, reviewer or acceptance follows automatically.
    let waiting = fx.ctx.store.get(&first.id).unwrap();
    assert_eq!(waiting.status, Status::Review);
    assert!(waiting.current_run().unwrap().gate.is_none());
    assert!(waiting.current_run().unwrap().review.is_none());
    assert_eq!(fx.records("submissions.jsonl").len(), 1);
    // When the owner runs the gate and an independent review that approves.
    corral_ops::gate_start(&fx.ctx, &first.id).unwrap();
    corral_ops::gate_collect(&fx.ctx, &first.id)
        .unwrap()
        .unwrap();
    corral_ops::review_run(&fx.ctx, &first.id, Some("rev")).unwrap();
    approve_review(&fx, &first.id);
    sync(&fx);
    // Then approval still requires the owner, and the successor never inherits a start.
    assert_eq!(status(&fx, &first.id), Status::Review);
    assert!(!fx.temp.path().join("app/first.txt").exists());
    corral_ops::approve(&fx.fresh(), &first.id, ApproveStrategy::Rebase, true, false).unwrap();
    assert_eq!(status(&fx, &first.id), Status::Done);
    assert!(fx.temp.path().join("app/first.txt").is_file());
    sync(&fx);
    sync(&fx);
    assert!(fx.ctx.store.get(&second.id).unwrap().runs.is_empty());
    assert_eq!(fx.records("submissions.jsonl").len(), 3);
}

#[test]
fn pending_question_is_presented_once_while_a_repaired_chain_lands_across_restarts() {
    // C2/C3: question on A; delegated B -> C with a gate that fails until repaired.
    let mut fx = Fx::new();
    fx.ctx.store.config.review.gate = Some(vec!["test -f repaired.txt".into()]);
    fx.ctx.store.save_config().unwrap();
    let a = fx.task("owner choice", Status::Ready);
    let b = fx.task("independent", Status::Ready);
    let c = fx.task("successor", Status::Backlog);
    depends(&fx, &c.id, &b.id);
    confirm(
        &mut fx,
        &OperatingSelection {
            tasks: [a.id.clone(), b.id.clone(), c.id.clone()].into(),
            authority: ALL,
            ..Default::default()
        },
    );
    let mut presented = Vec::new();
    pm_pass(&fx, &mut presented);
    register_question(&fx, &a.id);
    assert_eq!(status(&fx, &b.id), Status::Running);
    assert!(fx.ctx.store.get(&c.id).unwrap().runs.is_empty());

    // When B's first candidate fails its gate and repeated wakes and restarts follow.
    fx.done(&b.id, "independent.txt");
    for _ in 0..3 {
        pm_pass(&fx, &mut presented);
    }
    // Then exactly one bounded repair is delivered without an owner prompt, A is shown once.
    let repairing = fx.ctx.store.get(&b.id).unwrap();
    assert_eq!(repairing.status, Status::Running);
    assert_eq!(repairing.rounds, 1);
    assert_eq!(fx.records("deliveries.jsonl").len(), 1);
    assert_eq!(presented.len(), 1, "{presented:?}");
    assert_eq!(presented[0].0, a.id);
    let state = corral_ops::pm_workset(&fx.fresh()).unwrap();
    assert_eq!(state.status, OperatingStatus::NeedsYourDecision);
    assert!(state.independent_work_continues);
    assert!(state.decisions.iter().any(|decision| decision.task == a.id));

    // When the repair passes, the reviewer approves and each successor completes.
    fx.done(&b.id, "repaired.txt");
    pm_pass(&fx, &mut presented);
    approve_review(&fx, &b.id);
    pm_pass(&fx, &mut presented);
    assert_eq!(status(&fx, &b.id), Status::Done);
    assert_eq!(status(&fx, &c.id), Status::Running);
    fx.done(&c.id, "successor.txt");
    pm_pass(&fx, &mut presented);
    approve_review(&fx, &c.id);
    for _ in 0..2 {
        pm_pass(&fx, &mut presented);
    }

    // Then B and C landed, A stays pending and presented once, and Complete is impossible.
    assert_eq!(status(&fx, &c.id), Status::Done);
    for file in ["independent.txt", "repaired.txt", "successor.txt"] {
        assert!(fx.temp.path().join("app").join(file).is_file(), "{file}");
    }
    let state = corral_ops::pm_workset(&fx.fresh()).unwrap();
    assert_eq!(state.status, OperatingStatus::NeedsYourDecision);
    assert_eq!(state.progress.total, 3);
    assert_eq!(
        state.progress.accepted,
        2,
        "{:?} {:?}",
        state.blockers,
        state
            .decisions
            .iter()
            .map(|d| (
                &d.task,
                &d.action.why,
                &d.action.evidence,
                &d.action.recommended
            ))
            .collect::<Vec<_>>()
    );
    assert_eq!(presented.len(), 1, "{presented:?}");
    assert_eq!(presented[0].0, a.id);
    let pending = fx.ctx.store.get(&a.id).unwrap();
    assert!(
        pending
            .current_run()
            .unwrap()
            .obligations
            .pending_decision()
            .is_some()
    );
    assert_eq!(fx.ctx.store.get(&b.id).unwrap().rounds, 1);
    assert_eq!(fx.records("deliveries.jsonl").len(), 1);
}

#[test]
fn real_blockers_precede_stale_review_when_main_moves() {
    // C4: held, authentication-blocked, cancelled and owner-decision candidates, all reviewed.
    let mut fx = Fx::new();
    let held = fx.task("held", Status::Ready);
    let auth = fx.task("authentication", Status::Ready);
    let cancelled = fx.task("cancelled", Status::Ready);
    let question = fx.task("owner decision", Status::Ready);
    for (task, file) in [
        (&held, "held.txt"),
        (&auth, "auth.txt"),
        (&cancelled, "cancelled.txt"),
        (&question, "question.txt"),
    ] {
        reviewed_candidate(&fx, &task.id, file);
    }
    native_question(&mut fx, &question.id);
    let independent = fx.task("independent", Status::Ready);
    let ids = [&held.id, &auth.id, &cancelled.id, &question.id];
    confirm(
        &mut fx,
        &OperatingSelection {
            tasks: ids
                .iter()
                .map(|id| (*id).clone())
                .chain([independent.id.clone()])
                .collect(),
            authority: ALL,
            ..Default::default()
        },
    );
    let update = |id: &str, change: fn(&mut corral_core::Task)| {
        fx.ctx
            .store
            .update(id, |task| {
                change(task);
                Ok(())
            })
            .unwrap();
    };
    update(&held.id, |task| task.dispatch_hold = true);
    update(&auth.id, |task| {
        task.status = Status::NeedsMe;
        task.current_run_mut().unwrap().blocked_excerpt =
            Some("Authentication failed; owner must refresh credentials".into());
    });
    update(&cancelled.id, |task| task.status = Status::Dropped);
    advance_main(&fx);
    let key = |task: &corral_core::Task| {
        task.runs
            .iter()
            .map(|run| {
                (
                    run.id.clone(),
                    fixture::git(&run.worktree, &["rev-parse", "HEAD"]),
                    run.gate.clone(),
                    run.review.clone(),
                    run.ended,
                )
            })
            .collect::<Vec<_>>()
    };
    let before: Vec<_> = ids.iter().map(|id| fx.ctx.store.get(id).unwrap()).collect();
    for (id, task) in ids.iter().zip(&before) {
        let run = task.current_run().unwrap();
        assert!(
            run.gate.is_some() && run.review.is_some(),
            "{id} must be a reviewed candidate"
        );
    }
    let frozen: Vec<_> = before.iter().map(key).collect();
    let submitted = fx.records("submissions.jsonl").len();

    // When repeated fresh syncs see stale integration data with free capacity.
    for _ in 0..3 {
        sync(&fx);
    }

    // Then no rebase, gate, review, repair, retry or landing crosses any real blocker.
    for ((id, before), frozen) in ids.iter().zip(&before).zip(&frozen) {
        let after = fx.ctx.store.get(id).unwrap();
        assert_eq!(after.status, before.status, "{id}");
        assert_eq!(&key(&after), frozen, "{id}");
        assert_eq!(after.rounds, 0, "{id}");
        assert_eq!(after.dispatch_hold, before.dispatch_hold, "{id}");
    }
    for file in ["held.txt", "auth.txt", "cancelled.txt", "question.txt"] {
        assert!(!fx.temp.path().join("app").join(file).exists(), "{file}");
    }
    assert!(fx.records("deliveries.jsonl").is_empty());
    assert_eq!(fx.records("submissions.jsonl").len(), submitted + 1);
    assert_eq!(status(&fx, &independent.id), Status::Running);
    let state = corral_ops::pm_workset(&fx.fresh()).unwrap();
    assert_eq!(state.status, OperatingStatus::NeedsYourDecision);
    assert!(state.independent_work_continues);
    let pending = state.decisions.iter().find(|d| d.task == question.id);
    assert!(
        pending.is_some_and(|d| d.source.ends_with(" request journey|native-choice")),
        "{:?}",
        state.decisions
    );
}

#[test]
fn pause_collects_results_and_assist_prevents_landing_until_a_new_grant() {
    // C5: a delegated task whose gate is still running when the owner pauses.
    let mut fx = Fx::new();
    let task = fx.task("paused journey", Status::Ready);
    fx.grant(&[&task.id]);
    sync(&fx);
    std::fs::write(fx.bin.join("hold-gates"), "").unwrap();
    fx.done(&task.id, "paused.txt");
    sync(&fx);
    let gated = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(
        gated.current_run().unwrap().gate.as_ref().unwrap().result,
        GateResult::Running
    );
    let submitted = fx.records("submissions.jsonl").len();
    fx.fresh().store.pause_operating_policy(true).unwrap();
    let paused = corral_ops::pm_workset(&fx.fresh()).unwrap();
    assert!(paused.paused);
    assert!(paused.actions.is_empty());

    // When the running gate finishes during the pause and fresh syncs collect it.
    finish_held_gate(&fx);
    std::fs::remove_file(fx.bin.join("hold-gates")).unwrap();
    sync(&fx);
    sync(&fx);
    // Then its result is collected but no reviewer starts.
    let collected = fx.ctx.store.get(&task.id).unwrap();
    let run = collected.current_run().unwrap();
    assert_eq!(run.gate.as_ref().unwrap().result, GateResult::Pass);
    assert!(run.review.is_none());
    assert_eq!(fx.records("submissions.jsonl").len(), submitted);

    // When the owner resumes, the reviewer starts and approves.
    fx.fresh().store.pause_operating_policy(false).unwrap();
    sync(&fx);
    assert!(
        fx.ctx
            .store
            .get(&task.id)
            .unwrap()
            .current_run()
            .unwrap()
            .review
            .is_some()
    );
    assert_eq!(fx.records("submissions.jsonl").len(), submitted + 1);
    approve_review(&fx, &task.id);
    // When the owner switches to Assist before landing.
    fx.fresh().store.assist_operating_policy().unwrap();
    assert!(fx.fresh().store.pause_operating_policy(false).is_err());
    sync(&fx);
    sync(&fx);
    // Then the approved candidate does not land, and resume cannot revive the old grant.
    assert_eq!(status(&fx, &task.id), Status::Review);
    assert!(!fx.temp.path().join("app/paused.txt").exists());
    // When the owner confirms a fresh finite grant, the same verified candidate lands once.
    fx.grant(&[&task.id]);
    sync(&fx);
    assert_eq!(status(&fx, &task.id), Status::Done);
    assert!(fx.temp.path().join("app/paused.txt").is_file());
    assert_eq!(fx.records("submissions.jsonl").len(), submitted + 1);
}

#[test]
fn project_selection_is_a_finite_manifest_not_a_subscription() {
    // C6: a project grant without include-new resolves existing work only.
    let mut fx = Fx::new();
    let existing = fx.task("existing", Status::Ready);
    let excluded = fx.task("excluded", Status::Ready);
    let idea = fx.task("idea", Status::Idea);
    let selection = OperatingSelection {
        projects: ["app".into()].into(),
        exclusions: [excluded.id.clone()].into(),
        authority: OperatingAuthority {
            dispatch: true,
            accept: false,
            plan_accept: false,
        },
        ..Default::default()
    };
    let preview = fx.ctx.store.preview_operating_policy(&selection).unwrap();
    assert!(preview.policy.scope.projects.is_empty());
    assert!(preview.policy.scope.tasks.contains_key(&existing.id));
    assert!(!preview.policy.scope.tasks.contains_key(&idea.id));
    confirm(&mut fx, &selection);
    let later = fx.task("filed later", Status::Ready);
    // When repeated syncs run with free slots.
    for _ in 0..2 {
        sync(&fx);
    }
    // Then only the resolved member starts, and its grant cannot accept work.
    assert_eq!(status(&fx, &existing.id), Status::Running);
    for id in [&excluded.id, &idea.id, &later.id] {
        assert!(fx.ctx.store.get(id).unwrap().runs.is_empty());
    }
    fx.done(&existing.id, "existing.txt");
    sync(&fx);
    approve_review(&fx, &existing.id);
    sync(&fx);
    sync(&fx);
    assert_eq!(status(&fx, &existing.id), Status::Review);
    assert!(!fx.temp.path().join("app/existing.txt").exists());
    let policy = fx.fresh().store.effective_operating_policy().unwrap();
    assert!(!policy.allows(&existing.id, OperatingActionKind::Accept));
    assert!(!policy.allows(&later.id, OperatingActionKind::Dispatch));
}
