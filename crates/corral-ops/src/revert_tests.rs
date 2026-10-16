use super::*;
use corral_core::{AutoStage, AutopilotState, ReviewWork, Run, Store};
use corral_herdr::Herdr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Production Git calls inherit only fixture identity and config, never the owner environment.
fn isolated() -> bool {
    let name = std::thread::current().name().unwrap().to_owned();
    if std::env::var("CORRAL_REVERT_TEST").as_deref() == Ok(&name) {
        return false;
    }
    let home = tempfile::tempdir().unwrap();
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &name, "--nocapture"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Revert test")
        .env("GIT_AUTHOR_EMAIL", "revert@test.invalid")
        .env("GIT_COMMITTER_NAME", "Revert test")
        .env("GIT_COMMITTER_EMAIL", "revert@test.invalid")
        .env("GIT_AUTHOR_DATE", "2026-10-16T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-10-16T00:00:00Z")
        .env("CORRAL_REVERT_TEST", &name)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    true
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
}

fn commit(repo: &Path, path: &str, content: &str) -> String {
    fs::write(repo.join(path), content).unwrap();
    git(repo, &["add", "--", path]);
    git(repo, &["commit", "-qm", path]);
    git(repo, &["rev-parse", "HEAD"])
}

struct Fx {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    repo: PathBuf,
    task: Task,
    before: String,
    landed: String,
    originals: Vec<String>,
}

