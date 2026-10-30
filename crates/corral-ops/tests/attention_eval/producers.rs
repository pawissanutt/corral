//! S19 production-producer kill barriers (in-crate: the barriers are cfg(test) seams).
use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use corral_core::{AutoStage, HerdrRef, Status};
use corral_herdr::{AgentStatus, PaneInfo};
use serde_json::json;

use super::super::{Stats, Window, stats};
use crate::attention::handling_tests::fixture::{Fx, at};
use crate::owner_notice::{CRASH, CrashPoint};

const PRODUCERS: [&str; 12] = [
    "obligation_transition",
    "provider_exhausted",
    "autopilot_escalate",
    "landing_blocked",
    "legacy_review_fallback",
    "legacy_review_watchdog",
    "native_review_watchdog",
    "availability_exhausted",
    "native_review_host_lost",
    "needs_me_entry",
    "stuck",
    "question_needs_me",
];

fn panes() -> HashMap<String, PaneInfo> {
    HashMap::from([(
        "worker:p1".into(),
        PaneInfo {
            pane_id: "worker:p1".into(),
            workspace_id: "worker".into(),
            tab_id: "worker:t1".into(),
            agent: None,
            status: AgentStatus::Idle,
            title: None,
            cwd: None,
        },
    )])
}

fn write_state(fx: &Fx, area: &str, key: &str, value: serde_json::Value) {
    let dir = fx.ctx.store.state_dir().join(area);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{key}.json")),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
}

fn legacy_worker(fx: &Fx, id: &str) {
    fx.ctx
        .store
        .update(id, |task| {
            let run = task.current_run_mut().unwrap();
            run.agent_host = None;
            run.herdr = Some(HerdrRef {
                workspace_id: "worker".into(),
                pane_id: "worker:p1".into(),
                socket: Some(fx.ctx.herdr.socket_path()?),
            });
            Ok(())
        })
        .unwrap();
}

fn review(fx: &Fx, id: &str, producer: &str) {
    fx.ctx.store.update(id, |task| {
        let parent = task.current_run_mut().unwrap();
        parent.review = Some(serde_json::from_value(json!({
            "reviewer":"fixture","sha":"abc","at":at(0),"pane":"worker:p1"
        }))?);
        if producer != "legacy_review_watchdog" {
            let native = producer != "legacy_review_fallback";
            let mut child: corral_core::Run = serde_json::from_value(json!({
                "id":"R1-review1","agent":"fixture","worktree":"/missing-fixture-worktree",
                "branch":"corral/T","base":"main","base_sha":"abc","started":at(0)
            }))?;
            if native {
                child.harness = Some("pi-native".into());
                child.agent_host = Some(Box::new(serde_json::from_value(json!({
                    "version":1,
                    "binding":{"role":"reviewer","store":"store","project":"app","generation":1,
                        "health":if producer == "native_review_host_lost" {"lost"} else {"healthy"}},
                    "generations":[{"n":1,"bind_nonce_sha256":"nonce","pane":"worker:p1",
                        "created":at(0),"nudged_at":at(1)}],
                    "turns":[
                        {"generation":1,"seq":1,"origin":{"kind":"bridge"},"ended":at(1),"class":"no_verdict"},
                        {"generation":1,"seq":2,"origin":{"kind":"bridge"},"ended":at(2),"class":"no_verdict"}
                    ]
                }))?));
            }
            parent.review_runs.push(child);
        }
        Ok(())
    }).unwrap();
}

