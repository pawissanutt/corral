#![cfg(unix)] // Planner fixtures execute plan_engine.sh directly.

use super::*;
use corral_core::{AgentCmd, PlanRun, Profile, Store};

const PLAN: &str = "---\ngoal: Ship widget\nnon_goals: []\ndecisions: []\naffected_paths: [widget.rs]\ncontracts: []\ndependencies: []\nacceptance: [{scenario: spin, expected: clockwise}]\nverification: [check]\nowner_decisions: []\n---\n";

#[path = "plan_ownership_tests.rs"]
mod ownership;

#[path = "plan_lifecycle_tests.rs"]
mod lifecycle;
#[path = "plan_routing_tests.rs"]
mod routing;

fn fixture() -> (tempfile::TempDir, Ctx, String, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("app")).unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.projects.insert(
        "app".into(),
        corral_core::Project {
            dispatch: Default::default(),
            path: "app".into(),
            default_profile: None,
            base: None,
            review: Default::default(),
            artifact_retention: None,
            post_land: Vec::new(),
        },
    );
    store.config.agents.insert(
        "omo".into(),
        AgentCmd {
            command: "true {model_args}".into(),
        },
    );
    store.config.harnesses.get_mut("omo").unwrap().command = "true {model_args}".into();
    for (name, model) in [
        ("gpt-sol", "fixture-planner"),
        ("gpt-review", "fixture-reviewer"),
    ] {
        store.config.profiles.insert(
            name.into(),
            Profile {
                harness: "omo".into(),
                model: Some(model.into()),
                vendor: Some("openai".into()),
                effort: None,
                escalate_to: None,
                args: Vec::new(),
                metadata: None,
            },
        );
    }
    store.config.review.reviewer = Some("gpt-review".into());
    store.config.review.max_rounds = Some(1);
    store.save_config().unwrap();
    let task = store
        .create(
            "Design widget",
            Some("app"),
            Status::Running,
            "Widget design",
        )
        .unwrap();
    let output = store.state_dir().join("plan-output");
    std::fs::create_dir_all(&output).unwrap();
    let path = output.join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    store
        .update(&task.id, |t| {
            t.plan_required = Some(true);
            t.plan_runs.push(PlanRun {
                id: "D1".into(),
                profile: "gpt-sol".into(),
                harness: "omo".into(),
                model: Some("fixture-planner".into()),
                vendor: Some("openai".into()),
                effort: None,
                spec_revision: crate::plan_spec_revision(t)?,
                started: Timestamp::now(),
                ended: None,
                herdr: None,
                output_dir: output,
                submitted: None,
                rounds: 0,
                state: PlanRunState::Planning,
                pending_reply: None,
            });
            Ok(())
        })
        .unwrap();
    let engine = tmp.path().join("engine");
    std::fs::create_dir(&engine).unwrap();
    let bin = engine.join("herdr");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/plan_engine.sh"),
        &bin,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut ctx = Ctx {
        store,
        herdr: corral_herdr::Herdr::with_bin(bin, Some("corral-test".into())),
        resource_probe: crate::resources::FixedResourceProbe::arc(),
    };
    crate::plan_submit(&ctx, &task.id, &path).unwrap();
    grant(&ctx.store, &task.id, false);
    ctx.store.reload_config_if_changed().unwrap();
    (tmp, ctx, task.id, path)
}

