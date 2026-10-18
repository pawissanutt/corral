use super::*;
use corral_core::{HerdrRef, PlanApproval, PlanReview, PlanRun, PlanRunState, Store, Verdict};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const PLAN: &str = "---\ngoal: Ship widget\nnon_goals: []\ndecisions: []\naffected_paths: [widget.rs]\ncontracts: []\ndependencies: []\nacceptance: [{scenario: spin, expected: clockwise}]\nverification: [check]\nowner_decisions: []\n---\n";

// Every IPC call goes to this absolute fake executable. No real herdr is invoked.
const ENGINE: &str = r#"#!/bin/sh
set -eu
dir=$(dirname "$0")
export HOME="$dir" XDG_CONFIG_HOME="$dir" XDG_DATA_HOME="$dir" XDG_STATE_HOME="$dir" XDG_CACHE_HOME="$dir"
unset HERDR_SOCKET_PATH HERDR_SESSION HERDR_CONFIG_PATH HERDR_PANE_ID
printf '%s\n' "$*" >> "$dir/calls"
[ "$1" != --session ] || shift 2
case "$1 $2" in
  "pane get")
    if [ -f "$dir/gone" ]; then
      printf '{"error":{"code":"pane_not_found","message":"gone"}}\n'
    else
      printf '{"result":{"pane":{"pane_id":"w:p","workspace_id":"w","agent":"omo","status":"blocked"}}}\n'
    fi ;;
  "pane process-info")
    if [ -f "$dir/shell" ]; then
      printf '{"result":{"process_info":{"shell_pid":100,"foreground_processes":[{"pid":100,"argv":["sh"]}]}}}\n'
    else
      printf '{"result":{"process_info":{"shell_pid":100,"foreground_processes":[{"pid":101,"argv":["omo"]}]}}}\n'
    fi ;;
  "pane read") printf '> \n' ;;
  "agent prompt")
    if [ -f "$dir/fail" ]; then
      printf '{"error":{"code":"delivery_failed","message":"fixture failure"}}\n'
    else
      printf '{"result":{}}\n'
    fi ;;
  *) printf '{"error":{"code":"unexpected_call","message":"fixture rejects command"}}\n' ;;
esac
"#;

fn fixture() -> (tempfile::TempDir, Ctx, Task) {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("app")).unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.sync.omo_agent_dir = Some(tmp.path().join("omo"));
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
    store.save_config().unwrap();
    let engine = tmp.path().join("fake");
    std::fs::create_dir(&engine).unwrap();
    let bin = engine.join("herdr");
    std::fs::write(&bin, ENGINE).unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let ctx = Ctx {
        store,
        herdr: corral_herdr::Herdr::with_bin(bin, Some("isolated-fixture".into()))
            .with_poll_wait(|_| panic!("native fixture delivery must not poll")),
        resource_probe: crate::resources::FixedResourceProbe::arc(),
    };
    let task = ctx
        .store
        .create("Plan widget", Some("app"), Status::Ready, "Widget contract")
        .unwrap();
    let source = tmp.path().join("plan.md");
    std::fs::write(&source, PLAN).unwrap();
    crate::plan_submit(&ctx, &task.id, &source).unwrap();
    let task = ctx.store.get(&task.id).unwrap();
    (tmp, ctx, task)
}

fn planner(ctx: &Ctx, task: &Task) -> Task {
    ctx.store
        .update(&task.id, |t| {
            t.plan_runs.push(PlanRun {
                id: "D1".into(),
                profile: "omo".into(),
                harness: "omo".into(),
                model: None,
                vendor: None,
                effort: None,
                spec_revision: crate::plan_spec_revision(t)?,
                started: "2026-10-16T00:00:00Z".parse().unwrap(),
                ended: None,
                herdr: Some(HerdrRef {
                    workspace_id: "w".into(),
                    pane_id: "w:p".into(),
                    socket: Some(ctx.herdr.socket_path()?),
                }),
                output_dir: ctx.store.state_dir().join("planner"),
                submitted: None,
                rounds: 0,
                state: PlanRunState::Planning,
                pending_reply: None,
            });
            t.status = Status::NeedsMe;
            Ok(())
        })
        .unwrap()
}

fn set_review(task: &mut Task, verdict: Option<Verdict>) {
    task.plans.last_mut().unwrap().reviews.push(PlanReview {
        id: "V1".into(),
        reviewer: "owner".into(),
        policy_digest: "policy".into(),
        verdict,
        notes: None,
        at: "2026-10-16T00:00:00Z".parse().unwrap(),
        herdr: None,
        pending_reply: None,
    });
}