impl Fx {
    fn new(strategy: crate::ApproveStrategy) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("app");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-qb", "main"]);
        git(&repo, &["config", "user.name", "Revert test"]);
        git(&repo, &["config", "user.email", "revert@test.invalid"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        git(&repo, &["config", "core.hooksPath", "/dev/null"]);
        let dispatch_base = commit(&repo, "task.txt", "original\n");
        let worktree = tmp.path().join("worker");
        if strategy == crate::ApproveStrategy::Rebase {
            corral_git::worktree_add(&repo, &worktree, "corral/task", "main").unwrap();
        }
        let before = commit(&repo, "peer-before.txt", "keep before\n");
        if strategy != crate::ApproveStrategy::Rebase {
            corral_git::worktree_add(&repo, &worktree, "corral/task", "main").unwrap();
        }
        commit(&worktree, "task.txt", "task change\n");
        commit(&worktree, "second.txt", "task second\n");
        let landed = match strategy {
            crate::ApproveStrategy::FastForward | crate::ApproveStrategy::Rebase => {
                corral_git::land(
                    &repo,
                    &worktree,
                    "corral/task",
                    "main",
                    match strategy {
                        crate::ApproveStrategy::FastForward => {
                            corral_git::MergeStrategy::FastForward
                        }
                        crate::ApproveStrategy::Rebase => corral_git::MergeStrategy::Rebase,
                        crate::ApproveStrategy::KeepBranch => unreachable!(),
                    },
                )
                .unwrap()
            }
            crate::ApproveStrategy::KeepBranch => git(&worktree, &["rev-parse", "HEAD"]),
        };
        let first = git(&worktree, &["rev-parse", "HEAD^"]);
        let mut store = Store::init(tmp.path()).unwrap();
        store.config.projects.insert(
            "app".into(),
            serde_json::from_value(json!({"path": "app", "base": "main"})).unwrap(),
        );
        store.save_config().unwrap();
        let task = store
            .create("Landed task", Some("app"), Status::Done, "")
            .unwrap();
        let run: Run = serde_json::from_value(json!({
            "id":"R1", "agent":"fake", "worktree":worktree, "branch":"corral/task",
            "base":"main", "base_sha":dispatch_base,
            "started":"2026-10-16T00:00:00Z", "ended":"2026-10-16T01:00:00Z",
            "outcome":"done",
        }))
        .unwrap();
        let task = store
            .update(&task.id, |task| {
                task.runs.push(run);
                Ok(())
            })
            .unwrap();
        store
            .log_event(
                Some(&task.id),
                "approved",
                json!({
                    "run":"R1", "project":"app", "base":"main", "sha":landed,
                    "previous_sha":before,
                    "strategy":match strategy {
                        crate::ApproveStrategy::FastForward => "fast_forward",
                        crate::ApproveStrategy::Rebase => "rebase",
                        crate::ApproveStrategy::KeepBranch => "keep_branch",
                    },
                    "acceptance_mode":"owner_override",
                }),
            )
            .unwrap();
        let ctx = Ctx {
            store,
            herdr: Herdr::with_bin(tmp.path().join("herdr-must-not-run"), None),
            resource_probe: crate::resources::FixedResourceProbe::arc(),
        };
        Self {
            _tmp: tmp,
            ctx,
            repo,
            task,
            before,
            originals: vec![landed.clone(), first],
            landed,
        }
    }
}

#[test]
fn revert_reopens_backlog_held_and_preserves_peers_outside_exact_approval_range() {
    if isolated() {
        return;
    }
    // Given two task commits after a peer landed since dispatch, plus later unrelated work.
    let fx = Fx::new(crate::ApproveStrategy::FastForward);
    let later = commit(&fx.repo, "peer-after.txt", "keep after\n");
    fx.ctx
        .store
        .update(&fx.task.id, |task| {
            task.queued = true;
            task.dispatch_plan = Some(corral_core::DispatchPlan {
                plan: false,
                choice: corral_core::RetryPlan {
                    profile: "fake".into(),
                    model: None,
                    effort: None,
                    switch: None,
                },
                route: corral_core::RouteDecision {
                    profile: "fake".into(),
                    harness: "fake".into(),
                    model: None,
                    effort: None,
                    route: corral_core::RouteSource::Fallback,
                    warnings: Vec::new(),
                },
                base: Some("main".into()),
                mode: corral_core::RunMode::default(),
            });
            task.launch = Some(corral_core::LaunchReservation {
                approved_plan: None,
                socket: None,
                stage: corral_core::LaunchStage::Reserved,
                reserved: false,
                error: None,
            });
            task.review_wait = Some(ReviewWork::Reviewer);
            task.review_reservation = Some(ReviewWork::Gate);
            task.current_run_mut().unwrap().autopilot = Some(AutopilotState {
                stage: AutoStage::Done,
                reason: None,
                sha: Some(fx.landed.clone()),
                at: "2026-10-16T01:00:00Z".parse().unwrap(),
            });
            Ok(())
        })
        .unwrap();

    // When the public operation reverts that task.
    let result = revert(&fx.ctx, &fx.task.id).unwrap();

    // Then the inverse is appended, only task changes disappear, and readmission is held.
    assert_eq!(result.original_shas, fx.originals);
    assert_eq!(result.task.status, Status::Backlog);
    assert!(result.task.dispatch_hold);
    assert!(!result.task.queued);
    assert!(result.task.dispatch_plan.is_none());
    assert!(result.task.launch.is_none());
    assert!(result.task.review_reservation.is_none());
    assert!(result.task.review_wait.is_none());
    assert!(result.task.current_run().unwrap().autopilot.is_none());
    assert_eq!(fx.ctx.store.get(&fx.task.id).unwrap(), result.task);
    assert_eq!(git(&fx.repo, &["rev-parse", "HEAD^"]), later);
    assert_eq!(git(&fx.repo, &["rev-parse", "HEAD"]), result.revert_shas[0]);
    assert_eq!(
        fs::read_to_string(fx.repo.join("task.txt")).unwrap(),
        "original\n"
    );
    assert!(!fx.repo.join("second.txt").exists());
    assert_eq!(
        fs::read_to_string(fx.repo.join("peer-before.txt")).unwrap(),
        "keep before\n"
    );
    assert_eq!(
        fs::read_to_string(fx.repo.join("peer-after.txt")).unwrap(),
        "keep after\n"
    );
    assert!(corral_git::is_clean(&fx.repo).unwrap());
    let event = fx
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "reverted")
        .unwrap();
    assert_eq!(event.detail["original_shas"], json!(fx.originals));
    assert_eq!(event.detail["revert_shas"], json!(result.revert_shas));
    assert_eq!(event.detail["landing_base_before"], fx.before);
    assert_eq!(event.detail["base_before"], later);
}

#[test]
fn revert_uses_actual_rebased_landing_shas_not_dispatch_commits() {
    if isolated() {
        return;
    }
    // Given a branch actually rebased by the production landing helper after a peer advanced main.
    let fx = Fx::new(crate::ApproveStrategy::Rebase);
    // When it is reverted from its durable rebase approval.
    let result = revert(&fx.ctx, &fx.task.id).unwrap();
    // Then only the rebased task range is undone and the peer's commit survives.
    assert_eq!(result.original_shas, fx.originals);
    assert_eq!(
        fs::read_to_string(fx.repo.join("task.txt")).unwrap(),
        "original\n"
    );
    assert!(!fx.repo.join("second.txt").exists());
    assert_eq!(
        fs::read_to_string(fx.repo.join("peer-before.txt")).unwrap(),
        "keep before\n"
    );
    assert!(corral_git::is_ancestor(&fx.repo, &fx.landed, "HEAD").unwrap());
}

