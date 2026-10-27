//! Parking acceptance through real sync, canonical records and disposable transports.
#![cfg(unix)]
use corral_core::{
    Park, Responsible, Status, Store, SubmissionDisposition, Task, parse_park_check,
};
pub use corral_ops::{Ctx, resources};
use corral_ops::{DispatchOpts, attention};
use jiff::SignedDuration;
use serde_json::json;

#[path = "attention_handling/fixture.rs"]
mod attention_fixture;
#[path = "operating_launch/fixture.rs"]
mod operating_fixture;
use attention_fixture::at;

fn wakes_for(fx: &attention_fixture::Fx, id: &str) -> usize {
    fx.ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|event| {
            event.kind == "attention_due"
                && event.detail["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|entry| entry["task"] == id)
        })
        .map(|event| event.detail["fact_id"].as_str().unwrap().to_owned())
        .collect::<std::collections::BTreeSet<_>>()
        .len()
}

fn park(store: &Store, id: &str, checks: &[&str]) -> Task {
    store
        .update(id, |task| {
            task.park = Some(Park {
                version: 1,
                revision: 1,
                by: Responsible::Pm,
                reason: "Wait for prerequisites".into(),
                at: at(10),
                resume_when: checks
                    .iter()
                    .map(|check| parse_park_check(check))
                    .collect::<anyhow::Result<_>>()?,
                resume_action: "Inspect retained work before resuming".into(),
                review_by: at(10).checked_add(SignedDuration::from_hours(24))?,
                ready_at: None,
            });
            Ok(())
        })
        .unwrap()
}

fn owner_landing_resume(strategy: corral_ops::ApproveStrategy, expected: bool) {
    let mut fx = operating_fixture::Fx::new();
    fx.ctx.store.config.review.reviewer = Some("none".into());
    fx.ctx.store.save_config().unwrap();
    let prerequisite = fx.task("Land prerequisite", Status::Ready);
    fx.start(&prerequisite.id);
    fx.done(&prerequisite.id, "prerequisite.txt");
    let candidate = fx.ctx.store.get(&prerequisite.id).unwrap();
    let head = operating_fixture::git(
        &candidate.current_run().unwrap().worktree,
        &["rev-parse", "HEAD"],
    );
    corral_ops::gate_start(&fx.ctx, &prerequisite.id).unwrap();
    assert_eq!(
        corral_ops::gate_collect(&fx.ctx, &prerequisite.id)
            .unwrap()
            .unwrap()
            .result,
        corral_core::GateResult::Pass,
    );
    let dependent = fx.task("Wait for landing", Status::Ready);
    park(
        &fx.ctx.store,
        &dependent.id,
        &[&format!("landed:{}", prerequisite.id)],
    );
    let before = jiff::Timestamp::now();
    corral_ops::sync_at(&fx.fresh(), before).unwrap();
    assert!(
        fx.ctx
            .store
            .get(&dependent.id)
            .unwrap()
            .park
            .unwrap()
            .ready_at
            .is_none()
    );
    let previous = fx.ctx.store.get(&prerequisite.id).unwrap();
    corral_ops::approve(&fx.ctx, &prerequisite.id, strategy, false, false).unwrap();
    let main = operating_fixture::git(&fx.temp.path().join("app"), &["rev-parse", "main"]);
    assert_eq!(
        main == head,
        expected,
        "{strategy:?} must prove actual base advancement"
    );
    let landed = fx.ctx.store.get(&prerequisite.id).unwrap();
    assert_eq!(landed.status, Status::Done);
    assert_eq!(
        landed
            .landed
            .as_ref()
            .map(|receipt| (receipt.run.as_str(), receipt.sha.as_str())),
        expected.then_some((candidate.current_run().unwrap().id.as_str(), head.as_str())),
    );
    assert_eq!(
        landed.current_run().unwrap().autopilot,
        previous.current_run().unwrap().autopilot,
        "owner approval must not fabricate automatic executor state",
    );
    let now = before.checked_add(SignedDuration::from_secs(1)).unwrap();
    corral_ops::sync_at(&fx.fresh(), now).unwrap();
    let current = fx.ctx.store.get(&dependent.id).unwrap();
    let assessment = attention::board(&fx.fresh(), now)
        .unwrap()
        .tasks
        .into_iter()
        .find(|row| row.task == dependent.id)
        .unwrap()
        .assessment;
    assert_eq!(
        assessment
            .conditions
            .iter()
            .find(|condition| condition.kind == "parked")
            .unwrap()
            .resume[0]
            .satisfied,
        Some(expected),
        "an actual {strategy:?} landing must satisfy landed without accepting KeepBranch",
    );
    assert_eq!(
        current.park.as_ref().unwrap().ready_at,
        expected.then_some(now)
    );
    if expected {
        let ready = assessment
            .conditions
            .iter()
            .find(|condition| condition.kind == "park_resume_ready")
            .unwrap();
        fx.ctx
            .store
            .update(&prerequisite.id, |task| {
                task.status = Status::Review;
                Ok(())
            })
            .unwrap();
        let reopened_at = now.checked_add(SignedDuration::from_secs(1)).unwrap();
        corral_ops::sync_at(&fx.fresh(), reopened_at).unwrap();
        let reopened = fx.ctx.store.get(&dependent.id).unwrap();
        let next = attention::board(&fx.fresh(), reopened_at)
            .unwrap()
            .tasks
            .into_iter()
            .find(|row| row.task == dependent.id)
            .unwrap()
            .assessment
            .conditions
            .into_iter()
            .find(|condition| condition.kind == "park_resume_ready")
            .unwrap();
        assert_eq!(next.key, ready.key);
        assert_eq!(next.since, ready.since);
        assert_eq!(next.resume[0].satisfied, Some(false));
        assert_eq!(reopened.park.as_ref().unwrap().ready_at, Some(now));
        assert_eq!(reopened.updated, current.updated);
    } else {
        assert!(
            !assessment
                .conditions
                .iter()
                .any(|condition| condition.kind == "park_resume_ready")
        );
    }
}