fn authorize(fx: &mut Fx, id: &str) {
    std::fs::create_dir_all(fx.tmp.path().join("app")).unwrap();
    fx.ctx.store.config.projects.insert(
        "app".into(),
        serde_json::from_value(json!({"path":"app"})).unwrap(),
    );
    fx.ctx
        .store
        .update(id, |task| {
            task.project = Some("app".into());
            Ok(())
        })
        .unwrap();
    fx.ctx.store.save_config().unwrap();
    let preview = fx
        .ctx
        .store
        .preview_operating_policy(&corral_core::OperatingSelection {
            tasks: [id.to_owned()].into(),
            authority: corral_core::OperatingAuthority {
                dispatch: true,
                accept: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    fx.ctx.store.config = fx
        .ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", at(0))
        .unwrap();
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .env("GIT_MASTER", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn landing(fx: &mut Fx, id: &str) {
    // Reuse an existing immutable commit, never commit or rebase the working checkout.
    let repo = fx.tmp.path().join("app");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "other"]);
    git(
        &repo,
        &[
            "fetch",
            "-q",
            "--depth=1",
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .to_str()
                .unwrap(),
            "HEAD",
        ],
    );
    git(&repo, &["checkout", "-q", "FETCH_HEAD"]);
    git(&repo, &["switch", "-q", "-c", "other"]);
    git(&repo, &["branch", "main"]);
    let candidate = repo.join(".worktrees/candidate");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "candidate",
            candidate.to_str().unwrap(),
            "main",
        ],
    );
    let sha = git(&candidate, &["rev-parse", "HEAD"]);
    fx.ctx
        .store
        .update(id, |task| {
            let run = task.current_run_mut().unwrap();
            run.worktree = candidate;
            run.base_sha = sha.clone();
            run.autopilot = Some(corral_core::AutopilotState {
                stage: AutoStage::Landing,
                sha: Some(sha),
                reason: None,
                at: at(0),
            });
            Ok(())
        })
        .unwrap();
    fx.ctx.store.config.review.gate = Some(vec!["true".into()]);
}

fn setup(producer: &str) -> (Fx, String) {
    let mut fx = Fx::new();
    let reviewing = matches!(
        producer,
        "autopilot_escalate"
            | "landing_blocked"
            | "legacy_review_fallback"
            | "legacy_review_watchdog"
            | "native_review_watchdog"
            | "native_review_host_lost"
    );
    let task = if reviewing {
        fx.stalled(None)
    } else {
        fx.questions(usize::from(producer == "question_needs_me"))
    };
    match producer {
        "obligation_transition" => {}
        "provider_exhausted" => {
            legacy_worker(&fx, &task.id);
            let session = fx.tmp.path().join("provider.jsonl");
            std::fs::write(
                &session,
                json!({"type":"message","timestamp":at(0),
                    "message":{"role":"assistant","stopReason":"error",
                        "errorMessage":"provider overloaded"}})
                .to_string(),
            )
            .unwrap();
            write_state(
                &fx,
                "provider-stall",
                &format!("{}-R1", task.id),
                json!({"since":at(0),"error_at":at(0),"message":"provider overloaded",
                    "attempts":3,"last_attempt":at(0),"escalated":false,"session":session}),
            );
        }
        "autopilot_escalate" | "landing_blocked" => {
            fx.ctx.store.config.review.gate = Some(Vec::new());
            fx.ctx.store.config.review.reviewer = Some("none".into());
            fx.ctx.store.config.review.accept = Some(corral_core::Accept::Auto);
            if producer == "landing_blocked" {
                landing(&mut fx, &task.id);
            }
            authorize(&mut fx, &task.id);
        }
        "legacy_review_fallback"
        | "legacy_review_watchdog"
        | "native_review_watchdog"
        | "native_review_host_lost" => review(&fx, &task.id, producer),
        "availability_exhausted" => {
            // Restore a selected continuation; Assist refuses dispatch before launching anything.
            fx.ctx
                .store
                .update(&task.id, |task| {
                    task.current_run_mut().unwrap().agent_host = None;
                    Ok(())
                })
                .unwrap();
            write_state(
                &fx,
                "provider-stall",
                &format!("{}-R1", task.id),
                json!({"since":at(0),"error_at":at(0),"message":"model_not_found",
                    "attempts":0,"last_attempt":null,"escalated":true,"fallback_selected":true}),
            );
            let episode = format!("{}-R1-implementation", task.id);
            write_state(
                &fx,
                "availability",
                &episode,
                json!({
                    "version":1,"id":episode,"task":task.id,"role":"implementation",
                    "profile":"fixture","revision":"fixture-r1","started":at(0),
                    "attempts":[
                        {"candidate":"first","provider":"first","connection":"fixture","model":"first/model",
                            "run":"R1","selected_at":at(0),"reason":"preferred"},
                        {"candidate":"second","provider":"second","connection":"fixture","model":"second/model",
                            "run":null,"selected_at":at(1),"reason":"selected fallback"}
                    ]
                }),
            );
        }
        "needs_me_entry" | "stuck" => {
            legacy_worker(&fx, &task.id);
            let script = std::fs::read_to_string(&fx.bin).unwrap();
            let status = if producer == "stuck" {
                "idle"
            } else {
                "blocked"
            };
            let script = script.replace(
                "\"agent_status\":\"working\"",
                &format!("\"agent_status\":\"{status}\""),
            );
            let script = script.replace(
                "  \"pane read\")",
                "  \"pane process-info\") printf '%s\\n' '{\"result\":{\"process_info\":{\"shell_pid\":10,\"foreground_processes\":[{\"pid\":42,\"argv\":[\"grep\",\"pattern\"]}]}}}' ;;\n  \"pane read\")",
            );
            std::fs::write(&fx.bin, script).unwrap();
            if producer == "stuck" {
                fx.ctx
                    .store
                    .update(&task.id, |task| {
                        task.current_run_mut().unwrap().herdr_idle_since = Some(at(0));
                        Ok(())
                    })
                    .unwrap();
            }
        }
        "question_needs_me" => {
            fx.ctx
                .store
                .update(&task.id, |task| {
                    task.current_run_mut()
                        .unwrap()
                        .agent_host
                        .as_mut()
                        .unwrap()
                        .questions[0]
                        .state = corral_core::agent::QuestionState::Escalated;
                    Ok(())
                })
                .unwrap();
        }
        _ => panic!("unknown producer {producer}"),
    }
    (fx, task.id)
}

struct Processes;

impl crate::ProcessControl for Processes {
    fn inspect(&self, pid: u32) -> Result<Option<crate::ProcessSnapshot>> {
        Ok((pid == 42).then(|| crate::ProcessSnapshot {
            parent_pid: 41,
            started: "fixture-start".into(),
            elapsed_secs: 10000,
            argv: vec!["grep".into(), "pattern".into()],
        }))
    }

    fn terminate(&self, _: &corral_core::StuckProcess) -> Result<()> {
        panic!("notification evaluation must never terminate a process")
    }
}

fn invoke(fx: &Fx, id: &str, producer: &str) -> Result<()> {
    let ctx = fx.reopen();
    let task = ctx.store.get(id)?;
    let run = task.current_run().unwrap();
    match producer {
        "obligation_transition" => {
            crate::obligations::register_decision(
                &ctx,
                id,
                serde_json::from_value(json!({
                    "version":1,"id":"fixture-decision","run":"R1","question":"Choose a constraint",
                    "requested_action":"answer","owner":"owner","source":"worker","deadline":null
                }))?,
                at(1000),
            )?;
        }
        "provider_exhausted" => {
            crate::provider_stall::advance_agent(&ctx, &task, &panes(), at(1000))?;
        }
        "autopilot_escalate" | "landing_blocked" => {
            crate::autopilot::advance_all(&ctx, at(1000))?;
        }
        "legacy_review_fallback" => {
            if task.status == Status::Review {
                crate::reviewer::continue_review(
                    &ctx,
                    &task,
                    run,
                    &run.review_runs[0],
                    "fallback unavailable",
                    at(1000),
                )?;
            }
        }
        "legacy_review_watchdog" => {
            crate::reviewer::reconcile_review_at(&ctx, id, at(1000), &panes())?;
        }
        "native_review_watchdog" => {
            crate::agent::hooks::review::nudge(&ctx, id)?;
        }
        "availability_exhausted" => {
            crate::availability::advance_all(&ctx, at(1000))?;
        }
        "native_review_host_lost" => {
            crate::agent::hooks::review::hold_lost(&ctx, id)?;
        }
        "needs_me_entry" | "stuck" => {
            crate::sync_at_with_processes(&ctx, at(1000), &Processes)?;
        }
        "question_needs_me" => {
            crate::questions::hold_owner_attention(&ctx, id)?;
        }
        _ => panic!("unknown producer {producer}"),
    }
    Ok(())
}

fn snapshot(fx: &Fx) -> Stats {
    stats(
        &fx.reopen(),
        Window {
            start: "2000-01-01T00:00:00Z".parse().unwrap(),
            end: at(8000),
        },
    )
    .unwrap()
}

fn assert_unknown(stats: &Stats, producer: &str) {
    let notifications = &stats.notifications;
    let row = &notifications.by_producer[producer];
    assert_eq!(
        (
            row.batches,
            row.claims,
            row.success,
            row.failure,
            row.outcome_unknown,
            row.certainly_not_invoked,
        ),
        (1, 1, 0, 0, 1, 0),
        "{producer}"
    );
    assert_eq!(notifications.by_producer.len(), 1, "{producer}");
    assert_eq!(notifications.calls, [0, 1], "{producer}");
    assert_eq!(notifications.carry_in, 0, "{producer}");
    assert_eq!(notifications.carry_in_calls, [0, 0], "{producer}");
}

fn crash_window(producer: &str, point: CrashPoint) -> Stats {
    // Given the producer's real source and a separate transport invocation oracle.
    let (fx, id) = setup(producer);
    let baseline = fx.calls();
    assert_eq!(baseline, 0, "{producer} setup");
    // When its production entry stops at the barrier, then restarts on the same store.
    CRASH.with(|crash| crash.set(Some(point)));
    let result = invoke(&fx, &id, producer);
    let remaining = CRASH.with(|crash| crash.replace(None));
    assert_eq!(remaining, None, "{producer} did not reach {point:?}");
    if matches!(producer, "autopilot_escalate" | "landing_blocked") {
        // advance_all deliberately catches per-task errors and records their actual cause.
        result.unwrap();
        assert!(
            fx.ctx.store.events().unwrap().iter().any(|event| {
                event.kind == "autopilot_error"
                    && event.detail["error"]
                        .as_str()
                        .is_some_and(|error| error.contains("injected crash"))
            }),
            "{producer}: missing injected autopilot error"
        );
    } else {
        let error = result.unwrap_err();
        assert!(
            format!("{error:#}").contains("injected crash"),
            "{producer}: {error:#}"
        );
    }
    let expected = usize::from(point == CrashPoint::AfterNotify);
    assert_eq!(fx.calls() - baseline, expected, "{producer} {point:?}");
    crate::owner_notice::drain(&fx.reopen()).unwrap();
    let before = snapshot(&fx);
    assert_unknown(&before, producer);
    // Then re-entering the source and further syncs never recover send authority.
    invoke(&fx, &id, producer).unwrap();
    crate::sync_at_with_processes(&fx.reopen(), at(1001), &Processes).unwrap();
    assert_eq!(fx.calls() - baseline, expected, "{producer} replay");
    fx.ctx
        .store
        .update(&id, |task| {
            task.status = Status::Done;
            if producer == "stuck" {
                task.current_run_mut().unwrap().stuck = None;
            }
            Ok(())
        })
        .unwrap();
    for seconds in [7200, 7201] {
        crate::sync_at_with_processes(&fx.reopen(), at(seconds), &Processes).unwrap();
    }
    let after = snapshot(&fx);
    assert_unknown(&after, producer);
    assert_eq!(
        before.notifications.by_producer, after.notifications.by_producer,
        "{producer} prune"
    );
    assert_eq!(fx.calls() - baseline, expected, "{producer} final");
    assert!(
        fx.ctx
            .store
            .get(&id)
            .unwrap()
            .attention
            .notice_outbox
            .is_empty(),
        "{producer}: source claim was not pruned"
    );
    after
}

#[test]
fn all_producers_have_identical_unknown_metrics_before_and_after_send_without_resends() {
    for producer in PRODUCERS {
        let pre = crash_window(producer, CrashPoint::BeforeNotify);
        let post = crash_window(producer, CrashPoint::AfterNotify);
        assert_eq!(pre.notifications, post.notifications, "{producer}");
    }
}

#[test]
fn commit_producers_replay_after_claim_without_transport_or_known_outcomes() {
    for producer in [
        "obligation_transition",
        "autopilot_escalate",
        "landing_blocked",
    ] {
        crash_window(producer, CrashPoint::AfterClaim);
    }
}
