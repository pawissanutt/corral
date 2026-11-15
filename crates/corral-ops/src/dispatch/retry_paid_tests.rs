//! Unit-level replacement journey, with native bindings only: no host/provider traffic.
use super::{paid, supersede};
use crate::{
    agent::{BindTarget, CallResponse, test_support::*},
    availability::{
        checkpoint,
        continuation::{self, ContextSource, IntentPhase, SubjectRevision},
        policy,
    },
};
use corral_core::{
    agent::{AgentRole, ContainerRef, HostHealth},
    connection::ConnectionId,
    paid_episode::PaidPhase,
    *,
};
use serde_json::json;
use std::{path::Path, process::Command};

fn git(path: &Path, args: &[&str]) -> String {
    let result = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    String::from_utf8(result.stdout).unwrap().trim().into()
}

struct Worker {
    fx: Fx,
    task: String,
    host: Host,
    decision: RouteDecision,
    done: Vec<u8>,
}

fn worker() -> Worker {
    let mut fx = fx();
    let candidate = |id: &str, connection: &str, cost| ExecutionCandidate {
        id: id.into(),
        harness: "corral-pi".into(),
        connection: connection.into(),
        provider: "openai".into(),
        model: "same-model".into(),
        effort: None,
        args: vec![],
        evidence: "synthetic".into(),
        cost,
    };
    let sub = candidate("sub", "openai-subscription", CandidateCost::Subscription);
    let api = candidate(
        "api",
        "openai-api",
        CandidateCost::Metered {
            currency: "USD".into(),
            maximum_micros: 1,
        },
    );
    let profile: Profile = serde_json::from_value(json!({
        "harness":"corral-pi", "vendor":"openai", "model":"same-model", "metadata": {
            "display_name":"fixture", "purpose":"fixture", "guidance":{}, "instructions":"fixture",
            "eligibility":{"enabled":true,"automatic":true,"roles":["implementation"],"projects":[]},
            "revision":"fixture", "evidence":"synthetic", "candidates":[sub,api],
            "fallback":{"automatic":true,"paid_roles":["worker"],"approved_providers":["openai"],"max_switches":1,
                "allow_unknown_subscription_cost":true,"spending":{"currency":"USD","maximum_micros":1},"approval":"synthetic"}
        }
    })).unwrap();
    fx.ctx.store.config.profiles.insert("pi".into(), profile);
    fx.ctx.store.save_config().unwrap();
    let binding = fx
        .ctx
        .store
        .config
        .resolve_candidate("pi", &sub)
        .unwrap()
        .connection_binding()
        .unwrap()
        .unwrap();
    assert_eq!(binding.connection, ConnectionId::OpenaiSubscription);
    let task = worker_task(&fx, "p1");
    let repo = fx._tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "base",
        ],
    );
    fx.ctx
        .store
        .update(&task, |task| {
            task.runs[0].worktree = repo.clone();
            task.runs[0].base_sha = git(&repo, &["rev-parse", "HEAD"]);
            task.runs[0].checkpoint_required = true;
            Ok(())
        })
        .unwrap();
    let source = ContainerRef::Worker {
        task: task.clone(),
        run: "R1".into(),
    };
    let host = bind(
        &fx,
        &BindTarget::Worker {
            task: task.clone(),
            run: "R1".into(),
        },
        "p1",
        "s1",
    );
    let hello = committed(&hello(&fx, &host));
    let payload = json!({"summary":"Previous changes round completed"});
    let request = crate::agent::tool_request_id("s1", "old-done");
    let done = envelope(&host, "done", &request, payload.clone(), false);
    continuation::with_state(&fx.ctx, &source, |state| {
        state.generations[0].connection = Some(binding.clone());
        // A prior done was committed, then owner changes reopened this same run. Its
        // receipted callback must stay replayable after the later quota transfer.
        let mut receipt = hello.clone();
        receipt.request_id = request.clone();
        receipt.op = "done".into();
        receipt.payload_sha256 = crate::agent::payload_sha256(&payload);
        receipt.result = json!({"status":"review","run":"R1"});
        state.receipts.push(receipt);
        let now = corral_core::clock::now();
        state.turns.push(serde_json::from_value(json!({
            "generation":1,"seq":1,"origin":{"kind":"bridge"},"started":now,"ended":now,"class":"provider_failed",
            "provider_failure":{"version":1,"connection":"openai-subscription","category":"subscription_quota","scope":"account"}
        }))?);
        Ok(())
    }).unwrap();
    fx.ctx.store.update(&task, |task| {
        let run = &task.runs[0];
        let checkpoint = serde_json::from_value(json!({"artifact":"synthetic-handoff","checkpoint":{
            "version":1,"revision":1,"captured":corral_core::clock::now(),"progress":null,"completed_steps":[],"current_step":null,
            "input":{"task":task.id,"run":"R1","spec_revision":crate::progress::progress_spec_revision(task)?,
                "expected_revision":0,"progress_revision":0,"head":git(&repo, &["rev-parse","HEAD"]),
                "base":run.base_sha,"tree":crate::progress::tree_revision(run)?,"criterion_ids":[],"evidence_refs":[],
                "next_action":"Continue from the committed handoff","decisions":[],"waits":[],"resources":[]}
        }}))?;
        task.runs[0].checkpoints.push(checkpoint);
        Ok(())
    }).unwrap();
    let subject = SubjectRevision {
        task: Some(task.clone()),
        spec_revision: Some(
            crate::progress::progress_spec_revision(&fx.ctx.store.get(&task).unwrap()).unwrap(),
        ),
        target: None,
    };
    policy::ensure_root(&fx.ctx, &source, "pi", &subject).unwrap();
    let decision = RouteDecision {
        profile: "pi".into(),
        harness: "corral-pi".into(),
        model: Some("same-model".into()),
        effort: None,
        route: RouteSource::Override,
        warnings: vec![],
        availability: Some(CandidateBinding {
            candidate: "api".into(),
            episode: None,
            reason: None,
            skipped: vec![],
            pinned: false,
        }),
    };
    Worker {
        fx,
        task,
        host,
        decision,
        done,
    }
}