#[test]
fn owner_fast_forward_latches_landed_resume() {
    owner_landing_resume(corral_ops::ApproveStrategy::FastForward, true);
}

#[test]
fn owner_rebase_latches_landed_resume() {
    owner_landing_resume(corral_ops::ApproveStrategy::Rebase, true);
}

#[test]
fn kept_branch_does_not_satisfy_landed_resume() {
    owner_landing_resume(corral_ops::ApproveStrategy::KeepBranch, false);
}

#[test]
fn t128_resume_edge_is_latched_and_stall_ladder_is_paused() {
    // Given a stalled review with an already announced, canonical condition.
    let fx = attention_fixture::Fx::new();
    let task = fx.stalled(None);
    let first = fx
        .ctx
        .store
        .create("Land", None, Status::Review, "")
        .unwrap();
    let second = fx
        .ctx
        .store
        .create("Finish", None, Status::Review, "")
        .unwrap();
    corral_ops::sync_at(&fx.ctx, at(0)).unwrap();
    let before = fx.ctx.store.get(&task.id).unwrap();
    let (stall_key, stall_record) = before
        .attention
        .records
        .iter()
        .find(|(_, record)| record.kind == "unexpected_stall")
        .unwrap();
    park(
        &fx.ctx.store,
        &task.id,
        &[
            &format!("landed:{}", first.id),
            &format!("status:{}=done", second.id),
        ],
    );
    corral_ops::sync_at(&fx.ctx, at(3600)).unwrap();
    let parked = fx.ctx.store.get(&task.id).unwrap();
    let row = attention::board(&fx.ctx, at(3600))
        .unwrap()
        .tasks
        .into_iter()
        .find(|row| row.task == task.id)
        .unwrap();
    assert_eq!(row.assessment.primary.unwrap().kind, "parked");
    assert_eq!(
        parked.attention.records.get(stall_key).unwrap(),
        stall_record
    );
    assert_eq!(parked.park.as_ref().unwrap().ready_at, None);
    let wakes_before = wakes_for(&fx, &task.id);
    // When both canonical prerequisites are satisfied in a subsequent real sync.
    fx.ctx
        .store
        .update(&first.id, |task| {
            task.status = Status::Done;
            task.runs.push(serde_json::from_value(json!({
                "id":"R1","agent":"fixture","worktree":"/missing-park-fixture","branch":"corral/T",
                "base":"main","base_sha":"base","started":at(0),"outcome":"done",
                "autopilot":{"stage":"done","at":at(3601),"sha":"landed"}
            }))?);
            Ok(())
        })
        .unwrap();
    fx.ctx
        .store
        .update(&second.id, |task| {
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();
    corral_ops::sync_at(&fx.reopen(), at(3602)).unwrap();
    // Then one latched condition opens and exactly one additional digest routes to the PM.
    let ready = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(ready.park.as_ref().unwrap().ready_at, Some(at(3602)));
    let condition = attention::board(&fx.ctx, at(3602))
        .unwrap()
        .tasks
        .into_iter()
        .find(|row| row.task == task.id)
        .unwrap()
        .assessment
        .conditions
        .into_iter()
        .find(|condition| condition.kind == "park_resume_ready")
        .unwrap();
    assert!(
        condition
            .resume
            .iter()
            .all(|check| check.satisfied == Some(true))
    );
    assert_eq!(wakes_for(&fx, &task.id), wakes_before + 1);
    let updated = ready.updated;
    fx.ctx
        .store
        .update(&second.id, |task| {
            task.status = Status::Review;
            Ok(())
        })
        .unwrap();
    corral_ops::sync_at(&fx.reopen(), at(3603)).unwrap();
    let reopened = fx.ctx.store.get(&task.id).unwrap();
    let after = attention::board(&fx.ctx, at(3603))
        .unwrap()
        .tasks
        .into_iter()
        .find(|row| row.task == task.id)
        .unwrap()
        .assessment
        .conditions
        .into_iter()
        .find(|condition| condition.kind == "park_resume_ready")
        .unwrap();
    assert_eq!(after.key, condition.key);
    assert_eq!(after.since, condition.since);
    assert_eq!(after.resume[1].satisfied, Some(false));
    assert_eq!(reopened.park.as_ref().unwrap().ready_at, Some(at(3602)));
    assert_eq!(reopened.updated, updated);
    assert_eq!(wakes_for(&fx, &task.id), wakes_before + 1);
}

#[test]
fn unavailable_sync_branches_latch_without_read_only_projection_writes() {
    for branch in ["offline", "inventory-down"] {
        // Given parked work with a satisfied prerequisite and an unavailable inventory.
        let fx = attention_fixture::Fx::new();
        let dependency = fx.ctx.store.create("Done", None, Status::Done, "").unwrap();
        let task = fx.stalled(None);
        park(
            &fx.ctx.store,
            &task.id,
            &[&format!("status:{}=done", dependency.id)],
        );
        let before = fx.ctx.store.get(&task.id).unwrap();
        // When a read-only assessment runs, then it never writes ready_at.
        attention::board(&fx.ctx, at(20)).unwrap();
        assert_eq!(fx.ctx.store.get(&task.id).unwrap(), before);
        std::fs::write(fx.tmp.path().join(branch), "").unwrap();
        corral_ops::sync_at(&fx.reopen(), at(21)).unwrap();
        assert_eq!(
            fx.ctx.store.get(&task.id).unwrap().park.unwrap().ready_at,
            Some(at(21))
        );
    }
}

#[test]
fn parked_ready_tasks_do_not_starve_unrelated_work_in_the_same_grant() {
    // Given admitted coding/planning tasks and an unrelated admitted task.
    let mut fx = operating_fixture::Fx::new();
    let coding = fx.task("Parked code", Status::Ready);
    let planning = fx.task("Parked plan", Status::Ready);
    let unrelated = fx.task("Unrelated", Status::Ready);
    fx.ctx
        .store
        .update(&planning.id, |task| {
            task.plan_required = Some(true);
            Ok(())
        })
        .unwrap();
    fx.grant(&[&coding.id, &planning.id, &unrelated.id]);
    park(&fx.ctx.store, &coding.id, &[]);
    park(&fx.ctx.store, &planning.id, &[]);
    // When real sync drains this grant through the fake transport.
    corral_ops::sync_at(&fx.fresh(), at(20)).unwrap();
    // Then neither parked task starts, while independent work launches in that same pass.
    for id in [&coding.id, &planning.id] {
        let task = fx.ctx.store.get(id).unwrap();
        assert_eq!(task.status, Status::Ready);
        assert!(task.runs.is_empty() && task.plan_runs.is_empty());
    }
    assert_eq!(
        fx.ctx.store.get(&unrelated.id).unwrap().status,
        Status::Running
    );
    assert_eq!(fx.records("submissions.jsonl").len(), 1);
}

#[test]
fn parked_review_starts_no_gate_and_refuses_explicit_owner_entrypoints() {
    for delivery in [
        corral_core::inbox::DeliveryMode::Legacy,
        corral_core::inbox::DeliveryMode::Inbox,
    ] {
        // Given a real completed candidate whose next automatic action would start a gate.
        let mut fx = operating_fixture::Fx::new();
        let task = fx.task("Review", Status::Ready);
        fx.start(&task.id);
        fx.done(&task.id, "change.txt");
        fx.ctx
            .store
            .update(&task.id, |task| {
                task.current_run_mut().unwrap().delivery = delivery;
                Ok(())
            })
            .unwrap();
        fx.grant(&[&task.id]);
        let parked = park(&fx.ctx.store, &task.id, &[]);
        let count = fx.records("submissions.jsonl").len();
        corral_ops::sync_at(&fx.fresh(), at(20)).unwrap();
        assert_eq!(fx.records("submissions.jsonl").len(), count);
        assert!(
            fx.ctx
                .store
                .get(&task.id)
                .unwrap()
                .current_run()
                .unwrap()
                .gate
                .is_none()
        );
        // When each explicit start path is called, then the park refuses before task mutation.
        for kind in ["dispatch", "changes", "retry", "review-run", "plan"] {
            let before = fx.ctx.store.get(&task.id).unwrap();
            let events = std::fs::read(fx.ctx.store.dir().join("events.jsonl")).unwrap();
            let error = match kind {
                "dispatch" => {
                    corral_ops::dispatch(&fx.ctx, &task.id, &DispatchOpts::default()).unwrap_err()
                }
                "changes" => corral_ops::request_changes(&fx.ctx, &task.id, "Fix").unwrap_err(),
                "retry" => corral_ops::retry(&fx.ctx, &task.id, &Default::default()).unwrap_err(),
                "review-run" => corral_ops::review_run(&fx.ctx, &task.id, None).unwrap_err(),
                "plan" => corral_ops::dispatch(
                    &fx.ctx,
                    &task.id,
                    &DispatchOpts {
                        plan: true,
                        ..Default::default()
                    },
                )
                .unwrap_err(),
                _ => unreachable!(),
            };
            let message = error.to_string();
            assert!(
                message.contains(&parked.park.as_ref().unwrap().refusal(&task.id)),
                "{kind}: {error:#}"
            );
            assert_eq!(fx.ctx.store.get(&task.id).unwrap(), before, "{kind}");
            assert_eq!(
                std::fs::read(fx.ctx.store.dir().join("events.jsonl")).unwrap(),
                events,
                "{kind}"
            );
        }
    }
}

#[test]
fn question_and_delivery_conditions_keep_their_ladders_while_parked() {
    use corral_core::inbox::{Inbox, MessageBinding, Post, Recipient, Urgency};
    // Given a Needs me task with an escalated question and an undelivered owner message.
    let fx = attention_fixture::Fx::new();
    let task = fx.questions(1);
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.status = Status::NeedsMe;
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
    let inbox = Inbox::new(
        &fx.ctx.store,
        Recipient::Run {
            task: task.id.clone(),
            run: "R1".into(),
        },
    )
    .unwrap();
    let message = inbox
        .post(Post {
            kind: "owner_reply",
            urgency: Urgency::Wake,
            author: "owner",
            body: "Owner instruction",
            binding: MessageBinding::default(),
            subject: None,
            max_payload: 4096,
        })
        .unwrap();
    inbox
        .mark_terminal(
            message.seq,
            corral_core::inbox::TerminalState::Cancelled,
            "fixture cancellation",
            "test",
        )
        .unwrap();
    corral_ops::sync_at(&fx.ctx, at(0)).unwrap();
    let before = attention::board(&fx.ctx, at(0))
        .unwrap()
        .tasks
        .into_iter()
        .find(|row| row.task == task.id)
        .unwrap()
        .assessment
        .conditions;
    park(&fx.ctx.store, &task.id, &[]);
    // When the independent owner deadline passes under the park.
    corral_ops::sync_at(&fx.reopen(), at(7200)).unwrap();
    let current = fx.ctx.store.get(&task.id).unwrap();
    let after = attention::board(&fx.ctx, at(7200))
        .unwrap()
        .tasks
        .into_iter()
        .find(|row| row.task == task.id)
        .unwrap()
        .assessment
        .conditions;
    // Then every prior key still derives, and both kinds retain their owner level.
    for condition in before.iter().filter(|condition| condition.key.is_some()) {
        assert!(after.iter().any(|other| other.key == condition.key));
    }
    assert!(
        current
            .attention
            .records
            .values()
            .any(|record| record.kind.starts_with("question")
                && record.levels.owner_claimed_at.is_some())
    );
    assert!(
        current
            .attention
            .records
            .values()
            .any(|record| record.kind.starts_with("delivery")
                && record.levels.owner_claimed_at.is_some())
    );
    assert!(fx.calls() > 0);
    assert!(!fx.logical("owner_notified").is_empty());
}

#[test]
fn retained_launch_is_refused_under_the_final_lock_then_submits_after_unpark() {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    // Given exact owner consent and an acknowledged metadata-before-submit barrier.
    let fx = operating_fixture::Fx::new();
    let task = fx.task("Prepared launch", Status::Ready);
    let queued = corral_ops::enqueue(&fx.ctx, &task.id, &DispatchOpts::default()).unwrap();
    let authorization = queued.dispatch_plan.unwrap().authorization.unwrap();
    let path = fx.bin.join("barrier.sock");
    let listener = UnixListener::bind(&path).unwrap();
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| corral_ops::sync_at(&fx.fresh(), at(0)).unwrap());
        let mut descriptors = [PollFd::new(&listener, PollFlags::IN)];
        assert_eq!(
            poll(
                &mut descriptors,
                Some(&Timespec {
                    tv_sec: 60,
                    tv_nsec: 0
                })
            )
            .unwrap(),
            1
        );
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut request = String::new();
        BufReader::new(&stream).read_line(&mut request).unwrap();
        let prepared = fx.ctx.store.get(&task.id).unwrap();
        assert_eq!(prepared.status, Status::Ready);
        assert!(prepared.launch.is_some());
        // When parking wins after admission but before the final locked submission.
        park(&fx.ctx.store, &task.id, &[]);
        stream.write_all(b"R").unwrap();
        worker.join().unwrap();
    });
    drop(listener);
    std::fs::remove_file(path).unwrap();
    // Then the preparation is retained, and no process receives the command.
    assert!(fx.records("submissions.jsonl").is_empty());
    let record = fx.ctx.store.authorized_submission(&authorization).unwrap();
    assert!(
        matches!(&record.disposition, SubmissionDisposition::AuthorizationRefused { reason } if reason.contains("task parked"))
    );
    let before = fx.ctx.store.get(&task.id).unwrap();
    corral_ops::sync_at(&fx.fresh(), at(1)).unwrap();
    assert!(fx.records("submissions.jsonl").is_empty());
    assert_eq!(fx.ctx.store.get(&task.id).unwrap().launch, before.launch);
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.park = None;
            Ok(())
        })
        .unwrap();
    corral_ops::sync_at(&fx.fresh(), at(2)).unwrap();
    assert_eq!(fx.records("submissions.jsonl").len(), 1);
    let after = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(after.runs.len(), before.runs.len());
    assert_eq!(
        after.current_run().unwrap().herdr,
        before.current_run().unwrap().herdr
    );
    assert_eq!(
        fx.ctx
            .store
            .authorized_submission(&authorization)
            .unwrap()
            .disposition,
        SubmissionDisposition::Submitted
    );
}

