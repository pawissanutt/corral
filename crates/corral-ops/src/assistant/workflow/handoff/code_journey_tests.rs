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
}
