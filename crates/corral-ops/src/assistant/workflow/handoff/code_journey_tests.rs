use super::*;
use crate::agent::test_support as ts;
use corral_core::{CompletionDriver, TaskRef as _};

#[test]
fn dirty_saved_seed_returns_exact_layers_and_branch_without_residual_intent() {
    let mut f = ts::fx();
    let (session, _, expected) = tests::planning(&mut f);
    let repo = f._tmp.path().join("repo");
    tests::repo(&repo);
    git::git(&repo, &["branch", "-m", "main"], None).unwrap();
    std::fs::write(repo.join(".gitignore"), ".worktrees/\nignored\n").unwrap();
    git::git(&repo, &["add", ".gitignore"], None).unwrap();
    git::git(&repo, &["commit", "-m", "ignore worktrees"], None).unwrap();
    f.ctx.store.config.projects.insert(
        "app".into(),
        serde_json::from_value(serde_json::json!({"path":"repo","base":"main"})).unwrap(),
    );
    f.ctx.store.save_config().unwrap();
    let source = repo.join(".worktrees/local");
    git::git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "work",
            source.to_str().unwrap(),
            "main",
        ],
        None,
    )
    .unwrap();
    tests::record_source(&f, &session, &source, "main");
    std::fs::write(source.join("file"), b"staged\0\xff\r\n").unwrap();
    git::git(&source, &["add", "file"], None).unwrap();
    std::fs::write(source.join("file"), b"work\0\xfe\r\n").unwrap();
    std::fs::write(source.join("untracked"), b"note\r\n").unwrap();
    let container = f.ctx.store.get_assistant_container(&session).unwrap();
    let mut request = tests::request();
    request.goal = Goal::ImplementationPrepare;
    let prepared = prepare(
        &f.ctx.store,
        &session,
        &container,
        &expected,
        request,
        Some((&source, "main", "work")),
    )
    .unwrap();
    let before = prepared.package().snapshot.as_ref().unwrap();
    let choice =
        transfer::preview(&f.ctx, prepared.id(), "preserve dirty work", None, true).unwrap();
    let instruction = || lifecycle::Instruction::Shell {
        text: "explicit selected scope".into(),
        at: corral_core::clock::now(),
    };
    let saved =
        transfer::commit(&f.ctx, &choice, &choice.digest().unwrap(), instruction()).unwrap();
    assert!(saved.dispatch_hold && saved.runs.is_empty());
    assert_eq!(saved.cowork.as_ref().unwrap().scratch, "work");
    let returned = takeback::apply(
        &f.ctx,
        &saved.id,
        &saved.contract_revision().unwrap(),
        instruction(),
    )
    .unwrap();
    let takeback::Returned::Ready {
        scratch: Some(slug),
        plan_only: false,
        ..
    } = returned
    else {
        panic!("not returned")
    };
    let assistant = f.ctx.store.get_assistant(&session).unwrap();
    let returned = assistant.scratch.iter().find(|s| s.slug == slug).unwrap();
    assert_eq!(returned.base, "main");
    adopt::verify_worktree(before, &returned.path, &returned.branch).unwrap();
    before.validate_source(&source).unwrap();
    let task = f.ctx.store.get(&saved.id).unwrap();
    assert_eq!(f.ctx.store.list().unwrap().len(), 1);
    assert!(task.dispatch_hold && task.runs.is_empty() && task.plan_runs.is_empty());
    let scope = task.cowork.as_ref().unwrap();
    assert!(!scope.owner_confirmed);
    assert_eq!(
        scope.completion.as_ref().unwrap().driver,
        CompletionDriver::Assistant {
            session: session.clone(),
            scratch: slug.clone()
        }
    );
    assert!(
        admission::require(
            &f.ctx.store,
            &task,
            corral_core::OperatingActionKind::Gate,
            false
        )
        .is_err()
    );
    // Lost acknowledgement replay cannot re-release the returned scope or old intent.
    transfer::commit(&f.ctx, &choice, &choice.digest().unwrap(), instruction()).unwrap();
    assert_eq!(f.ctx.store.get(&task.id).unwrap().cowork, task.cowork);
    // Local preparation may change the preserved code, but an ordinary commit starts nothing.
    git::git(&returned.path, &["add", "."], None).unwrap();
    git::git(
        &returned.path,
        &["commit", "-m", "finish current work"],
        None,
    )
    .unwrap();
    assert!(f.ctx.store.get(&task.id).unwrap().runs.is_empty());
    f.ctx
        .store
        .update_assistant_container(&session, |c| {
            let w = &mut c.workflow.as_mut().unwrap().current;
            w.phase = WorkflowPhase::Candidate;
            w.state_revision += 1;
            Ok(())
        })
        .unwrap();
    let c = f.ctx.store.get_assistant_container(&session).unwrap();
    let w = &c.workflow.as_ref().unwrap().current;
    let expected = Expected {
        workflow: w.id.clone(),
        state_revision: w.state_revision,
        authority_epoch: w.authority_epoch,
        plan_revision: None,
    };
    let mut request = tests::request();
    request.goal = Goal::ImplementationPrepare;
    let next = prepare(
        &f.ctx.store,
        &session,
        &c,
        &expected,
        request,
        Some((&returned.path, "main", &returned.branch)),
    )
    .unwrap();
    let selected =
        transfer::preview(&f.ctx, next.id(), "save finished current work", None, true).unwrap();
    f.ctx
        .store
        .settle_assistant_container(&session, |c, _| reserve(&f.ctx.store, &session, c, &next))
        .unwrap();
    let reserved_choice =
        transfer::preview(&f.ctx, next.id(), "save finished current work", None, true).unwrap();
    assert_eq!(
        reserved_choice.into, selected.into,
        "a bare C2 reservation must not hide returned lineage or offer a duplicate task"
    );
    let tracked = f.ctx.store.get(&saved.id).unwrap();
    assert!(
        source::require_not_pending(&f.ctx.store, &tracked).is_err(),
        "the current returned scratch is fenced even when the historical run worktree differs or is absent"
    );
    let current = transfer::commit(
        &f.ctx,
        &selected,
        &selected.digest().unwrap(),
        instruction(),
    )
    .unwrap();
    assert_eq!(current.id, saved.id);
    assert_eq!(f.ctx.store.list().unwrap().len(), 1);
    assert_eq!(current.runs.len(), 1);
    assert_eq!(
        current.runs[0].autopilot.as_ref().unwrap().sha.as_deref(),
        Some(
            next.package()
                .snapshot
                .as_ref()
                .unwrap()
                .state
                .head
                .as_str()
        )
    );
    // A direct native resume/bind is also effect admission, not a way around Save.
    let target = crate::agent::BindTarget::Worker {
        task: current.id.clone(),
        run: current.runs[0].id.clone(),
    };
    assert!(
        crate::agent::open_generation(&f.ctx.store, &target, "stale-pane").is_err(),
        "Save must prevent a native generation from reopening the preserved candidate"
    );
    assert_eq!(
        f.ctx.store.get(&current.id).unwrap().runs[0].agent_host,
        current.runs[0].agent_host,
        "refused resume must not rotate the binding"
    );
    let planning = continuation::preview(&f.ctx, &current.id, Goal::PlanContinue).unwrap();
    continuation::start(
        &f.ctx,
        &planning,
        &planning.digest().unwrap(),
        instruction(),
    )
    .unwrap();
    let implement =
        continuation::preview(&f.ctx, &current.id, Goal::ImplementationPrepare).unwrap();
    let resumed = continuation::start(
        &f.ctx,
        &implement,
        &implement.digest().unwrap(),
        instruction(),
    )
    .unwrap();
    assert_eq!(
        resumed.runs.len(),
        1,
        "goal changes cannot duplicate an owned candidate"
    );
    assert_eq!(resumed.runs[0].worktree, current.runs[0].worktree);
    // Linux also proves the stop against an actual owned stand-in process, not a
    // dead-PID fixture. This is not a real SDK host. Historical open turns remain
    // truthful and never force a forbidden Done/turn-end to settle the writer.
    f.ctx.herdr = corral_herdr::Herdr::with_bin(
        f._tmp.path().join("herdr-must-not-run"),
        Some("isolated-fixture".into()),
    );
    let socket = f.ctx.herdr.socket_path().unwrap();
    f.ctx
        .store
        .update(&current.id, |task| {
            task.status = corral_core::Status::Running;
            let run = task.current_run_mut().unwrap();
            run.outcome = None;
            run.herdr = Some(corral_core::HerdrRef {
                workspace_id: "fixture-workspace".into(),
                pane_id: "worker-pane".into(),
                socket: Some(socket.clone()),
            });
            Ok(())
        })
        .unwrap();
    #[cfg(target_os = "linux")]
    let mut process = crate::agent::bridge::fixture::HostProc::spawn(true);
    let mut host = ts::bind(&f, &target, "worker-pane", "worker-session");
    host.identity.project = current.project.clone().unwrap();
    #[cfg(target_os = "linux")]
    {
        let hello = ts::envelope(
            &host,
            "hello",
            &crate::agent::event_request_id(host.identity.generation, 0),
            serde_json::json!({"ctl_cap_sha256":corral_core::agent::paths::sha256_hex(host.ctl_cap.as_bytes()),
                "session_file":host.sessions.join("session.jsonl"),"pid":process.pid(),
                "start_time":process.start_time(),"pane":host.pane}),
            true,
        );
        ts::committed(&ts::send(&f, &hello, "hello"));
        assert!(!process.reaped());
    }
    #[cfg(not(target_os = "linux"))]
    ts::committed(&ts::hello(&f, &host));
    ts::committed(&ts::send(
        &f,
        &ts::event_input(&host, 1, "turn_started", 1),
        "event",
    ));
    let running = f.ctx.store.get(&current.id).unwrap();
    let run = running.current_run().unwrap();
    std::fs::write(
        run.worktree.join("latest-repair"),
        b"not in the original package\0\xff",
    )
    .unwrap();
    let latest =
        Snapshot::capture(&f.ctx.store, &run.worktree, &run.base_sha, &run.branch).unwrap();
    let paused = continuation::save(
        &f.ctx,
        &running.id,
        &running.contract_revision().unwrap(),
        instruction(),
    )
    .unwrap();
    assert_eq!(
        paused.current_run().unwrap().outcome,
        Some(corral_core::RunOutcome::Cancelled)
    );
    #[cfg(target_os = "linux")]
    {
        assert!(
            process.reaped(),
            "Save must physically stop the bound owned host, not only fence it"
        );
        assert_eq!(
            paused
                .current_run()
                .unwrap()
                .agent_host
                .as_ref()
                .unwrap()
                .binding
                .generation,
            1,
            "stop must not bootstrap a replacement generation"
        );
        eprintln!(
            "C3_NATIVE_STOP: verified owned stand-in exited; no replacement generation or fabricated turn end"
        );
    }
    let next = continuation::preview(&f.ctx, &running.id, Goal::ImplementationPrepare).unwrap();
    let resumed =
        continuation::start(&f.ctx, &next, &next.digest().unwrap(), instruction()).unwrap();
    assert!(
        adopt::pending_seed(&f.ctx, &resumed).unwrap(),
        "explicit continuation must seed the next run from the latest stopped work"
    );
    assert_eq!(resumed.status, corral_core::Status::Ready);
    assert_eq!(
        resumed.runs.len(),
        1,
        "the scheduler still owns next-run allocation"
    );
    let (seed, branch) = adopt::seed_path(&f.ctx, &resumed).unwrap();
    adopt::verify_worktree(&latest, &seed, &branch).unwrap();
    let mut next_run = resumed.current_run().unwrap().clone();
    next_run.id = "R2".into();
    next_run.worktree = seed;
    next_run.branch = branch;
    assert_eq!(
        adopt::start_head(
            &f.ctx,
            &resumed,
            &corral_core::RunHead::of(&next_run).unwrap()
        )
        .unwrap(),
        Some(latest.state.head.clone())
    );
    assert!(
        crate::operating::require(
            &f.ctx.store,
            &resumed,
            corral_core::OperatingActionKind::Dispatch
        )
        .is_ok()
    );
    let taken = takeback::apply(
        &f.ctx,
        &running.id,
        &running.contract_revision().unwrap(),
        instruction(),
    )
    .unwrap();
    let Returned::Ready {
        scratch: Some(slug),
        plan_only: false,
        ..
    } = taken
    else {
        panic!("native writer did not settle: {taken:?}")
    };
    let local = f.ctx.store.get_assistant(&session).unwrap();
    let scratch = local.scratch.iter().find(|s| s.slug == slug).unwrap();
    adopt::verify_worktree(&latest, &scratch.path, &scratch.branch).unwrap();
    let stopped = f.ctx.store.get(&current.id).unwrap();
    let run = stopped.current_run().unwrap();
    assert_eq!(run.outcome, Some(corral_core::RunOutcome::Cancelled));
    let state = run.agent_host.as_ref().unwrap();
    assert_eq!(state.binding.health, corral_core::agent::HostHealth::Fenced);
    assert!(
        state.turns[0].ended.is_none(),
        "do not fabricate an authenticated turn end"
    );
    assert!(stopped.dispatch_hold);
    assert_eq!(f.ctx.store.list().unwrap().len(), 1);
    assert!(crate::agent::open_generation(&f.ctx.store, &target, "stale-pane").is_err());
}