#[test]
fn running_gate_and_reviewer_record_results_but_start_nothing_until_unparked() {
    use corral_core::{GateResult, Verdict};
    // Given a real candidate with a gate already running in the disposable transport.
    let mut fx = operating_fixture::Fx::new();
    let task = fx.task("In flight", Status::Ready);
    fx.start(&task.id);
    fx.done(&task.id, "in-flight.txt");
    fx.grant(&[&task.id]);
    std::fs::write(fx.bin.join("hold-gates"), "").unwrap();
    corral_ops::sync_at(&fx.fresh(), at(0)).unwrap();
    let running = fx.ctx.store.get(&task.id).unwrap();
    let run = running.current_run().unwrap();
    assert_eq!(run.gate.as_ref().unwrap().result, GateResult::Running);
    let paths = corral_ops::gate_paths(&fx.ctx, &task.id, &run.id);
    park(&fx.ctx.store, &task.id, &[]);
    let count = fx.records("submissions.jsonl").len();
    // When the already-running gate finishes, sync records its real output.
    let result = std::process::Command::new("bash")
        .arg(&paths.script)
        .env("CORRAL_ROOT", fx.temp.path())
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    corral_ops::sync_at(&fx.fresh(), at(20)).unwrap();
    let gated = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(
        gated.current_run().unwrap().gate.as_ref().unwrap().result,
        GateResult::Pass
    );
    assert!(gated.current_run().unwrap().review.is_none());
    assert_eq!(fx.records("submissions.jsonl").len(), count);
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.park = None;
            Ok(())
        })
        .unwrap();
    corral_ops::sync_at(&fx.fresh(), at(21)).unwrap();
    let reviewing = fx.ctx.store.get(&task.id).unwrap();
    assert!(
        reviewing
            .current_run()
            .unwrap()
            .review
            .as_ref()
            .unwrap()
            .verdict
            .is_none()
    );
    park(&fx.ctx.store, &task.id, &[]);
    let count = fx.records("submissions.jsonl").len();
    corral_ops::verdict(
        &fx.ctx,
        &task.id,
        Verdict::Approve,
        "Independent check passed",
    )
    .unwrap();
    corral_ops::sync_at(&fx.fresh(), at(22)).unwrap();
    let reviewed = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(
        reviewed
            .current_run()
            .unwrap()
            .review
            .as_ref()
            .unwrap()
            .verdict,
        Some(Verdict::Approve)
    );
    assert!(reviewed.park.is_some());
    assert_eq!(reviewed.status, Status::Review);
    assert_eq!(fx.records("submissions.jsonl").len(), count);
    // Unpark resumes from those receipts, without launching another gate or reviewer.
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.park = None;
            Ok(())
        })
        .unwrap();
    corral_ops::sync_at(&fx.fresh(), at(23)).unwrap();
    let resumed = fx.ctx.store.get(&task.id).unwrap();
    assert!(
        resumed.status == Status::Done
            || resumed
                .current_run()
                .unwrap()
                .autopilot
                .as_ref()
                .unwrap()
                .stage
                == corral_core::AutoStage::Reviewed
    );
    assert_eq!(fx.records("submissions.jsonl").len(), count);
}

