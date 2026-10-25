//! Seam tests: other harnesses never reach a hook; pi-native profiles are refused before any
//! worktree or pane; run-end cleanup; the sync hook; op and subcommand routing.
use std::collections::HashMap;
use std::path::Path;

use corral_core::agent::{AgentRole, ContainerRef, HostHealth, QuestionState, paths};
use corral_core::inbox::DeliveryMode;
use corral_core::{ProfileChoice, ResolvedProfile, Run, RunOutcome, Status, Task};
use serde_json::json;

use super::super::test_support::{self as ts, Fx, fx};
use super::super::{AgentOp, BindTarget, CallResponse, call, tool_request_id};
use super::*;

fn with_harness(id: &str, pane: &str, harness: &str) -> Run {
    let mut run = ts::run(id, pane);
    run.harness = Some(harness.into());
    run
}

/// A Running task whose current run and one review child use `harness`.
fn task_with(fx: &Fx, harness: &str) -> Task {
    let task = fx
        .ctx
        .store
        .create("Seams", None, Status::Running, "")
        .unwrap();
    fx.ctx
        .store
        .update(&task.id, |t| {
            t.project = Some("corral".into());
            let mut run = with_harness("R1", "p1", harness);
            run.review_runs
                .push(with_harness("R1-review1", "p2", harness));
            t.runs.push(run);
            Ok(())
        })
        .unwrap()
}

fn pi_profile(fx: &mut Fx, vendor: &str) {
    let profile = serde_json::from_value(json!({
        "harness": "pi-native", "model": "gpt-6.1-sol", "vendor": vendor, "effort": "high"
    }))
    .unwrap();
    fx.ctx.store.config.profiles.insert("pi".into(), profile);
}

fn resolved(fx: &Fx, profile: Option<&str>) -> ResolvedProfile {
    let choice = ProfileChoice {
        profile: profile.map(str::to_owned),
        ..ProfileChoice::default()
    };
    fx.ctx
        .store
        .config
        .resolve_profile(&choice, None, None)
        .unwrap()
}

/// Exercises every seam's gate for `task`'s runs and returns the seams that reached a hook.
fn exercise(fx: &Fx, task: &Task, profile: &ResolvedProfile) -> Vec<&'static str> {
    take_consulted();
    let ctx = &fx.ctx;
    let run = &task.runs[0];
    let child = &run.review_runs[0];
    if sync::wants(task) {
        sync::task(ctx, task, jiff::Timestamp::now()).unwrap();
    }
    sync::pass(ctx).unwrap();
    for target in [run, child] {
        let mut copy = target.clone();
        crate::usage::collect(&mut copy, &ctx.store);
        let _ = crate::delivery::adapters::pi_native(target);
    }
    let harness = run.harness.as_deref().unwrap();
    let _ = crate::delivery::owner::launch_mode(harness);
    let _ = crate::delivery::owner::review_launch_mode(harness);
    let _ = preflight::check(ctx, AgentRole::Worker, profile);
    let _ = review::preflight(ctx, profile);
    let _ = pm::preflight(ctx, Some(profile));
    let _ = review::command(ctx, task, run, profile, "p-review");
    crate::provider_stall::advance_agent(ctx, task, &HashMap::new(), jiff::Timestamp::now())
        .unwrap();
    let mut done = task.clone();
    done.status = Status::Done;
    if task_is_pi_native(&done) {
        cleanup::run_end(ctx, &done).unwrap();
    }
    if configured(&ctx.store.config) {
        doctor::checks(ctx).unwrap();
    }
    take_consulted()
}

#[test]
fn other_harnesses_never_reach_an_agent_hook() {
    // Given an OmO task and the default OmO profile, with no pi-native profile configured:
    let fx = fx();
    let task = task_with(&fx, "omo");
    let omo = resolved(&fx, None);
    assert_eq!(omo.harness, "omo");
    // When every seam runs for it:
    let consulted = exercise(&fx, &task, &omo);
    // Then no hook is consulted and the existing decisions are unchanged.
    assert_eq!(consulted, Vec::<&str>::new());
    assert!(!task_is_pi_native(&task) && !configured(&fx.ctx.store.config));
    assert_eq!(
        crate::delivery::owner::launch_mode("omo"),
        DeliveryMode::Inbox
    );
    assert_eq!(
        crate::delivery::owner::review_launch_mode("omo"),
        DeliveryMode::Legacy
    );
    assert!(review::command(&fx.ctx, &task, &task.runs[0], &omo, "p-review").is_none());
}

