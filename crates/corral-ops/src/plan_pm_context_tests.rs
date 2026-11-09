//! C3 PM selection is logical policy identity, not permission to stop the workspace PM.
use super::*;
use crate::agent::{BindTarget, bridge::fixture::HostProc, test_support as ts};
use crate::assistant::workflow::handoff as h;
use corral_core::records::RecordArea;
use serde_json::json;

// Disposable transport only: no command executes and closing the shared PM fails loudly.
const ENGINE: &str = r#"#!/usr/bin/env bash
set -eu
cd "$(dirname "$0")"
printf '%s\n' "$*" >> context-calls
if [ "${1:-}" = "--session" ]; then shift 2; fi
ok() { printf '{"id":"fixture","result":%s}\n' "$1"; }
case "$1 $2" in
  "workspace create") ok '{"workspace":{"workspace_id":"w1"},"root_pane":{"workspace_id":"w1","tab_id":"w1:t1","pane_id":"w1:p1"}}' ;;
  "workspace list") ok '{"workspaces":[{"workspace_id":"pm-space"}]}' ;;
  "pane list")
    extra=''; if [ -f review-live ]; then extra=',{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","status":"working"}'; fi
    workspace=pm-space; if [ -f pm-colocated ]; then workspace=w1; fi
    ok "{\"panes\":[{\"pane_id\":\"pm-pane\",\"workspace_id\":\"$workspace\",\"status\":\"working\"}$extra]}" ;;
  "pane get")
    if [ "$3" = pm-pane ]; then workspace=pm-space; if [ -f pm-colocated ]; then workspace=w1; fi; ok "{\"pane\":{\"pane_id\":\"pm-pane\",\"workspace_id\":\"$workspace\",\"status\":\"working\"}}"
    elif [ "$3" = w1:p1 ] && [ -f review-live ]; then ok '{"pane":{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","status":"working"}}'
    else ok '{}'; fi ;;
  "pane report-metadata") [ "$3" = w1:p1 ]; ok '{}' ;;
  "pane run") printf '%s' "$4" > launch-command; touch review-live; ok '{}' ;;
  "pane process-info") ok '{"process_info":{"shell_pid":10,"foreground_processes":[]}}' ;;
  "pane close") [ "$3" != pm-pane ]; rm -f review-live; ok '{}' ;;
  "workspace close") [ "$3" != pm-space ]; if [ -f pm-colocated ] && [ "$3" = w1 ]; then exit 95; fi; ok '{}' ;;
  "agent prompt") [ "$3" = pm-pane ]; ok '{}' ;;
  *) printf 'unexpected fixture request: %s\n' "$*" >&2; exit 94 ;;
esac
"#;