fn transfer(w: &Worker) -> anyhow::Result<Option<continuation::ContinuationIntent>> {
    let head = w.fx.ctx.store.head(&w.task)?;
    let task = head.shell_task()?;
    let prev = task.current_run().unwrap();
    paid::require_checkpoint(&w.fx.ctx, &head, prev, true)?;
    paid::transfer(&w.fx.ctx, &head, prev, &w.decision)
}

#[test]
fn worker_paid_replacement_links_one_successor_pauses_stale_checkpoint_and_replays_done() {
    let w = worker();
    let intent = transfer(&w).unwrap().unwrap();
    assert_eq!(intent.phase, IntentPhase::Prepared);
    let source = ContainerRef::Worker {
        task: w.task.clone(),
        run: "R1".into(),
    };
    let successor = ContainerRef::Worker {
        task: w.task.clone(),
        run: "R2".into(),
    };
    assert_eq!(intent.source.container, source);
    assert_eq!(intent.successor.container, successor);
    assert_ne!(intent.source.container, intent.successor.container);
    assert_eq!(state(&w.fx, &source).binding.health, HostHealth::Fenced);
    let ContextSource::Checkpoint { digest } = &intent.context else {
        panic!("worker must use its committed checkpoint");
    };
    let (_, context) =
        checkpoint::load(&w.fx.ctx.store, digest, &intent.source, &intent.subject).unwrap();
    assert_eq!(
        context,
        serde_json::to_vec(&w.fx.ctx.store.get(&w.task).unwrap().runs[0].checkpoints[0].checkpoint)
            .unwrap()
    );
    assert!(
        matches!(w.fx.ctx.store.paid_episode(&intent.link.episode).unwrap().phase, PaidPhase::Prepared { ref successor, .. } if successor == &intent.successor)
    );
    let plan: RetryPlan =
        serde_json::from_value(json!({"profile":"pi","model":"same-model"})).unwrap();
    // Exercise retry_with's head mutation and the ordinary run-allocation primitive;
    // executing dispatch itself would require a host installation and pane engine.
    w.fx.ctx
        .store
        .dispatch_update_head(&w.task, |_, task| {
            supersede(task, &plan);
            task.start_run(run("R2", "p2"))?;
            Ok(())
        })
        .unwrap();
    let task = w.fx.ctx.store.get(&w.task).unwrap();
    assert_eq!(task.runs[0].outcome, Some(RunOutcome::Superseded));
    assert_eq!(task.current_run().unwrap().id, "R2");
    let intents = continuation::INTENTS.list(&w.fx.ctx.store).unwrap();
    assert_eq!(
        intents
            .iter()
            .filter(|(_, i)| i.phase == IntentPhase::Prepared)
            .map(|(_, i)| &i.successor.container)
            .collect::<Vec<_>>(),
        vec![&successor]
    );
    continuation::require_launch(&w.fx.ctx.store, &successor, &intent.successor.binding).unwrap();
    assert!(
        continuation::require_launch(&w.fx.ctx.store, &source, &intent.successor.binding).is_err()
    );
    let before = task_bytes(&w.fx, &w.task);
    assert!(matches!(
        send(&w.fx, &w.done, "done"),
        CallResponse::Receipt { replayed: true, .. }
    ));
    assert_eq!(task_bytes(&w.fx, &w.task), before);
    let fresh_done = envelope(
        &w.host,
        "done",
        &crate::agent::tool_request_id("s1", "late-done"),
        json!({"summary":"Must not finish R2"}),
        false,
    );
    assert_eq!(code(&send(&w.fx, &fresh_done, "done")), Some("run_ended"));
    assert_eq!(task_bytes(&w.fx, &w.task), before);

    // Dispatch can fail before allocating a successor run or native host state.
    let unbound = worker();
    let intent = transfer(&unbound).unwrap().unwrap();
    let before = task_bytes(&unbound.fx, &unbound.task);
    assert!(
        corral_core::agent::container_state(
            &unbound.fx.ctx.store.get(&unbound.task).unwrap(),
            &intent.successor.container,
        )
        .is_none()
    );
    let error = paid::launched(
        &unbound.fx.ctx,
        Some(&intent),
        Err(anyhow::anyhow!("successor launch failed")),
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("successor launch failed"));
    let paused = continuation::task_intent(&unbound.fx.ctx.store, &unbound.task, AgentRole::Worker)
        .unwrap()
        .unwrap();
    assert_eq!(paused.phase, IntentPhase::Paused);
    assert_eq!(
        paused.pause,
        Some(continuation::PauseReason::SuccessorAmbiguous)
    );
    assert_eq!(task_bytes(&unbound.fx, &unbound.task), before);
    // A later retry must finish the task hold even though no successor was allocated.
    let mut expected = unbound.fx.ctx.store.get(&unbound.task).unwrap();
    let before = unbound.fx.ctx.store.tasks_stamp().unwrap();
    let error = transfer(&unbound).unwrap_err();
    assert!(format!("{error:#}").contains("SuccessorAmbiguous"));
    let held = unbound.fx.ctx.store.get(&unbound.task).unwrap();
    expected.status = Status::NeedsMe;
    expected.updated = held.updated;
    expected.status_since = Some(held.updated);
    assert_eq!(held, expected);
    let after = unbound.fx.ctx.store.tasks_stamp().unwrap();
    assert_ne!(after, before);
    let error = transfer(&unbound).unwrap_err();
    assert!(format!("{error:#}").contains("SuccessorAmbiguous"));
    assert_eq!(unbound.fx.ctx.store.tasks_stamp().unwrap(), after);
    // A stale retry must not transfer its hold to an unrelated replacement run.
    unbound
        .fx
        .ctx
        .store
        .update(&unbound.task, |task| {
            task.runs[0].outcome = Some(RunOutcome::Superseded);
            task.runs.push(run("R3", "p3"));
            task.status = Status::Running;
            Ok(())
        })
        .unwrap();
    let before = task_bytes(&unbound.fx, &unbound.task);
    let version = unbound.fx.ctx.store.tasks_stamp().unwrap();
    assert!(continuation::failed_start(&unbound.fx.ctx, &intent.successor.container, 1).unwrap());
    assert_eq!(task_bytes(&unbound.fx, &unbound.task), before);
    assert_eq!(unbound.fx.ctx.store.tasks_stamp().unwrap(), version);

    for change_spec in [false, true] {
        let stale = worker();
        if change_spec {
            stale
                .fx
                .ctx
                .store
                .update(&stale.task, |task| {
                    task.body.push_str("\nChanged spec");
                    Ok(())
                })
                .unwrap();
        } else {
            let task = stale.fx.ctx.store.get(&stale.task).unwrap();
            std::fs::write(
                task.runs[0].worktree.join("new-work"),
                "uncheckpointed work",
            )
            .unwrap();
        }
        let before = task_bytes(&stale.fx, &stale.task);
        assert!(transfer(&stale).is_err());
        let task = stale.fx.ctx.store.get(&stale.task).unwrap();
        assert_eq!(task.current_run().unwrap().id, "R1");
        assert_eq!(task.runs[0].outcome, None);
        assert_eq!(task_bytes(&stale.fx, &stale.task), before);
        assert_eq!(events(&stale.fx, "continuation_paused"), 1);
        assert!(
            continuation::INTENTS
                .list(&stale.fx.ctx.store)
                .unwrap()
                .is_empty()
        );
    }
}