#[test]
fn pi_native_runs_reach_every_hook() {
    // Given the same task on the pi-native harness and a pi-native profile:
    let mut fx = fx();
    pi_profile(&mut fx, "openai");
    let task = task_with(&fx, "pi-native");
    let pi = resolved(&fx, Some("pi"));
    // When every seam runs, each reaches its hook, so the other-harness test can fail.
    let consulted = exercise(&fx, &task, &pi);
    for seam in [
        "sync",
        "usage",
        "delivery",
        "preflight",
        "review",
        "stall",
        "cleanup",
        "doctor",
    ] {
        assert!(
            consulted.contains(&seam),
            "{seam} missing from {consulted:?}"
        );
    }
    assert_eq!(
        crate::delivery::owner::review_launch_mode("pi-native"),
        DeliveryMode::Inbox
    );
    assert!(pi.model_args.is_empty(), "{:?}", pi.model_args);
    assert_eq!(pi.command, corral_core::PI_NATIVE_COMMAND);
}

#[test]
fn pi_native_usage_note_names_the_missing_collector() {
    let fx = fx();
    let task = task_with(&fx, "pi-native");
    for mut run in [task.runs[0].clone(), task.runs[0].review_runs[0].clone()] {
        crate::usage::collect(&mut run, &fx.ctx.store);
        assert_eq!(run.usage, None);
        assert_eq!(
            run.usage_note.as_deref(),
            Some("no bound Pi session: the run has no host binding")
        );
    }
}

