use super::*;
use corral_core::{Actor, ExecutionSnapshot, Profile, Run, Status};
use serde_json::json;
use std::process::Command;

struct Clock;
impl Drop for Clock {
    fn drop(&mut self) {
        corral_core::clock::pin_thread(None);
    }
}
fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Actual PM author")
        .env("GIT_AUTHOR_EMAIL", "pm@fixture.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture committer")
        .env("GIT_COMMITTER_EMAIL", "committer@fixture.invalid")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}
fn fixture() -> (tempfile::TempDir, Ctx, Request) {
    corral_core::clock::pin_thread(Some("2026-11-01T00:00:00Z".parse().unwrap()));
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("app");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    let mut store = Store::init(root.path()).unwrap();
    store.config.projects.insert(
        "app".into(),
        serde_json::from_value(json!({"path":"app"})).unwrap(),
    );
    store.config.profiles.insert(
        "general".into(),
        serde_json::from_value::<Profile>(json!({"harness":"generic"})).unwrap(),
    );
    store.save_config().unwrap();
    let task = store
        .create(
            "Historical task",
            Some("app"),
            Status::Review,
            "exact scope",
        )
        .unwrap();
    let _actor = store.attribute(Actor::Pm);
    for (index, profile) in ["initial-profile", "actual-pm-profile", "last-profile"]
        .into_iter()
        .enumerate()
    {
        if index > 0 {
            store.log_event(Some(&task.id),"autopilot",json!({"run":format!("R{index}"),"to":"changes","round":index,
                "max_rounds":2,"effective_max_rounds":2,"round_limit_source":"task_review_policy.max_rounds",
                "sha":git(&repo,&["rev-parse","HEAD"]),"reason":format!("required correction {index}")})).unwrap();
        }
        git(
            &repo,
            &[
                "commit",
                "--allow-empty",
                "-qm",
                &format!("candidate {index}"),
            ],
        );
        let sha = git(&repo, &["rev-parse", "HEAD"]);
        let id = format!("R{}", index + 1);
        store.update(&task.id,|t| {
            t.runs.push(serde_json::from_value::<Run>(json!({"id":id,"agent":profile,"worktree":repo,"branch":"fixture","base":"main","base_sha":sha,
                "started":"2026-11-01T00:00:00Z","ended":"2026-11-01T00:00:00Z","outcome":"done"}))?);
            t.rounds = index as u32;
            if index == 2 { t.current_run_mut().unwrap().gate = Some(serde_json::from_value(json!({"sha":sha,"at":"2026-11-01T00:00:00Z","result":"fail","failed_cmd":"unit:atomicity"}))?); }
            Ok(())
        }).unwrap();
        store.log_event(Some(&task.id),"cowork_finished",json!({"run":id,"head":sha,"session":"A-fixture","slug":"work","owner_confirmed":true})).unwrap();
    }
    let head = store.head(&task.id).unwrap();
    let request = Request {
        version: 1,
        id: "fixture-import".into(),
        task: task.id.clone(),
        expected_revision: head.rev,
        expected_run: "R3".into(),
        expected_contract: head.contract_revision().unwrap(),
    };
    drop(_actor);
    (root, Ctx::from_store(store), request)
}