fn grant(store: &Store, id: &str, plan_accept: bool) {
    let preview = store
        .preview_operating_policy(&corral_core::OperatingSelection {
            tasks: std::collections::BTreeSet::from([id.into()]),
            authority: corral_core::OperatingAuthority {
                dispatch: true,
                plan_accept,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    store
        .confirm_operating_policy(&preview, "owner", "2026-10-18T08:00:00Z".parse().unwrap())
        .unwrap();
}

fn review(ctx: &Ctx, id: &str, verdict: Verdict) {
    advance(ctx, &ctx.store.get(id).unwrap(), Timestamp::now()).unwrap();
    let task = ctx.store.get(id).unwrap();
    let plan = task.plans.last().unwrap();
    let attempt = plan.reviews.last().unwrap();
    assert_ne!(attempt.reviewer, task.plan_runs[0].profile);
    crate::plan_verdict(
        ctx,
        id,
        &crate::PlanVerdict {
            plan: &plan.id,
            attempt: Some(&attempt.id),
            verdict,
            notes: "Review notes",
        },
    )
    .unwrap();
}

#[test]
fn plan_changes_relaunch_planner_and_stop_at_max_rounds() {
    // Given a submitted plan reviewed by a different model.
    let (_tmp, ctx, id, path) = fixture();
    review(&ctx, &id, Verdict::Changes);
    // When autopilot returns changes and then receives another rejected revision.
    advance(&ctx, &ctx.store.get(&id).unwrap(), Timestamp::now()).unwrap();
    let revised = ctx.store.get(&id).unwrap();
    assert_eq!(revised.plan_runs[0].rounds, 1);
    assert_eq!(revised.plan_runs[0].state, PlanRunState::Changes);
    assert_eq!(revised.status, Status::Running);
    crate::plan_submit(&ctx, &id, &path).unwrap();
    review(&ctx, &id, Verdict::Changes);
    advance(&ctx, &ctx.store.get(&id).unwrap(), Timestamp::now()).unwrap();
    // Then bounded retries escalate without a code candidate or code gate.
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.plan_runs[0].rounds, 1);
    assert_eq!(task.plan_runs[0].state, PlanRunState::Escalated);
    assert_eq!(task.status, Status::NeedsMe);
    assert!(task.runs.is_empty());
    assert!(task.launch.is_none());
    assert_eq!(task.plans.len(), 2);
    assert_eq!(crate::slot_usage(&ctx).unwrap().total(), 0);
}

#[test]
fn review_approval_waits_for_owner_without_holding_worker_slot() {
    // Given a planning session and independent reviewer.
    let (_tmp, ctx, id, _path) = fixture();
    // When the reviewer approves and autopilot reconciles.
    review(&ctx, &id, Verdict::Approve);
    advance(&ctx, &ctx.store.get(&id).unwrap(), Timestamp::now()).unwrap();
    // Then owner attention is visible, but no acceptance or worker slot exists.
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(task.plan_runs[0].state, PlanRunState::AwaitingOwner);
    assert!(task.plans[0].approval.is_none());
    assert!(task.runs.is_empty());
    assert_eq!(crate::slot_usage(&ctx).unwrap().total(), 0);
}

#[test]
fn planner_pane_loss_records_failure_and_releases_admission() {
    // Given a planner in its first active round.
    let (_tmp, ctx, id, _path) = fixture();
    ctx.store
        .update(&id, |t| {
            let run = t.plan_runs.last_mut().unwrap();
            run.state = PlanRunState::Planning;
            run.ended = None;
            t.status = Status::Running;
            Ok(())
        })
        .unwrap();
    let task = ctx.store.get(&id).unwrap();
    // When a valid inventory still has no pane after startup grace.
    let now = task.plan_runs[0].started + jiff::SignedDuration::from_secs(60);
    reconcile(&ctx, &task, &Default::default(), now).unwrap();
    // Then it is a failed plan session, not a phantom implementation run.
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(task.plan_runs[0].state, PlanRunState::Failed);
    assert_eq!(task.plan_runs[0].ended, Some(now));
    assert_eq!(crate::slot_usage(&ctx).unwrap().total(), 0);
}

#[test]
fn reviewer_loss_and_owner_supersession_release_one_slot_admission() {
    let (_tmp, mut ctx, id, _path) = fixture();
    ctx.store.config.limits.running = 1;
    ctx.store.config.limits.review = 1;
    ctx.store.config.limits.count_reviews = true;
    ctx.store.save_config().unwrap();
    advance(&ctx, &ctx.store.get(&id).unwrap(), Timestamp::now()).unwrap();
    let reviewing = ctx.store.get(&id).unwrap();
    let attempt = reviewing.plans[0].reviews[0].clone();
    assert_eq!(crate::slot_usage(&ctx).unwrap().total(), 1);

    // The fake engine reports no pane. Advance the clock, not a sleep or poll.
    let now = attempt.at
        + jiff::SignedDuration::from_secs(
            i64::try_from(ctx.store.config.sync.startup_grace_secs).unwrap() + 1,
        );
    advance(&ctx, &reviewing, now).unwrap();
    let lost = ctx.store.get(&id).unwrap();
    assert_eq!(lost.plan_runs[0].state, PlanRunState::Escalated);
    assert_eq!(lost.plans[0].reviews[0].verdict, Some(Verdict::Escalate));
    assert!(lost.plans[0].reviews[0].pending_reply.is_none());
    assert_eq!(crate::slot_usage(&ctx).unwrap().total(), 0);
    assert!(
        crate::plan_verdict(
            &ctx,
            &id,
            &crate::PlanVerdict {
                plan: "P1",
                attempt: Some(&attempt.id),
                verdict: Verdict::Approve,
                notes: "Late callback",
            }
        )
        .is_err()
    );

    let retry = crate::plan_review(&ctx, &id, None).unwrap();
    assert_ne!(retry.id, attempt.id);
    escalate(&ctx, &lost, "Stale reviewer-loss observation").unwrap();
    assert!(
        ctx.store.get(&id).unwrap().plans[0].reviews[1]
            .verdict
            .is_none()
    );
    assert_eq!(crate::slot_usage(&ctx).unwrap().total(), 1);
    crate::plan_verdict(
        &ctx,
        &id,
        &crate::PlanVerdict {
            plan: "P1",
            attempt: None,
            verdict: Verdict::Changes,
            notes: "Revise the contract",
        },
    )
    .unwrap();
    let changes = ctx.store.get(&id).unwrap();
    assert_eq!(changes.plans[0].reviews[1].verdict, Some(Verdict::Escalate));
    assert!(changes.plans[0].reviews[1].pending_reply.is_none());
    assert_eq!(crate::slot_usage(&ctx).unwrap().total(), 0);
    assert!(
        crate::plan_verdict(
            &ctx,
            &id,
            &crate::PlanVerdict {
                plan: "P1",
                attempt: Some(&retry.id),
                verdict: Verdict::Approve,
                notes: "Superseded callback",
            }
        )
        .is_err()
    );
    advance(&ctx, &changes, Timestamp::now()).unwrap();
    let revised = ctx.store.get(&id).unwrap();
    assert_eq!(revised.status, Status::Running);
    assert_eq!(revised.plan_runs[0].rounds, 1);
    assert!(revised.plan_runs[0].ended.is_none());
    assert_eq!(crate::slot_usage(&ctx).unwrap().tasks, 1);
    assert_eq!(crate::slot_usage(&ctx).unwrap().reviews, 0);
}

/// The fake engine reports its real prepared pane metadata before IPC. The
/// datagram exchange blocks that report, not pane_run, with bounded receives.
fn metadata_barrier(tmp: &std::path::Path) -> std::os::unix::net::UnixDatagram {
    let engine = tmp.join("engine");
    std::fs::copy(engine.join("herdr"), engine.join("base")).unwrap();
    let socket = engine.join("barrier.sock");
    let listener = std::os::unix::net::UnixDatagram::bind(&socket).unwrap();
    listener
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    let wrapper = r#"#!/usr/bin/env bash
set -eu
dir="$(dirname "$0")"
if [ "${1:-}" = "--session" ]; then shift 2; fi
if [ "$1 $2" = "pane report-metadata" ]; then
  python3 - "$dir/barrier.sock" <<'PY'
import socket, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
s.settimeout(10)
s.bind(sys.argv[1] + ".client")
s.sendto(b"prepared", sys.argv[1])
assert s.recv(16) == b"release"
PY
  printf '{"result":{}}\n'
else
  exec "$dir/base" "$@"
fi
"#;
    std::fs::write(engine.join("herdr"), wrapper).unwrap();
    listener
}

fn revoke_at_metadata(store: &Store, barrier: &std::os::unix::net::UnixDatagram) {
    let mut bytes = [0; 32];
    let (_, peer) = barrier.recv_from(&mut bytes).unwrap();
    Store::open(store.root())
        .unwrap()
        .pause_operating_policy(true)
        .unwrap();
    barrier
        .send_to(b"release", peer.as_pathname().unwrap())
        .unwrap();
}

#[test]
fn automatic_initial_planner_refusal_retains_preparation_without_a_process_slot() {
    // Given a finite grant and a real metadata-before-submit barrier.
    let (tmp, ctx, _, _) = fixture();
    let task = ctx
        .store
        .create(
            "Another design",
            Some("app"),
            Status::Ready,
            "Choose layout",
        )
        .unwrap();
    grant(&ctx.store, &task.id, false);
    let barrier = metadata_barrier(tmp.path());
    let launch_ctx = Ctx {
        store: Store::open(ctx.store.root()).unwrap(),
        ..ctx.clone()
    };
    let id = task.id.clone();
    let launch = std::thread::spawn(move || {
        crate::plan_dispatch::dispatch(
            &launch_ctx,
            &launch_ctx.store.get(&id).unwrap(),
            &crate::DispatchOpts {
                plan: true,
                ..Default::default()
            },
            &crate::dispatch::Admission::Automatic {
                inputs: serde_json::json!({"source":"backlog"}),
                at: "2026-10-18T08:00:00Z".parse().unwrap(),
            },
        )
    });
    // When another context commits revocation before releasing preparation.
    revoke_at_metadata(&ctx.store, &barrier);
    let error = launch.join().unwrap().unwrap_err();
    // Then the exact claim survives, with zero command submissions or fake failure.
    assert!(crate::operating::refused(&error), "{error:#}");
    let task = ctx.store.get(&task.id).unwrap();
    let run = &task.plan_runs[0];
    assert_eq!(run.state, PlanRunState::Planning);
    assert!(!run.holds_slot());
    assert_eq!(task.plan_required, None);
    assert!(run.herdr.is_some());
    assert!(run.output_dir.join("prompt.md").exists());
    assert!(run.output_dir.join("launch.json").exists());
    assert!(!tmp.path().join("engine/launch-command").exists());
    let mut action = crate::operating::identity(
        &task.id,
        "D1",
        corral_core::OperatingActionKind::Plan,
        "launch",
    );
    action.round = Some(0);
    let auth = crate::operating::load_start(&ctx, &action)
        .unwrap()
        .unwrap();
    assert!(crate::operating::is_automatic(&auth));
    assert!(matches!(
        ctx.store
            .authorized_submission(&auth.id)
            .unwrap()
            .disposition,
        corral_core::SubmissionDisposition::AuthorizationRefused { .. }
    ));
    assert_eq!(crate::slot_usage(&ctx).unwrap().total(), 0);
}

#[test]
fn automatic_revision_refusal_retains_notes_without_consuming_its_round() {
    // Given a real changes verdict and a separate automatic revision action.
    let (tmp, ctx, id, _) = fixture();
    review(&ctx, &id, Verdict::Changes);
    std::fs::remove_file(tmp.path().join("engine/launch-command")).unwrap();
    let barrier = metadata_barrier(tmp.path());
    let launch_ctx = ctx.clone();
    let launch_id = id.clone();
    let launch = std::thread::spawn(move || {
        crate::plan_dispatch::revise(
            &launch_ctx,
            &launch_ctx.store.get(&launch_id).unwrap(),
            "Review notes",
        )
    });
    // When revocation wins the final submission boundary.
    revoke_at_metadata(&ctx.store, &barrier);
    let error = launch.join().unwrap().unwrap_err();
    // Then the pending revision is not a failed process or a consumed repair round.
    assert!(crate::operating::refused(&error), "{error:#}");
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.plan_runs[0].rounds, 0);
    assert_eq!(
        task.plan_runs[0].pending_reply.as_deref(),
        Some("Review notes")
    );
    assert!(!task.plan_runs[0].holds_slot());
    assert!(!tmp.path().join("engine/launch-command").exists());
    let before = task.plan_runs.clone();
    advance(&ctx, &task, "2026-10-18T08:10:00Z".parse().unwrap()).unwrap();
    assert_eq!(ctx.store.get(&id).unwrap().plan_runs, before);
}

#[test]
fn automatic_plan_reviewer_refusal_keeps_exact_suspended_attempt() {
    // Given a prepared independent review, not merely a helper-only fixture.
    let (tmp, ctx, id, _) = fixture();
    let barrier = metadata_barrier(tmp.path());
    let launch_ctx = ctx.clone();
    let launch_id = id.clone();
    let launch =
        std::thread::spawn(move || crate::plan_review::review_auto(&launch_ctx, &launch_id));
    // When pause is acknowledged before the metadata barrier is released.
    revoke_at_metadata(&ctx.store, &barrier);
    let error = launch.join().unwrap().unwrap_err();
    // Then the pending PR1 is durable but does not pretend to hold a running slot.
    assert!(crate::operating::refused(&error), "{error:#}");
    let task = ctx.store.get(&id).unwrap();
    assert!(task.plans[0].reviews.is_empty());
    assert_eq!(task.plan_runs[0].state, PlanRunState::Reviewing);
    assert_eq!(crate::slot_usage(&ctx).unwrap().total(), 0);
    let prepared: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            ctx.store
                .state_dir()
                .join(format!("plan-review-{id}-P1.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(prepared["review"]["id"], "PR1");
    assert!(prepared["review"]["verdict"].is_null());
    assert!(prepared["review"]["herdr"].is_object());
    assert!(prepared["review"]["pending_reply"].is_string());
    assert!(!tmp.path().join("engine/launch-command").exists());
    assert!(crate::plan_review(&ctx, &id, None).is_err());
    assert!(!tmp.path().join("engine/launch-command").exists());
}

#[test]
fn prepared_planner_without_provenance_never_gains_owner_authority() {
    // Given an interrupted legacy preparation with no companion authorization.
    let (tmp, ctx, id, _) = fixture();
    let task = ctx.store.get(&id).unwrap();
    std::fs::write(
        task.plan_runs[0].output_dir.join("launch.json"),
        serde_json::to_vec(&serde_json::json!({
            "run":"D1", "attempt":"launch", "round":0, "previous_round":0,
            "plan":null, "notes":null, "force_resources":false, "command":null, "herdr":null,
        }))
        .unwrap(),
    )
    .unwrap();
    // When recovery reaches its original exact authorization reference.
    let result = crate::plan_dispatch::resume_prepared(&ctx, &task);
    // Then it refuses without a manual fallback or a new command.
    assert!(result.is_err());
    assert!(!tmp.path().join("engine/launch-command").exists());
    assert_eq!(ctx.store.get(&id).unwrap().plan_runs, task.plan_runs);
}