fn grant(ctx: &mut Ctx, task: &Task) {
    let preview = ctx
        .store
        .preview_operating_policy(&corral_core::OperatingSelection {
            tasks: [task.id.clone()].into(),
            authority: corral_core::OperatingAuthority {
                dispatch: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    ctx.store.config = ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", Timestamp::UNIX_EPOCH)
        .unwrap();
}

#[test]
fn pending_plan_tracks_only_latest_current_unaccepted_submission() {
    // Given each possible review boundary on a current submission.
    for verdict in [
        None,
        Some(Verdict::Approve),
        Some(Verdict::Escalate),
        Some(Verdict::Changes),
    ] {
        let (_tmp, _ctx, mut task) = fixture();
        set_review(&mut task, verdict);
        // When admission evaluates that latest submission.
        let reason = crate::queue::pending_plan_reason(&task).unwrap();
        // Then review and owner waits block, but requested revisions do not.
        assert_eq!(reason.is_some(), verdict != Some(Verdict::Changes));
    }
}

#[test]
fn historical_pending_plan_does_not_block_newest_approved_or_stale_plan() {
    // Given a pending plan followed by a newer approved or obsolete revision.
    for approved in [false, true] {
        let (_tmp, _ctx, mut task) = fixture();
        let mut latest = task.plans[0].clone();
        latest.id = "P2".into();
        if approved {
            latest.approval = Some(PlanApproval {
                by: "owner".into(),
                at: "2026-10-16T00:00:00Z".parse().unwrap(),
                policy_digest: "policy".into(),
                authorization: None,
            });
        } else {
            latest.spec_revision = "obsolete".into();
        }
        task.plans.push(latest);
        // When admission considers plan history.
        let reason = crate::queue::pending_plan_reason(&task).unwrap();
        // Then only the newest submission is relevant.
        assert!(reason.is_none());
    }
}

#[test]
fn pending_plan_blocks_manual_automatic_and_queued_dispatch_without_launches() {
    for kind in ["manual", "automatic", "queued", "enqueue"] {
        // Given a Ready task with a current unreviewed submission.
        let (tmp, mut ctx, task) = fixture();
        grant(&mut ctx, &task);
        let before = ctx.store.get(&task.id).unwrap();
        // When any implicit dispatch path tries to admit it.
        let result = match kind {
            "manual" => dispatch(&ctx, &task.id, &DispatchOpts::default()).map(|_| ()),
            "automatic" => dispatch_automatic(
                &ctx,
                &task.id,
                json!({"source":"backlog"}),
                Timestamp::UNIX_EPOCH,
            )
            .map(|_| ()),
            "queued" => dispatch_queued(&ctx, &task.id, 1, Timestamp::UNIX_EPOCH).map(|_| ()),
            "enqueue" => {
                crate::queue::enqueue_ready(&ctx, &task.id, &DispatchOpts::default()).map(|_| ())
            }
            _ => unreachable!(),
        };
        // Then the pending plan is explained without IPC or task mutation.
        assert!(result.unwrap_err().to_string().contains("plan P1 pending"));
        assert_eq!(ctx.store.get(&task.id).unwrap(), before);
        assert!(!tmp.path().join("fake/calls").exists());
    }
}

#[test]
fn explicit_replan_passes_pending_review_and_owner_status_guards() {
    for status in [Status::Review, Status::NeedsMe, Status::Ready] {
        // Given a pending plan without a project, so launch stops before subprocesses.
        let (tmp, ctx, task) = fixture();
        ctx.store
            .update(&task.id, |t| {
                t.status = status;
                t.project = None;
                t.plans[0].spec_revision = crate::plan_spec_revision(t)?;
                Ok(())
            })
            .unwrap();
        // When the owner explicitly requests a new planning run.
        let error = dispatch(
            &ctx,
            &task.id,
            &DispatchOpts {
                plan: true,
                ..Default::default()
            },
        )
        .unwrap_err();
        // Then it reaches the planning path rather than rejecting the pending plan.
        assert_eq!(error.to_string(), "plan task has no project");
        assert!(!tmp.path().join("fake/calls").exists());
    }
}

#[test]
fn explicit_replacement_accepts_cancelled_needs_me_after_changes_verdict() {
    // Given a cancelled execution whose reviewed plan requests changes.
    let (tmp, ctx, task) = fixture();
    let task = planner(&ctx, &task);
    ctx.store
        .update(&task.id, |t| {
            set_review(t, Some(Verdict::Changes));
            t.plan_runs[0].state = PlanRunState::Cancelled;
            t.plan_runs[0].ended = Some(Timestamp::UNIX_EPOCH);
            t.project = None;
            t.plans[0].spec_revision = crate::plan_spec_revision(t)?;
            Ok(())
        })
        .unwrap();
    // When the owner explicitly requests replacement from Needs me.
    let error = dispatch(
        &ctx,
        &task.id,
        &DispatchOpts {
            plan: true,
            ..Default::default()
        },
    )
    .unwrap_err();
    // Then status admission permits a new planner, stopping at the missing project.
    assert_eq!(error.to_string(), "plan task has no project");
    assert!(!tmp.path().join("fake/calls").exists());
}

#[test]
fn queue_reports_pending_plan_instead_of_eligible() {
    // Given an otherwise Ready task awaiting a plan review.
    let (_tmp, mut ctx, task) = fixture();
    grant(&mut ctx, &task);
    // When the owner inspects queue admission.
    let queue = crate::queue::dispatch_queue(&ctx).unwrap();
    // Then the submission and its wait are visible.
    let entry = queue.iter().find(|entry| entry.id == task.id).unwrap();
    assert!(entry.reason.contains("plan P1 pending"));
}

#[test]
fn automatic_backlog_admission_preserves_pending_task_and_reports_reason() {
    // Given automatic Backlog admission and an unreviewed current plan.
    let (tmp, mut ctx, task) = fixture();
    ctx.store.config.dispatch.auto_backlog = true;
    ctx.store.save_config().unwrap();
    let before = ctx
        .store
        .update(&task.id, |t| {
            t.status = Status::Backlog;
            Ok(())
        })
        .unwrap();
    grant(&mut ctx, &before);
    // When the full automatic admission pass considers the task.
    let changes = crate::dispatch_policy::advance(&ctx, Timestamp::UNIX_EPOCH).unwrap();
    // Then it skips without moving to Ready or launching, and records why.
    assert!(changes.is_empty());
    assert_eq!(ctx.store.get(&task.id).unwrap(), before);
    assert!(!tmp.path().join("fake/calls").exists());
    let events = ctx.store.events().unwrap();
    assert!(!events.iter().any(|e| e.kind == "dispatch_policy_error"));
    let reason = crate::queue::blocked_reason(&ctx, &ctx.store, &before, false)
        .unwrap()
        .unwrap();
    assert!(reason.contains("P1"));
}

#[test]
fn atomic_admission_requires_explicit_replan_override() {
    for explicit in [false, true] {
        // Given a pending submission at the atomic claim boundary.
        let (_tmp, ctx, task) = fixture();
        // When manual admission rechecks its launch options.
        let reason = Admission::Manual
            .reason(
                &ctx,
                &ctx.store,
                &task,
                &DispatchOpts {
                    plan: explicit,
                    ..Default::default()
                },
            )
            .unwrap();
        // Then only an explicit request bypasses the plan wait.
        assert_eq!(reason.is_some(), !explicit);
    }
}

#[test]
#[cfg(unix)]
fn planner_reply_delivers_to_owned_pane_and_resumes_without_coding_run() {
    // Given a blocked planner with a saved socket and live foreground process.
    let (tmp, ctx, task) = fixture();
    let task = planner(&ctx, &task);
    // When the owner answers the planner.
    let saved = reply(&ctx, &task.id, "Use option B").unwrap();
    // Then only the plan delivery and status change; coding history stays empty.
    assert!(saved.runs.is_empty());
    assert_eq!(saved.status, Status::Running);
    assert!(saved.plan_runs[0].pending_reply.is_none());
    let events = ctx.store.events().unwrap();
    let event = events.iter().find(|e| e.kind == "plan_replied").unwrap();
    assert_eq!(event.detail["target"], "plan");
    assert_eq!(event.detail["submitted"], true);
    let calls = std::fs::read_to_string(tmp.path().join("fake/calls")).unwrap();
    assert!(calls.contains("agent prompt w:p Use option B"));
}

#[test]
#[cfg(unix)]
fn planner_delivery_failure_keeps_exact_reply_for_resend() {
    // Given a live planner and a transport that refuses submission.
    let (tmp, ctx, task) = fixture();
    let task = planner(&ctx, &task);
    std::fs::write(tmp.path().join("fake/fail"), "").unwrap();
    // When the owner replies.
    let error = reply(&ctx, &task.id, "Exact answer").unwrap_err();
    // Then the failure is plan-specific and text remains durable.
    assert!(error.to_string().contains("planner reply not delivered"));
    let saved = ctx.store.get(&task.id).unwrap();
    assert_eq!(
        saved.plan_runs[0].pending_reply.as_deref(),
        Some("Exact answer")
    );
    assert_eq!(saved.status, Status::NeedsMe);
}

#[test]
#[cfg(unix)]
fn planner_resend_uses_durable_plan_text() {
    // Given a prior failed planner delivery, now accepted by the fake transport.
    let (tmp, ctx, task) = fixture();
    let task = planner(&ctx, &task);
    ctx.store
        .update(&task.id, |t| {
            t.plan_runs[0].pending_reply = Some("Saved answer".into());
            Ok(())
        })
        .unwrap();
    // When the owner resends without supplying text.
    let saved = resend_reply(&ctx, &task.id).unwrap();
    // Then the plan text is delivered, not a coding reply.
    assert!(saved.plan_runs[0].pending_reply.is_none());
    let calls = std::fs::read_to_string(tmp.path().join("fake/calls")).unwrap();
    assert!(calls.contains("agent prompt w:p Saved answer"));
}

#[test]
#[cfg(unix)]
fn planner_reply_rejects_gone_shell_and_foreign_panes_before_delivery() {
    for failure in ["gone", "shell", "foreign", "missing", "stale", "exited"] {
        // Given a planner whose pane or ownership is not usable.
        let (tmp, ctx, task) = fixture();
        let task = planner(&ctx, &task);
        ctx.store
            .update(&task.id, |t| {
                match failure {
                    "foreign" => {
                        t.plan_runs[0].herdr.as_mut().unwrap().socket = Some("/foreign.sock".into())
                    }
                    "missing" => t.plan_runs[0].herdr = None,
                    "stale" => t.plan_runs[0].spec_revision = "old".into(),
                    _ => {}
                }
                Ok(())
            })
            .unwrap();
        if matches!(failure, "gone" | "shell") {
            std::fs::write(tmp.path().join("fake").join(failure), "").unwrap();
        }
        if failure == "exited" {
            std::fs::create_dir_all(&task.plan_runs[0].output_dir).unwrap();
            std::fs::write(task.plan_runs[0].output_dir.join("exit-0.status"), "1\n").unwrap();
        }
        // When the owner attempts a reply.
        let error = reply(&ctx, &task.id, "Do not send").unwrap_err();
        // Then no terminal input or agent prompt is sent.
        assert!(error.to_string().contains("planner"));
        let calls = std::fs::read_to_string(tmp.path().join("fake/calls")).unwrap_or_default();
        assert!(!calls.contains("agent prompt"));
        assert!(!calls.contains("send-text"));
        assert!(
            ctx.store.get(&task.id).unwrap().plan_runs[0]
                .pending_reply
                .is_none()
        );
    }
}

#[test]
fn terminal_planner_states_reject_reply_without_ipc() {
    for state in [
        PlanRunState::Submitted,
        PlanRunState::Reviewing,
        PlanRunState::AwaitingOwner,
        PlanRunState::Approved,
        PlanRunState::Escalated,
        PlanRunState::Failed,
        PlanRunState::Cancelled,
    ] {
        // Given a terminal or noninteractive planner.
        let (tmp, ctx, task) = fixture();
        let task = planner(&ctx, &task);
        ctx.store
            .update(&task.id, |t| {
                t.plan_runs[0].state = state;
                Ok(())
            })
            .unwrap();
        // When a reply is attempted.
        let error = reply(&ctx, &task.id, "Do not send").unwrap_err();
        // Then the owner receives a plan-specific error before any pane access.
        assert!(error.to_string().contains("no live planner"));
        assert!(!tmp.path().join("fake/calls").exists());
    }
}

#[test]
#[cfg(unix)]
fn coding_reply_preserves_original_delivery_and_status_semantics() {
    // Given an active coding run, even with historical planner metadata.
    let (_tmp, ctx, task) = fixture();
    let task = planner(&ctx, &task);
    ctx.store
        .update(&task.id, |t| {
            t.plan_runs[0].state = PlanRunState::Approved;
            t.runs.push(serde_json::from_value(json!({
                "id":"R1", "agent":"fake", "worktree":ctx.store.root(), "branch":"main",
                "base":"main", "base_sha":"fixture", "started":"2026-10-16T00:00:00Z",
                "herdr":{"workspace_id":"w", "pane_id":"w:p", "socket":ctx.herdr.socket_path()?},
                "blocked_excerpt":"Question"
            }))?);
            Ok(())
        })
        .unwrap();
    // When the owner answers the coding worker.
    let saved = reply(&ctx, &task.id, "Coding answer").unwrap();
    // Then the original coding reply contract is preserved.
    assert_eq!(saved.status, Status::Running);
    assert!(saved.current_run().unwrap().pending_reply.is_none());
    assert!(saved.current_run().unwrap().blocked_excerpt.is_none());
    assert!(
        ctx.store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.kind == "replied")
    );
    assert!(
        !ctx.store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.kind == "plan_replied")
    );
}