fn write(dir: &Path, file: &str, text: &str) {
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn refusal(error: anyhow::Error) -> String {
    match error.downcast_ref::<preflight::Refusal>() {
        Some(refusal) => format!("{}: {}", refusal.code, refusal.detail),
        None => format!("{error:#}"),
    }
}

#[test]
fn preflight_names_the_missing_piece_and_its_owner() {
    // Given a pi-native profile and an asset directory filled in one step at a time:
    let mut fx = fx();
    pi_profile(&mut fx, "openai");
    let pi = resolved(&fx, Some("pi"));
    let check = |fx: &Fx, role| {
        preflight::check(&fx.ctx, role, &pi)
            .err()
            .map(refusal)
            .unwrap_or_default()
    };
    assert!(check(&fx, AgentRole::Worker).starts_with("assets_unavailable: no pi-native asset"));
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("assets");
    fx.ctx.store.config.agent.assets_dir = Some(dir.clone());
    assert!(check(&fx, AgentRole::Worker).starts_with("assets_unavailable: asset directory"));
    std::fs::create_dir(&dir).unwrap();
    assert!(check(&fx, AgentRole::Worker).starts_with("compat_unavailable: no GO"));
    write(&dir, "compat.json", r#"{"verdict":"NO-GO"}"#);
    assert!(check(&fx, AgentRole::Worker).ends_with("(owner host-bridge)"));
    write(&dir, "compat.json", r#"{"verdict":"GO"}"#);
    // Then each role names its own owner, then delivery and auth name theirs.
    for (role, expected) in [
        (
            AgentRole::Worker,
            "role_unavailable: worker, owner worker-role",
        ),
        (
            AgentRole::Reviewer,
            "role_unavailable: reviewer, owner reviewer-role",
        ),
        (AgentRole::Pm, "role_unavailable: pm, owner pm-role"),
    ] {
        assert!(
            check(&fx, role).starts_with(expected),
            "{}",
            check(&fx, role)
        );
        write(&dir, &format!("src/roles/{}/index.ts", role.as_str()), "");
    }
    assert!(check(&fx, AgentRole::Worker).starts_with("delivery_unavailable: owner inbox-adapter"));
    write(&dir, "src/delivery/index.ts", "");
    assert!(check(&fx, AgentRole::Pm).starts_with("credentials_unavailable: owner host-install"));
    write(&dir, "src/auth/index.ts", "");
    for role in [AgentRole::Worker, AgentRole::Reviewer, AgentRole::Pm] {
        assert_eq!(check(&fx, role), "");
    }
    // A fixture installation is accepted only through an explicit assets_dir.
    write(&dir, "fixture.json", "{}");
    assert_eq!(check(&fx, AgentRole::Worker), "");
    let implicit = preflight::assets(&dir, AgentRole::Worker, false).unwrap_err();
    assert_eq!(implicit.code, "fixture_assets_not_allowed");
    // An anthropic vendor is refused before any asset is consulted.
    pi_profile(&mut fx, "anthropic");
    let anthropic = resolved(&fx, Some("pi"));
    let error = preflight::check(&fx.ctx, AgentRole::Worker, &anthropic).unwrap_err();
    assert!(
        format!("{error:#}")
            .starts_with("vendor_refused: pi-native profile 'pi' uses vendor 'anthropic'"),
        "{error:#}"
    );
}

#[test]
fn dispatch_and_pm_open_refuse_before_any_worktree_or_pane() {
    // Given a pi-native profile with no installed assets, a Ready task in a real project, and a
    // terminal engine that does not exist (any pane or workspace call would fail differently):
    let mut fx = fx();
    let repo = tempfile::tempdir().unwrap();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["commit", "-q", "--allow-empty", "-m", "base"],
    ] {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .stdin(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }
    fx.ctx.store.config.projects.insert(
        "corral".into(),
        serde_json::from_value(json!({"path": repo.path()})).unwrap(),
    );
    pi_profile(&mut fx, "openai");
    fx.ctx.store.config.pm.agent = "pi".into();
    fx.ctx.herdr = corral_herdr::Herdr::with_bin(repo.path().join("no-herdr"), None);
    // Machine pressure from a parallel test run must not refuse first.
    fx.ctx.resource_probe = crate::resources::FixedResourceProbe::arc();
    let task = fx
        .ctx
        .store
        .create("Pi task", Some("corral"), Status::Ready, "")
        .unwrap();
    // When dispatch runs with the pi-native profile, and the PM opens on it:
    let opts = crate::DispatchOpts {
        profile: Some("pi".into()),
        ..Default::default()
    };
    // Then dispatch refuses with the preflight reason and creates no run or worktree, both for
    // missing assets and, before any asset is read, for a vendor outside the allowlist.
    for (vendor, code) in [
        ("openai", "assets_unavailable"),
        ("anthropic", "vendor_refused"),
    ] {
        pi_profile(&mut fx, vendor);
        let error = refusal(crate::dispatch(&fx.ctx, &task.id, &opts).unwrap_err());
        assert!(error.starts_with(code), "{vendor}: {error}");
        let after = fx.ctx.store.get(&task.id).unwrap();
        assert!(after.runs.is_empty() && after.launch.is_none());
        assert!(!repo.path().join(".worktrees").exists());
    }
    pi_profile(&mut fx, "openai");
    let error = refusal(crate::open_pm(&fx.ctx).unwrap_err());
    assert!(error.starts_with("assets_unavailable"), "PM open: {error}");
}

fn run_dir(fx: &Fx, root: &Path, recipient: corral_core::inbox::Recipient) -> std::path::PathBuf {
    let dir = super::super::launch::runtime_dir_in(root, &fx.ctx.store, &recipient).unwrap();
    std::fs::create_dir_all(dir.join("g1")).unwrap();
    dir
}

fn sessions(fx: &Fx, container: &ContainerRef) -> std::path::PathBuf {
    let dir = paths::container_sessions_dir(&fx.ctx.store, container);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("session.jsonl"), "{}\n").unwrap();
    dir
}

#[test]
fn run_end_cleanup_removes_only_ended_runtime_and_keeps_sessions() {
    use corral_core::inbox::Recipient;
    let fx = fx();
    let root = tempfile::tempdir().unwrap();
    // Given a lost (resumable) pi-native run with a superseded and an open, host-lost child:
    let task = task_with(&fx, "pi-native");
    let id = task.id.clone();
    let worker = ContainerRef::Worker {
        task: id.clone(),
        run: "R1".into(),
    };
    let child = |review: &str| ContainerRef::Reviewer {
        task: id.clone(),
        run: "R1".into(),
        review: review.into(),
    };
    fx.ctx
        .store
        .update(&id, |t| {
            t.status = Status::NeedsMe;
            t.runs[0]
                .review_runs
                .push(with_harness("R1-review2", "p3", "pi-native"));
            Ok(())
        })
        .unwrap();
    for target in [
        BindTarget::Worker {
            task: id.clone(),
            run: "R1".into(),
        },
        BindTarget::Reviewer {
            task: id.clone(),
            run: "R1".into(),
            review: "R1-review2".into(),
            callback: "g1:c1".into(),
        },
    ] {
        ts::bind(&fx, &target, "p9", "s-lost");
    }
    let task = fx
        .ctx
        .store
        .update(&id, |t| {
            let run = &mut t.runs[0];
            run.agent_host.as_mut().unwrap().binding.health = HostHealth::Lost;
            let open = run.review_runs.last_mut().unwrap();
            open.agent_host.as_mut().unwrap().binding.health = HostHealth::Lost;
            Ok(())
        })
        .unwrap();
    let dirs = [
        (
            run_dir(
                &fx,
                root.path(),
                Recipient::Run {
                    task: id.clone(),
                    run: "R1".into(),
                },
            ),
            sessions(&fx, &worker),
        ),
        (
            run_dir(
                &fx,
                root.path(),
                Recipient::ReviewRun {
                    task: id.clone(),
                    run: "R1".into(),
                    review: "R1-review1".into(),
                },
            ),
            sessions(&fx, &child("R1-review1")),
        ),
        (
            run_dir(
                &fx,
                root.path(),
                Recipient::ReviewRun {
                    task: id.clone(),
                    run: "R1".into(),
                    review: "R1-review2".into(),
                },
            ),
            sessions(&fx, &child("R1-review2")),
        ),
    ];
    // When run-end cleanup runs while the run is lost:
    let removed = cleanup::run_end_in(&fx.ctx, &task, root.path()).unwrap();
    // Then only the closed child's runtime goes; the lost run and open child keep both.
    assert_eq!(removed, vec![dirs[1].0.clone()]);
    assert!(dirs[0].0.exists() && dirs[2].0.exists());
    assert!(
        dirs.iter()
            .all(|(_, sessions)| sessions.join("session.jsonl").exists())
    );
    // And once the run is terminal, every runtime directory goes and every session stays.
    let mut terminal = task.clone();
    terminal.status = Status::Done;
    terminal.runs[0].outcome = Some(RunOutcome::Done);
    let removed = cleanup::run_end_in(&fx.ctx, &terminal, root.path()).unwrap();
    assert_eq!(removed, vec![dirs[0].0.clone(), dirs[2].0.clone()]);
    assert!(dirs
        .iter()
        .all(|(runtime, sessions)| !runtime.exists() && sessions.join("session.jsonl").exists()));
}

#[test]
fn sync_hook_calls_once_per_open_container_and_terminalizes_closed_ones() {
    // Given a Review task whose pi-native reviewer asked a question, then was superseded by a
    // new review child, with the worker run's host healthy:
    let fx = fx();
    let (id, target) = ts::reviewer_task(&fx);
    let host = ts::bind(&fx, &target, "p2", "s-rev");
    assert!(ts::hello(&fx, &host).receipt().is_some());
    let payload = json!({"kind": "clarification", "text": "which?", "blocking": true});
    let ask = ts::envelope(
        &host,
        "ask",
        &tool_request_id("s-rev", "c1"),
        payload,
        false,
    );
    let key = match ts::send(&fx, &ask, "ask") {
        CallResponse::Receipt { receipt, .. } => receipt.result["key"].as_str().unwrap().to_owned(),
        other => panic!("ask refused: {other:?}"),
    };
    let worker = BindTarget::Worker {
        task: id.clone(),
        run: "R1".into(),
    };
    ts::bind(&fx, &worker, "p1", "s-work");
    fx.ctx
        .store
        .update(&id, |t| {
            t.runs[0].agent_host.as_mut().unwrap().binding.health = HostHealth::Healthy;
            t.runs[0]
                .review_runs
                .push(with_harness("R1-review2", "p3", "pi-native"));
            Ok(())
        })
        .unwrap();
    let task = fx.ctx.store.get(&id).unwrap();
    assert!(sync::wants(&task));
    // When sync calls the hook:
    take_consulted();
    let synced = sync::task(&fx.ctx, &task, jiff::Timestamp::now()).unwrap();
    let open = &synced.open;
    // Then exactly the two open containers are reported, with their binding health,
    // (plus one run-end cleanup for the superseded child),
    assert_eq!(take_consulted(), vec!["cleanup", "sync", "sync"]);
    let reported: Vec<(ContainerRef, bool)> = open
        .iter()
        .map(|c| (c.container.clone(), c.healthy))
        .collect();
    assert_eq!(
        reported,
        vec![
            (
                ContainerRef::Worker {
                    task: id.clone(),
                    run: "R1".into()
                },
                true
            ),
            (
                ContainerRef::Reviewer {
                    task: id.clone(),
                    run: "R1".into(),
                    review: "R1-review2".into()
                },
                false
            ),
        ]
    );
    let current = fx.ctx.store.get(&id).unwrap();
    assert!(synced.current_worker_healthy(&current));
    // and the superseded child's question is persisted cancelled.
    let state = corral_core::agent::container_state(
        &current,
        &ContainerRef::Reviewer {
            task: id.clone(),
            run: "R1".into(),
            review: "R1-review1".into(),
        },
    )
    .unwrap();
    let question = state.questions.iter().find(|q| q.key == key).unwrap();
    assert_eq!(question.state, QuestionState::Cancelled);
}

#[test]
fn no_role_has_unlanded_ops_after_the_pm_role_lands() {
    let fx = fx();
    let id = ts::worker_task(&fx, "p1");
    let worker = ts::bind(
        &fx,
        &BindTarget::Worker {
            task: id,
            run: "R1".into(),
        },
        "p1",
        "s-w",
    );
    assert!(ts::hello(&fx, &worker).receipt().is_some());
    let (_, target) = ts::reviewer_task(&fx);
    let reviewer = ts::bind(&fx, &target, "p2", "s-r");
    assert!(ts::hello(&fx, &reviewer).receipt().is_some());
    let pm = ts::bind(
        &fx,
        &BindTarget::Pm {
            project: "corral".into(),
        },
        "pm",
        "s-pm",
    );
    assert!(ts::hello(&fx, &pm).receipt().is_some());
    let mut routed = 0;
    for (host, role) in [
        (&worker, AgentRole::Worker),
        (&reviewer, AgentRole::Reviewer),
        (&pm, AgentRole::Pm),
    ] {
        for op in AgentOp::ALL
            .into_iter()
            .filter(|op| op.roles().contains(&role))
        {
            let Some(owner) = op.owner(role) else {
                continue;
            };
            let request = tool_request_id(&host.identity.session_id, op.as_str());
            let input = ts::envelope(host, op.as_str(), &request, json!({}), false);
            let response = call(&fx.ctx, op.as_str(), &input).unwrap();
            let rejection = response
                .rejection()
                .unwrap_or_else(|| panic!("{op:?}: {response:?}"));
            assert_eq!(rejection.code.as_str(), "op_unavailable", "{op:?}");
            assert_eq!(rejection.owner, Some(owner), "{op:?}");
            routed += 1;
        }
    }
    // All three roles are installed; no advertised operation points at an unlanded child.
    assert_eq!(routed, 0);
}

#[test]
fn every_agent_subcommand_routes_to_its_owner_module() {
    use super::super::cli::{self, AgentCommand, AgentTarget, OpUnavailable};
    let fx = fx();
    let target = AgentTarget::Pm;
    let results = [
        (
            AgentCommand::Cancel,
            cli::cancel(
                &fx.ctx,
                &cli::CancelArgs {
                    target: target.clone(),
                    turn: None,
                },
            ),
        ),
        (
            AgentCommand::Resume,
            cli::resume(
                &fx.ctx,
                &cli::ResumeArgs {
                    target,
                    replace: false,
                },
            ),
        ),
    ];
    // Every subcommand has landed: cancel and resume reach recovery, which refuses an unbound PM.
    for (command, result) in results {
        let error = result.unwrap_err();
        assert!(
            error.downcast_ref::<OpUnavailable>().is_none(),
            "{command:?}"
        );
        assert_eq!(command.owner(), "recovery");
        assert!(
            error.to_string().contains("no Pi-native PM is bound"),
            "{error}"
        );
    }
}

#[test]
fn a_held_reviewer_stays_live_and_its_withdrawal_restores_review() {
    use crate::questions::Actor;
    // Given a bound Pi reviewer whose blocking question the PM escalated to the owner:
    let fx = fx();
    let (id, target) = ts::reviewer_task(&fx);
    let host = ts::bind(&fx, &target, "p2", "s-rev");
    assert!(ts::hello(&fx, &host).receipt().is_some());
    let payload = json!({"kind": "clarification", "text": "which?", "blocking": true});
    let ask = ts::envelope(
        &host,
        "ask",
        &tool_request_id("s-rev", "c1"),
        payload,
        false,
    );
    let key = match ts::send(&fx, &ask, "ask") {
        CallResponse::Receipt { receipt, .. } => receipt.result["key"].as_str().unwrap().to_owned(),
        other => panic!("ask refused: {other:?}"),
    };
    crate::questions::escalate(&fx.ctx, &key, 1, Actor::Pm(None), None, None).unwrap();
    // Then the task waits in Needs me on that question,
    let held = fx.ctx.store.get(&id).unwrap();
    assert_eq!(held.status, Status::NeedsMe);
    assert!(corral_core::agent::question_hold(&held, &held.runs[0]));
    // and the reviewer is still live: its withdrawal commits and returns the task to Review.
    let withdraw = json!({"key": key, "reason": "found it in the spec"});
    let input = ts::envelope(
        &host,
        "withdraw",
        &tool_request_id("s-rev", "c2"),
        withdraw,
        false,
    );
    ts::committed(&ts::send(&fx, &input, "withdraw"));
    let settled = fx.ctx.store.get(&id).unwrap();
    assert_eq!(settled.status, Status::Review);
    assert_eq!(settled.runs[0].blocked_excerpt, None);
}