fn hello_process(fx: &ts::Fx, host: &ts::Host, process: &HostProc) {
    let payload = json!({
        "ctl_cap_sha256": paths::sha256_hex(host.ctl_cap.as_bytes()),
        "session_file": host.sessions.join("session.jsonl"),
        "pid": process.pid(), "start_time": process.start_time(), "pane": host.pane,
    });
    ts::committed(&ts::send(
        fx,
        &ts::envelope(
            host,
            "hello",
            &crate::agent::event_request_id(1, 0),
            payload,
            true,
        ),
        "hello",
    ));
    ts::committed(&ts::send(
        fx,
        &ts::event_input(host, 1, "turn_started", 1),
        "event",
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn handoff_pm_context_restart_drains_only_the_bound_attempt_and_returns_latest_plan() {
    let (tmp, mut ctx, id) = preview_fixture();
    let assets = native_assets(tmp.path(), &mut ctx);
    ts::write_executable(&tmp.path().join("engine/herdr"), ENGINE);
    let supported: Vec<_> = corral_core::agent::CATALOG
        .iter()
        .filter(|d| d.host)
        .map(|d| d.key)
        .collect();
    std::fs::write(
        assets.join("compat.json"),
        serde_json::to_vec(&json!({
            "version":1,"verdict":"GO","bun":{"range":"1.4.2"},"capabilities":supported,
        }))
        .unwrap(),
    )
    .unwrap();
    ctx.store.config.agent.capabilities = serde_json::from_value(json!({"preset":"full"})).unwrap();
    ctx.store.config.pm.agent = "gpt-review".into();
    ctx.store.config.review.reviewer = Some("pm".into());
    let profile = ctx.store.config.profiles.get_mut("gpt-review").unwrap();
    profile.harness = corral_core::CORRAL_PI_HARNESS.into();
    profile.model = Some("gpt-6.1-sol".into());
    profile.vendor = Some("openai".into());
    profile.effort = Some("high".into());
    ctx.store.save_config().unwrap();
    let selected = ctx
        .store
        .config
        .resolve_profile(
            &corral_core::ProfileChoice {
                profile: Some("gpt-review".into()),
                ..Default::default()
            },
            None,
            None,
        )
        .unwrap();
    let plan = ctx.store.get(&id).unwrap().plans.last().unwrap().clone();
    let source = exact_source(&ctx, &id, &plan, h::Goal::PlanContinue);
    let source_before = ctx.store.get_assistant_container(&source).unwrap();
    // Logical `pm` must not bypass an explicitly required normal family floor.
    let prior_plans = ctx.store.get(&id).unwrap().plans;
    assert!(prior_plans[0].reviews.is_empty());
    let family_policy = ctx.store.config.review.require_cross_family;
    ctx.store.config.review.require_cross_family = Some(true);
    ctx.store.save_config().unwrap();
    let refusal = crate::plan_review(&ctx, &id, Some("pm")).unwrap_err();
    eprintln!("C3_PM_INDEPENDENCE_REFUSAL: {refusal:#}");
    assert_eq!(ctx.store.get(&id).unwrap().plans, prior_plans);
    assert!(!tmp.path().join("engine/launch-command").exists());
    ctx.store.config.review.require_cross_family = family_policy;
    ctx.store.save_config().unwrap();

    let fx = ts::Fx {
        _tmp: tempfile::tempdir().unwrap(),
        ctx: ctx.clone(),
    };
    RecordArea::<serde_json::Value>::new("pm").save(&ctx.store, "state", &json!({
        "pane_id":"pm-pane", "execution":{"harness":"corral-pi","model":"gpt-6.1-sol","vendor":"openai"}
    })).unwrap();
    let pm = ts::bind(
        &fx,
        &BindTarget::Pm {
            project: ctx.store.get(&id).unwrap().project.unwrap(),
        },
        "pm-pane",
        "unrelated-pm-session",
    );
    let mut pm_process = HostProc::spawn(true);
    hello_process(&fx, &pm, &pm_process);
    let pm_before = ts::state(&fx, &ContainerRef::Pm);
    assert!(
        pm_before
            .current_generation()
            .unwrap()
            .capabilities
            .as_ref()
            .unwrap()
            .enabled("codemode")
    );

    let review = crate::plan_review(&ctx, &id, Some("pm")).unwrap();
    assert_eq!(review.reviewer, "pm");
    let Some(corral_core::PlanReviewContext::C3PmIsolatedV1 { profile }) = &review.context else {
        panic!("explicit isolated context required")
    };
    assert_eq!(**profile, selected);
    let state = review
        .agent_host
        .as_ref()
        .expect("C3 PM review owns an isolated native attempt");
    assert_eq!(state.binding.role, AgentRole::PlanReviewer);
    assert_eq!(state.binding.generation, 1);
    assert_ne!(review.herdr.as_ref().unwrap().pane_id, "pm-pane");
    let caps = state
        .current_generation()
        .unwrap()
        .capabilities
        .as_ref()
        .unwrap();
    for key in [
        "mcp.enabled",
        "codemode",
        "web.enabled",
        "background_shell",
        "subagents.enabled",
        "memory.enabled",
    ] {
        assert!(
            !caps.enabled(key),
            "review context must not inherit PM extra capability {key}"
        );
    }
    assert_eq!(caps.project_text, Some(false));
    assert!(caps.mcp.is_none() && caps.web.is_none());
    let command = std::fs::read_to_string(tmp.path().join("engine/launch-command")).unwrap();
    assert!(
        command.contains("agent launch --bootstrap")
            && command.contains(&assets.display().to_string())
    );
    assert!(
        !std::fs::read_to_string(tmp.path().join("engine/context-calls"))
            .unwrap()
            .contains("agent prompt pm-pane")
    );
    let container = ContainerRef::PlanReviewer {
        task: id.clone(),
        plan: plan.id.clone(),
        review: review.id.clone(),
    };
    let reviewer = reviewer_from_bootstrap(&ctx, &container);
    let recipient = corral_core::inbox::Recipient::PlanReview {
        task: id.clone(),
        plan: plan.id.clone(),
        review: review.id.clone(),
    };
    let bootstrap: crate::agent::launch::Bootstrap = serde_json::from_slice(
        &std::fs::read(
            crate::agent::launch::generation_dir(&ctx.store, &recipient, 1)
                .unwrap()
                .join(crate::agent::launch::BOOTSTRAP_FILE),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(bootstrap.role, AgentRole::PlanReviewer);
    assert_eq!(bootstrap.model, "gpt-6.1-sol");
    assert_eq!(bootstrap.effort.as_deref(), Some("high"));
    assert!(bootstrap.resume_session.is_none());
    assert_eq!(bootstrap.capabilities.as_ref(), Some(caps));
    assert_ne!(reviewer.identity.session_id, pm.identity.session_id);
    assert_ne!(reviewer.sessions, pm.sessions);
    let mut review_process = HostProc::spawn(true);
    hello_process(&fx, &reviewer, &review_process);
    let notes = "Latest native review working notes; not a verdict or owner approval.\n";
    let note_key = crate::agent::tool_request_id(&reviewer.identity.session_id, "latest-notes");
    let note_input = ts::envelope(
        &reviewer,
        "plan_write",
        &note_key,
        json!({"content":notes,"offset":0}),
        false,
    );
    let write = ts::committed(&ts::send(&fx, &note_input, "plan_write"));
    let notes_dir = crate::scratch_reclaim::Attempt::PlanReview {
        task: &id,
        plan: &plan.id,
        review: &review.id,
    }
    .dir(&ctx)
    .unwrap();
    assert_eq!(
        std::fs::read(notes_dir.join("notes.md")).unwrap(),
        notes.as_bytes()
    );
    // A smaller stopped-planner record supplies newer unsubmitted work. It is not
    // a running process fixture: the actual live holder above is the native reviewer.
    let draft_dir = crate::scratch_reclaim::Attempt::Planner {
        task: &id,
        run: "D2",
    }
    .open(&ctx)
    .unwrap();
    let latest = "# Latest stopped planning work\n\nKeep this refinement, not the transferred predecessor.\n";
    std::fs::write(draft_dir.join("plan.md"), latest).unwrap();
    ctx.store
        .update(&id, |task| {
            let mut stopped = task.plan_runs[0].clone();
            stopped.id = "D2".into();
            stopped.output_dir = draft_dir.clone();
            stopped.state = corral_core::PlanRunState::Cancelled;
            stopped.ended = Some(corral_core::clock::now());
            stopped.submitted = None;
            stopped.agent_host = None;
            stopped.herdr = None;
            stopped.inbox.settled = stopped.ended;
            task.plan_runs.push(stopped);
            Ok(())
        })
        .unwrap();
    // Model an owner moving an unrelated PM pane into the review workspace.
    std::fs::write(tmp.path().join("engine/pm-colocated"), "").unwrap();
    let current = ctx.store.get(&id).unwrap();
    let instruction = h::lifecycle::Instruction::Shell {
        text: "fixture: take back this task's latest work".into(),
        at: corral_core::clock::now(),
    };
    let artifact_cap = ctx.store.config.artifacts.max_bytes;
    ctx.store.config.artifacts.max_bytes = 4;
    ctx.store.save_config().unwrap();
    let interrupted = h::takeback::apply(
        &ctx,
        &id,
        &current.contract_revision().unwrap(),
        instruction.clone(),
    )
    .unwrap_err();
    eprintln!("C3_PM_ARCHIVE_HELD: {interrupted:#}");
    assert!(
        review_process.reaped(),
        "archive failure occurs only after physical settlement"
    );
    let pending = ctx.store.get(&id).unwrap();
    let journal = h::lifecycle::for_task(&ctx.store, &pending)
        .unwrap()
        .unwrap();
    assert_eq!(
        journal.transfer.as_ref().unwrap().disposition,
        h::lifecycle::Disposition::TakingBack
    );
    assert!(h::lifecycle::board_owner(&ctx.store, &journal).unwrap());
    assert_eq!(
        ctx.store
            .get_assistant_container(&source)
            .unwrap()
            .workflow
            .as_ref()
            .unwrap()
            .current
            .owner,
        corral_core::assistant_workflow::ScopeOwner::Board { task: id.clone() }
    );
    let mut ctx = Ctx {
        store: corral_core::Store::open(ctx.store.root()).unwrap(),
        ..ctx.clone()
    };
    ctx.store.config.artifacts.max_bytes = artifact_cap;
    ctx.store.save_config().unwrap();
    h::recovery::reconcile(&ctx).unwrap();
    let returned = h::takeback::apply(
        &ctx,
        &id,
        &current.contract_revision().unwrap(),
        instruction,
    )
    .unwrap();
    assert!(
        matches!(returned, h::takeback::Returned::Ready { .. }),
        "{returned:?}"
    );
    assert!(
        review_process.reaped(),
        "the positively bound dedicated host must be physically gone"
    );
    assert!(
        !pm_process.reaped(),
        "the shared PM and unrelated context stay alive"
    );
    let held = ctx.store.get(&id).unwrap();
    assert!(held.dispatch_hold && !held.queued && held.runs.is_empty() && held.landed.is_none());
    assert_eq!(held.plans[0].artifact, plan.artifact);
    assert_eq!(held.plans[0].run, plan.run);
    assert!(held.plans[0].approval.is_none());
    assert!(held.plans[0].reviews[0].cancelled && held.plans[0].reviews[0].verdict.is_none());
    let retired = held.plans[0].reviews[0].agent_host.as_ref().unwrap();
    assert_eq!(
        retired.binding.health,
        corral_core::agent::HostHealth::Fenced
    );
    assert!(
        retired.turns.iter().all(|t| t.ended.is_none()),
        "do not invent turn-ended evidence"
    );
    let local = ctx.store.get_assistant_container(&source).unwrap();
    let workflow = &local.workflow.as_ref().unwrap().current;
    assert_eq!(
        workflow.owner,
        corral_core::assistant_workflow::ScopeOwner::Local
    );
    assert!(workflow.execution.is_none());
    let local_plan = workflow.current_plan.as_ref().unwrap();
    assert_eq!(
        local_plan.stage,
        corral_core::assistant_workflow::DesignStage::Draft
    );
    assert_eq!(
        crate::assistant::workflow::read_revision(
            &ctx.store,
            &source,
            &local,
            local_plan.revision,
            0,
            1024
        )
        .unwrap()["text"],
        latest
    );
    assert!(
        local_plan.revision
            > source_before
                .workflow
                .as_ref()
                .unwrap()
                .current
                .current_plan
                .as_ref()
                .unwrap()
                .revision
    );
    let completion = held.cowork.as_ref().unwrap().completion.as_ref().unwrap();
    assert!(!held.cowork.as_ref().unwrap().owner_confirmed);
    assert_eq!(completion.intent, corral_core::CompletionIntent::Verify);
    let archive = held
        .artifacts
        .iter()
        .find(|a| {
            a.title
                .contains("isolated-pm-review-notes-checkpoint-not-a-verdict")
        })
        .unwrap();
    assert_eq!(
        crate::artifact_storage::artifact_read_confined(&ctx, archive).unwrap(),
        notes.as_bytes()
    );
    std::fs::remove_dir_all(&notes_dir).unwrap();

    // Reopen before replay: no warm controller state is allowed to rearm the actor.
    let reopened = Ctx {
        store: corral_core::Store::open(ctx.store.root()).unwrap(),
        ..ctx.clone()
    };
    let after = ts::Fx {
        _tmp: tempfile::tempdir().unwrap(),
        ctx: reopened.clone(),
    };
    let late = ts::envelope(
        &reviewer,
        "plan_write",
        &crate::agent::tool_request_id(&reviewer.identity.session_id, "late-note"),
        json!({"content":"late overwrite","offset":0}),
        false,
    );
    assert!(ts::code(&ts::send(&after, &late, "plan_write")).is_some());
    let verdict = ts::envelope(
        &reviewer,
        "plan_verdict",
        &crate::agent::tool_request_id(&reviewer.identity.session_id, "late-verdict"),
        json!({"verdict":"approve","notes":"late approval"}),
        false,
    );
    assert!(ts::code(&ts::send(&after, &verdict, "plan_verdict")).is_some());
    assert!(
        crate::agent::open_generation(
            &reopened.store,
            &BindTarget::PlanReviewer {
                task: id.clone(),
                plan: plan.id.clone(),
                review: review.id.clone(),
            },
            "late-restart"
        )
        .is_err()
    );
    let replay = ts::send(&after, &note_input, "plan_write");
    if let crate::agent::CallResponse::Receipt {
        receipt, replayed, ..
    } = &replay
    {
        assert!(*replayed);
        assert_eq!(receipt, &write);
    } else {
        assert!(ts::code(&replay).is_some());
    }
    assert!(
        !notes_dir.exists(),
        "late/replayed effects must not recreate output after its archive"
    );
    assert_eq!(reopened.store.get(&id).unwrap().plans, held.plans);
    assert_eq!(reopened.store.get(&id).unwrap().plan_runs, held.plan_runs);
    assert_eq!(reopened.store.list().unwrap().len(), 1);
    assert_eq!(
        reopened.store.get_assistant_container(&source).unwrap(),
        local
    );
    assert_eq!(
        crate::artifact_storage::artifact_read_confined(&reopened, archive).unwrap(),
        notes.as_bytes()
    );
    let root_read = ts::envelope(
        &pm,
        "read_show",
        &crate::agent::tool_request_id(&pm.identity.session_id, "unrelated-after-return"),
        json!({"task":id}),
        false,
    );
    ts::committed(&ts::send(&after, &root_read, "read_show"));
    reopened
        .herdr
        .agent_prompt("pm-pane", "unrelated owner request still works")
        .unwrap();
    let pm_after = ts::state(&after, &ContainerRef::Pm);
    assert_eq!(pm_after.binding, pm_before.binding);
    assert_eq!(
        pm_after.current_generation(),
        pm_before.current_generation()
    );
    assert!(!pm_process.reaped());
    eprintln!(
        "C3_PM_CONTEXT: configured PM profile/model/effort + normal independence; fresh readonly PlanReviewer scope, actual notes RPC + verified owned PID stop, same-task latest draft and notes archive return, prepare-only, reopened late effects/verdict/replay fenced; shared PM process/authenticated context survives. Scripted transport + process stand-ins, not real SDK/model or owner consent."
    );
    let _ = std::fs::remove_dir_all(
        crate::agent::launch::runtime_root().join(paths::store_id(&ctx.store)),
    );
}