#[test]
fn revert_conflicting_later_commit_reports_sha_and_keeps_task_base_and_events_unchanged() {
    if isolated() {
        return;
    }
    // Given later owner work on the task's changed line.
    let fx = Fx::new(crate::ApproveStrategy::FastForward);
    let later = commit(&fx.repo, "task.txt", "later owner change\n");
    let task = fx.ctx.store.get(&fx.task.id).unwrap();
    let events = fs::read(fx.ctx.store.dir().join("events.jsonl")).unwrap();
    let index = git(&fx.repo, &["write-tree"]);
    // When reverting would conflict.
    let error = revert(&fx.ctx, &fx.task.id).unwrap_err().to_string();
    // Then the offending SHA/path are reported and no state is changed.
    assert!(error.contains(&later), "{error}");
    assert!(error.contains("task.txt"), "{error}");
    assert_eq!(fx.ctx.store.get(&fx.task.id).unwrap(), task);
    assert_eq!(
        fs::read(fx.ctx.store.dir().join("events.jsonl")).unwrap(),
        events
    );
    assert_eq!(git(&fx.repo, &["rev-parse", "HEAD"]), later);
    assert_eq!(git(&fx.repo, &["write-tree"]), index);
    assert!(corral_git::is_clean(&fx.repo).unwrap());
}

#[test]
fn revert_refuses_keep_branch_acceptance_without_changing_task_or_base() {
    if isolated() {
        return;
    }
    // Given Done via KeepBranch, which is not a landing.
    let fx = Fx::new(crate::ApproveStrategy::KeepBranch);
    let before = git(&fx.repo, &["rev-parse", "HEAD"]);
    let task = fx.ctx.store.get(&fx.task.id).unwrap();
    // When revert is requested.
    let result = revert(&fx.ctx, &fx.task.id);
    // Then it refuses without treating dispatch ancestry as a landing range.
    assert!(result.is_err());
    assert_eq!(fx.ctx.store.get(&fx.task.id).unwrap(), task);
    assert_eq!(git(&fx.repo, &["rev-parse", "HEAD"]), before);
    assert!(corral_git::is_clean(&fx.repo).unwrap());
}

#[test]
fn revert_refuses_tasks_with_no_approval() {
    if isolated() {
        return;
    }
    // Given a Done task whose run has no durable approval.
    let fx = Fx::new(crate::ApproveStrategy::FastForward);
    let mut task = fx
        .ctx
        .store
        .create("Not landed", Some("app"), Status::Done, "")
        .unwrap();
    task.runs = fx.task.runs.clone();
    fx.ctx.store.save(&task).unwrap();
    let before = git(&fx.repo, &["rev-parse", "HEAD"]);
    // When its status alone is offered as landing evidence.
    let result = revert(&fx.ctx, &task.id);
    // Then no approximation is used and the base is untouched.
    assert!(result.is_err());
    assert_eq!(git(&fx.repo, &["rev-parse", "HEAD"]), before);
}

#[test]
fn revert_refuses_repeat_even_if_task_status_is_manually_restored_to_done() {
    if isolated() {
        return;
    }
    // Given an already reverted task, with a manual status edit that cannot authorize replay.
    let fx = Fx::new(crate::ApproveStrategy::FastForward);
    let first = revert(&fx.ctx, &fx.task.id).unwrap();
    fx.ctx
        .store
        .update(&fx.task.id, |task| {
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();
    let task = fx.ctx.store.get(&fx.task.id).unwrap();
    // When the same durable landing is requested again.
    let result = revert(&fx.ctx, &fx.task.id);
    // Then no duplicate inverse or event is appended.
    assert!(result.is_err());
    assert_eq!(git(&fx.repo, &["rev-parse", "HEAD"]), first.revert_shas[0]);
    assert_eq!(fx.ctx.store.get(&fx.task.id).unwrap(), task);
    assert_eq!(
        fx.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "reverted")
            .count(),
        1
    );
    assert!(corral_git::is_clean(&fx.repo).unwrap());
}

fn configure_hook(fx: &mut Fx) {
    fx.ctx
        .store
        .config
        .projects
        .get_mut("app")
        .unwrap()
        .post_land = vec![corral_core::PostLandHook {
        name: "inspect".into(),
        argv: vec!["/usr/bin/env".into()],
        cwd: ".".into(),
        env: std::collections::BTreeMap::new(),
        timeout_secs: 5,
        max_attempts: 1,
        identity: None,
    }];
    fx.ctx.store.save_config().unwrap();
}

#[test]
fn reverted_base_runs_durable_post_land_hooks_bound_to_the_new_sha() {
    if isolated() {
        return;
    }
    // Given an opt-in operational hook on base advancement.
    let mut fx = Fx::new(crate::ApproveStrategy::FastForward);
    configure_hook(&mut fx);
    // When a task is reverted.
    let result = revert(&fx.ctx, &fx.task.id).unwrap();
    // Then durable obligations execute once on the exact inverse, after Backlog is saved.
    let operations = crate::post_land_operations(&fx.ctx, Some("app")).unwrap();
    assert_eq!(operations.len(), 1);
    let op = &operations[0];
    assert!(op.ready(), "{op:?}");
    assert_eq!(op.base_before, fx.landed);
    assert_eq!(op.landed_sha, result.revert_shas[0]);
    assert_eq!(op.hooks[0].attempts.len(), 1);
    assert_eq!(
        op.hooks[0].attempts[0].env["CORRAL_LANDED_SHA"],
        result.revert_shas[0]
    );
    assert!(
        fs::read_to_string(&op.hooks[0].attempts[0].log)
            .unwrap()
            .contains(&format!("CORRAL_LANDED_SHA={}", result.revert_shas[0]))
    );
    let saved = fx.ctx.store.get(&fx.task.id).unwrap();
    assert_eq!(saved.status, Status::Backlog);
    assert!(saved.dispatch_hold);
}

#[test]
fn unresolved_post_land_obligation_blocks_revert_before_base_or_task_mutation() {
    if isolated() {
        return;
    }
    // Given an uncertain pre-existing operation which must not be bypassed by reverting.
    let mut fx = Fx::new(crate::ApproveStrategy::FastForward);
    configure_hook(&mut fx);
    let run = fx.task.current_run().unwrap();
    let mut operation =
        crate::hooks::prepare(&fx.ctx, &fx.task, run, &fx.repo, &fx.before, &fx.landed)
            .unwrap()
            .unwrap();
    operation.landed = true;
    operation.hooks[0].state = crate::HookState::Failed;
    operation.hooks[0].reason = Some("fixture unresolved operation".into());
    operation.notified = true;
    crate::hooks::save(&fx.ctx, &operation).unwrap();
    // When revert is requested.
    let result = revert(&fx.ctx, &fx.task.id);
    // Then operational readiness still blocks advancement and the task is not reopened.
    assert!(result.is_err());
    assert_eq!(git(&fx.repo, &["rev-parse", "HEAD"]), fx.landed);
    assert_eq!(fx.ctx.store.get(&fx.task.id).unwrap().status, Status::Done);
    assert!(corral_git::is_clean(&fx.repo).unwrap());
    assert!(
        !fx.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.kind == "reverted")
    );
}