#[test]
fn selected_availability_retry_on_needs_me_stops_at_the_park_fence() {
    // Given a durably selected, quiescent fallback that the existing executor would reconsider.
    let mut fx = operating_fixture::Fx::new();
    let task = fx.task("Fallback", Status::Ready);
    fx.grant(&[&task.id]);
    fx.start(&task.id);
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.status = Status::NeedsMe;
            task.current_run_mut().unwrap().herdr = None;
            Ok(())
        })
        .unwrap();
    park(&fx.ctx.store, &task.id, &[]);
    let episode = format!("{}-R1-implementation", task.id);
    let path = fx.ctx.store.state_dir();
    for directory in ["availability", "provider-stall"] {
        std::fs::create_dir_all(path.join(directory)).unwrap();
    }
    let attempts = [
        json!({"candidate":"old","provider":"fixture","connection":"fixture","model":"old",
            "run":"R1","selected_at":at(0),"reason":"preferred","failures":[]}),
        json!({"candidate":"next","provider":"fixture","connection":"fixture","model":"next",
            "run":null,"selected_at":at(1),"reason":"selected fallback","failures":[]}),
    ];
    std::fs::write(
        path.join("availability").join(format!("{episode}.json")),
        serde_json::to_vec(
            &json!({"version":1,"id":episode,"task":task.id,"role":"implementation",
            "profile":"fixture","revision":"1","started":at(0),"attempts":attempts}),
        )
        .unwrap(),
    )
    .unwrap();
    let stalled = path
        .join("provider-stall")
        .join(format!("{}-R1.json", task.id));
    std::fs::write(&stalled, serde_json::to_vec(&json!({
        "since":at(0),"error_at":at(0),"message":"model_not_found","attempts":0,
        "last_attempt":null,"escalated":true,"class":{"kind":"unavailable"},"fallback_selected":true
    })).unwrap()).unwrap();
    // When real sync reaches the selected-fallback executor after inventory processing.
    let submissions = fx.records("submissions.jsonl").len();
    corral_ops::sync_at(&fx.fresh(), at(20)).unwrap();
    // Then it records the concrete park refusal instead of retrying, checkpointing or launching.
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(stalled).unwrap()).unwrap();
    assert!(
        state["fallback_blocked"]
            .as_str()
            .unwrap()
            .contains("task parked")
    );
    let current = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(current.runs.len(), 1);
    assert_eq!(current.status, Status::NeedsMe);
    assert!(current.park.is_some());
    assert!(current.launch.is_none());
    assert_eq!(fx.records("submissions.jsonl").len(), submissions);
    assert!(
        !fx.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .any(|event| event.kind == "availability_switched")
    );
}