#[test]
fn canonical_import_is_record_only_then_one_approved_continuation_cannot_be_reused() {
    let _clock = Clock;
    let (_root, ctx, request) = fixture();
    let before = ctx.store.get(&request.task).unwrap();
    let proposal = preview(&ctx, &request).unwrap();
    assert_eq!(ctx.store.get(&request.task).unwrap(), before);
    assert!(ctx.store.repair_ledger(&request.task).unwrap().is_none());
    assert_eq!(proposal.commits.len(), 3);
    assert!(
        proposal
            .commits
            .iter()
            .all(|c| c.author == "Actual PM author <pm@fixture.invalid>")
    );
    assert!(
        proposal
            .entries
            .iter()
            .filter(|e| e.classification == "submission")
            .all(|e| e.actor.as_deref() == Some("pm"))
    );
    assert_eq!(
        proposal
            .entries
            .iter()
            .filter_map(|e| e.accounting.as_ref())
            .filter(|e| matches!(e, corral_core::RepairEvent::Started { .. }))
            .count(),
        3
    );
    let receipt = apply_checked(&ctx, authority::fixture(proposal.clone())).unwrap();
    assert_eq!(
        apply_checked(&ctx, authority::fixture(proposal.clone())).unwrap(),
        receipt
    );
    assert_eq!(ctx.store.get(&request.task).unwrap().runs, before.runs);
    let imported = ctx.store.repair_ledger(&request.task).unwrap().unwrap();
    assert_eq!(
        (
            imported.task_counters().unwrap().initial_implementations,
            imported.task_counters().unwrap().coding_repairs
        ),
        (1, 2)
    );
    assert!(matches!(
        imported.transition().unwrap(),
        corral_core::RepairTransition::Owner { .. }
    ));
    assert_eq!(
        imported.active().unwrap().history.current_profile(),
        "initial-profile"
    );
    let profiles = imported
        .active()
        .unwrap()
        .history
        .events()
        .iter()
        .filter_map(|e| match e {
            corral_core::RepairEvent::Started { receipt } => Some(receipt.profile.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        profiles,
        vec!["initial-profile", "actual-pm-profile", "last-profile"]
    );
    let mut changed = proposal;
    changed.request.id = "different-request".into();
    assert!(apply_checked(&ctx, authority::fixture(changed.clone())).is_err());
    assert!(confirm(&ctx, &changed, "PM prose is not owner input").is_err());
    ctx.store
        .update_head(&request.task, |t| {
            t.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    let renewal = ContinuationRequest {
        version: 1,
        id: "one-owner-decision".into(),
        task: request.task.clone(),
        previous_digest: imported.digest().unwrap(),
        import_lineage: receipt.lineage.clone(),
        profile: "general".into(),
    };
    let approved = preview_continuation(&ctx, &renewal).unwrap();
    let renewed =
        continuation::renew_fixture(&ctx, continuation::fixture(approved.clone())).unwrap();
    assert_eq!(renewed.allowances[0], imported.allowances[0]);
    assert_eq!(
        renewed
            .active()
            .unwrap()
            .history
            .counters()
            .unwrap()
            .initial_implementations,
        0
    );
    assert_eq!(
        continuation::renew_fixture(&ctx, continuation::fixture(approved.clone())).unwrap(),
        renewed
    );
    let mut replay = approved;
    replay.request.id = "same-approval-new-request".into();
    assert!(continuation::renew_fixture(&ctx, continuation::fixture(replay)).is_err());
    let execution = ExecutionSnapshot::new(
        &ctx.store.config,
        ctx.store
            .config
            .resolve_profile(
                &corral_core::ProfileChoice {
                    profile: Some("general".into()),
                    ..Default::default()
                },
                None,
                None,
            )
            .unwrap(),
    );
    // Even with an exact route, recovery of an archived attempt cannot disguise the new cycle.
    let attempt = |availability| {
        ctx.store.update(&request.task, |task| {
            let previous = task.current_run().unwrap().id.clone();
            crate::repair_runtime::prepare_continuation(&ctx.store, task, &previous, availability)?;
            let mut run = task.current_run().unwrap().clone();
            run.id = format!("R{}", task.runs.len() + 1);
            run.agent = "general".into();
            run.model = execution.resolved.model.clone();
            run.effort = execution.resolved.effort.clone();
            run.harness = Some(execution.resolved.harness.clone());
            run.execution = Some(execution.clone());
            task.runs.push(run);
            crate::repair_runtime::start_launch(&ctx.store, task)
        })
    };
    assert!(attempt(true).is_err());
    attempt(false).unwrap();
    let charged = ctx.store.repair_ledger(&request.task).unwrap().unwrap();
    assert_eq!(charged.task_counters().unwrap().coding_repairs, 3);
    assert!(attempt(false).is_err());
    assert_eq!(
        ctx.store.repair_ledger(&request.task).unwrap().unwrap(),
        charged
    );
    assert_eq!(charged.task_counters().unwrap().initial_implementations, 1);
    assert_eq!(
        ctx.store
            .record::<Receipt>("repair-import-consumed", &request.task)
            .unwrap()
            .unwrap(),
        receipt
    );
}

#[test]
fn incomplete_contradictory_future_or_stale_sources_refuse_without_publication() {
    let _clock = Clock;
    let (_root, ctx, request) = fixture();
    let proposal = preview(&ctx, &request).unwrap();
    for change in [
        "missing-issue",
        "bad-cap",
        "future",
        "tampered",
        "missing-submission",
        "missing-author",
    ] {
        let mut changed = proposal.sources.clone();
        match change {
            "missing-issue" => changed
                .events
                .retain(|e| !(e.event.kind == "autopilot" && e.event.detail["round"] == 1)),
            "bad-cap" => {
                changed
                    .events
                    .iter_mut()
                    .find(|e| e.event.kind == "autopilot")
                    .unwrap()
                    .event
                    .detail["max_rounds"] = json!(1)
            }
            "future" => changed.events[0].event.detail["version"] = json!(999),
            "tampered" => {
                changed
                    .events
                    .iter_mut()
                    .find(|e| e.event.kind == "cowork_finished")
                    .unwrap()
                    .event
                    .detail["head"] = json!("f".repeat(40))
            }
            "missing-author" => {
                changed
                    .events
                    .iter_mut()
                    .find(|e| e.event.kind == "cowork_finished")
                    .unwrap()
                    .event
                    .actor = None
            }
            _ => changed.events.pop().map(|_| ()).unwrap(),
        }
        changed.end.event_count = changed.events.len();
        let result = chronology::check_existing(&changed).and_then(|_| {
            chronology::reconstruct(
                &ctx.store,
                &ctx.store.head(&request.task).unwrap(),
                &changed,
            )
        });
        assert!(result.is_err(), "accepted {change}");
    }
    ctx.store
        .update_head(&request.task, |t| {
            t.title.push_str(" changed");
            Ok(())
        })
        .unwrap();
    assert!(apply_checked(&ctx, authority::fixture(proposal)).is_err());
    assert!(ctx.store.repair_ledger(&request.task).unwrap().is_none());
    assert!(
        ctx.store
            .record_row("repair-import", &request.task)
            .unwrap()
            .is_none()
    );
    assert!(
        ctx.store
            .record_row("repair-import-consumed", &request.task)
            .unwrap()
            .is_none()
    );
    // The old fail-closed placeholder is retained verbatim, not dropped as zero spent.
    let mut placeholder = RepairHistory::new(
        RepairPolicy::approved_starter_policy(),
        "last-profile",
        None,
    )
    .unwrap();
    placeholder
        .apply(corral_core::RepairEvent::Started {
            receipt: corral_core::RepairStartReceipt {
                attempt_id: corral_core::RepairAttemptId("initial:R3".into()),
                profile: "last-profile".into(),
                kind: corral_core::RepairAttemptKind::InitialImplementation,
                findings: Vec::new(),
            },
        })
        .unwrap();
    placeholder
        .apply(corral_core::RepairEvent::CodingBlocked {
            reason: corral_core::LEGACY_REPAIR_BLOCKER.into(),
        })
        .unwrap();
    ctx.store
        .save_repair_ledger(&request.task, &RepairLedger::new(placeholder))
        .unwrap();
    let original = ctx
        .store
        .record_row(corral_core::REPAIR_RUNTIME, &request.task)
        .unwrap()
        .unwrap();
    let head = ctx.store.head(&request.task).unwrap();
    let request = Request {
        expected_revision: head.rev,
        expected_contract: head.contract_revision().unwrap(),
        ..request
    };
    let preview = preview(&ctx, &request).unwrap();
    let receipt = apply_checked(&ctx, authority::fixture(preview)).unwrap();
    let proof: serde_json::Value =
        serde_json::from_slice(&ctx.store.blob_bytes(&receipt.evidence).unwrap().unwrap()).unwrap();
    assert_eq!(
        proof["preview"]["sources"]["records"][corral_core::REPAIR_RUNTIME]["doc"],
        original.doc
    );
    assert!(
        chronology::check_existing(&ctx.store.repair_sources(&request.task).unwrap()).is_err(),
        "substantive history cannot be overwritten"
    );
}