#[test]
fn approval_lock_blocks_revert_for_normalized_task_identifiers() {
    if isolated() {
        return;
    }
    // Given an acceptance or autopilot operation holding the canonical task lease.
    let fx = Fx::new(crate::ApproveStrategy::FastForward);
    let _lock = fx
        .ctx
        .store
        .try_lock(&format!("autopilot-{}", fx.task.id))
        .unwrap()
        .unwrap();
    // When the caller uses an abbreviated ID to attempt a concurrent revert.
    let result = revert(&fx.ctx, "1");
    // Then it cannot bypass the canonical lease or mutate the base/task.
    assert!(result.is_err());
    assert_eq!(git(&fx.repo, &["rev-parse", "HEAD"]), fx.landed);
    assert_eq!(fx.ctx.store.get(&fx.task.id).unwrap(), fx.task);
}

#[test]
fn busy_peer_task_blocks_revert_without_base_or_task_mutation() {
    if isolated() {
        return;
    }
    // Given a project peer being inspected or reviewed under its task lock.
    let fx = Fx::new(crate::ApproveStrategy::FastForward);
    let peer = fx
        .ctx
        .store
        .create("Peer", Some("app"), Status::Review, "")
        .unwrap();
    let _lock = fx.ctx.store.try_lock(&peer.id).unwrap().unwrap();
    // When revert reaches the board/task transaction.
    let result = revert(&fx.ctx, &fx.task.id);
    // Then it refuses rather than changing the base underneath that peer.
    assert!(result.is_err());
    assert_eq!(git(&fx.repo, &["rev-parse", "HEAD"]), fx.landed);
    assert_eq!(fx.ctx.store.get(&fx.task.id).unwrap(), fx.task);
}

#[test]
fn missing_pre_landing_base_never_falls_back_to_dispatch_base() {
    if isolated() {
        return;
    }
    // Given an old approval that omits the exact pre-landing base.
    let fx = Fx::new(crate::ApproveStrategy::FastForward);
    fx.ctx
        .store
        .log_event(
            Some(&fx.task.id),
            "approved",
            json!({
                "run":"R1", "project":"app", "base":"main", "sha":fx.landed,
                "strategy":"fast_forward",
            }),
        )
        .unwrap();
    // When task reversion is requested.
    let result = revert(&fx.ctx, &fx.task.id);
    // Then the dispatch base cannot accidentally select peer commits.
    assert!(result.is_err());
    assert_eq!(git(&fx.repo, &["rev-parse", "HEAD"]), fx.landed);
    assert_eq!(fx.ctx.store.get(&fx.task.id).unwrap(), fx.task);
}
