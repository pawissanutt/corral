//! cowork-landing (T-544 C9, `review required` at finish and finish into an existing task)
//! through the ops API, against a fake terminal server whose panes run gates synchronously.
#![cfg(unix)] // Fixtures execute the symlinked POSIX fake terminal server.

use std::path::Path;

#[path = "assistant_cowork/fixture.rs"]
mod fixture;
mod support;
use fixture::Fx;
#[path = "assistant_cowork/triage.rs"]
mod triage;

use corral_core::{
    Accept, Cowork, CoworkReview, Origin, Reviewer, Run, RunOutcome, Status, Verdict,
};
use corral_ops::assistant::scratch::{self, Finish};
use corral_ops::{ApproveStrategy, Ctx, DispatchOpts, DispatchOutcome};
use serde_json::json;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = support::command("git", dir)
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
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn kinds(ctx: &Ctx, task: &str) -> Vec<String> {
    let events = ctx.store.events_shared().unwrap();
    let mine = events.iter().filter(|e| e.task.as_deref() == Some(task));
    mine.map(|event| event.kind.clone()).collect()
}

#[test]
fn terminal_cowork_with_missing_directory_can_be_discarded() {
    let fx = Fx::new(None);
    let made = fx.scratch("released");
    let head = fx.commit(&made.path, "work.txt");
    let finished = scratch::finish(
        &fx.ctx,
        "A-1",
        "released",
        &Finish {
            title: Some("Released work"),
            ..Finish::default()
        },
    )
    .unwrap();
    std::fs::remove_dir_all(&made.path).unwrap();
    // Actual task-discard bookkeeping must retain a missing-directory record while the
    // requested retained branch still holds unique work, rather than orphaning that branch.
    corral_ops::discard_with_options(&fx.ctx, &finished.task, true).unwrap();
    assert_eq!(fx.kind("released").as_deref(), Some(scratch::COWORK));
    assert_eq!(git(&fx.app, &["rev-parse", &made.branch]), head);
    let preview = scratch::gc(&fx.ctx, jiff::Timestamp::now(), true).unwrap();
    assert_eq!(preview.items[0].action, "remove", "{preview:?}");
    // A subsequent ordinary task discard must bundle those commits before branch deletion.
    corral_ops::discard(&fx.ctx, &finished.task).unwrap();
    assert!(
        git(
            &fx.app,
            &[
                "for-each-ref",
                "--format=%(objectname)",
                &format!("refs/heads/{}", made.branch)
            ]
        )
        .is_empty()
    );
    let session = fx.ctx.store.get_assistant("A-1").unwrap();
    let bundle = session
        .artifacts
        .iter()
        .find(|a| a.path.ends_with("commits.bundle"))
        .expect("task discard must preserve unique co-work commits before deleting the branch");
    assert_eq!(bundle.tier_override, Some(corral_core::ArtifactTier::Keep));
    let saved = fx.app.join("saved.bundle");
    std::fs::write(&saved, corral_ops::artifact_read(&fx.ctx, bundle).unwrap()).unwrap();
    git(&fx.app, &["bundle", "verify", saved.to_str().unwrap()]);
    git(
        &fx.app,
        &[
            "fetch",
            "-q",
            saved.to_str().unwrap(),
            &format!("refs/heads/{}:refs/heads/restored", made.branch),
        ],
    );
    assert_eq!(git(&fx.app, &["rev-parse", "restored"]), head);
    assert_eq!(git(&fx.app, &["show", "restored:work.txt"]), "work.txt");
    assert!(
        fx.ctx
            .store
            .get_assistant("A-1")
            .unwrap()
            .scratch
            .is_empty()
    );
}

#[test]
fn owner_finish_materializes_gated_reviewed_work_and_autopilot_lands_it() {
    // Given A-1's scratch cache-fix with one commit, while main moved on, in a project whose
    // policy names reviewer rev; A-1's transcript has usage before, during and after the work.
    let fx = Fx::new(Some("rev"));
    let made = fx.scratch("cache-fix");
    fx.commit(&made.path, "cache.txt");
    let moved = fx.commit(&fx.app, "main.txt");
    fx.usage_around(made.created);
    // A missing classification previously missed every reviewer routing rule.
    let mut fx = fx;
    fx.ctx.store.config.review.reviewer = None;
    fx.ctx.store.config.limits.capacity = Some(corral_core::CapacityLimits {
        unfinished: 1,
        agents: 1,
        gates: 1,
        adhoc_reserve: 0,
    });
    fx.ctx.store.config.review.accept = Some(Accept::Owner);
    fx.ctx.store.config.routing.reviewer_rules.push(
        serde_json::from_value(json!({
            "kind": "backend", "size": "m", "profile": "rev"
        }))
        .unwrap(),
    );
    fx.ctx.store.save_config().unwrap();
    // When the owner finishes it as a fix for the cache, review is mandatory.
    let opts = Finish {
        title: Some("Fix the cache"),
        adhoc: true,
        ..Finish::default()
    };
    let finished = scratch::finish(&fx.ctx, "A-1", "cache-fix", &opts).unwrap();
    // Then a Review task exists with origin assistant, label cowork and run R1 on the scratch.
    assert_eq!(
        (finished.gate.as_str(), finished.status, &finished.reviewer),
        ("queued", Status::Review, &None)
    );
    let task = fx.ctx.store.get(&finished.task).unwrap();
    assert_eq!(task.title, "Fix the cache");
    assert_eq!(task.capacity_lane, corral_core::CapacityLane::Adhoc);
    assert_eq!(task.capacity_started, Some(true));
    assert_eq!(task.origin, Some(Origin::Assistant));
    assert_eq!(task.labels, ["cowork"]);
    assert_eq!(task.kind.as_deref(), Some("backend"));
    assert_eq!(task.size, Some(corral_core::TaskSize::M));
    let cowork = Cowork {
        completion: None,
        session: "A-1".into(),
        scratch: "cache-fix".into(),
        run: "R1".into(),
        review: CoworkReview::CompletionV1,
        owner_confirmed: true,
        review_waiver: None,
    };
    assert_eq!(
        serde_json::to_value(task.cowork.as_ref().unwrap()).unwrap()["owner_confirmed"],
        true
    );
    assert_eq!(task.cowork.as_ref().unwrap().review, cowork.review);
    let run = task.current_run().unwrap();
    assert_eq!((run.id.as_str(), &run.worktree), ("R1", &made.path));
    assert_eq!(
        (run.branch.as_str(), run.base.as_str()),
        (made.branch.as_str(), "main")
    );
    assert_eq!(run.outcome, Some(RunOutcome::Done));
    assert!(run.herdr.is_none());
    assert!(
        run.gate.is_none(),
        "submission must not wait for or launch a gate"
    );
    assert!(run.review.is_none());
    // The scratch was rebased onto the moved main before the gate.
    assert_eq!(git(&made.path, &["rev-parse", "HEAD^"]), moved);
    // Only the usage between scratch creation and finish is on the run.
    let usage = run.usage.as_ref().unwrap_or_else(|| {
        panic!(
            "A-1's usage on the run: note={:?}, started={}",
            run.usage_note, run.started
        )
    });
    assert_eq!((usage.input, usage.entries.len()), (10, 1));
    // Confirmed finish selects the independent reviewer and delegates acceptance for this run.
    let policy = fx.ctx.store.config.task_review_policy(&task);
    assert_eq!(
        (policy.reviewer, policy.accept),
        (Reviewer::parse("rev"), Accept::Auto)
    );
    // The scratch now belongs to the task: kind cowork, and the session cannot discard it.
    assert_eq!(fx.kind("cache-fix").as_deref(), Some("cowork"));
    let held = scratch::discard(&fx.ctx, "A-1", "cache-fix").unwrap_err();
    assert!(format!("{held:#}").contains("scratch_attached"), "{held:#}");

    // The explicit finish authorizes this pipeline in Assist mode without a workspace grant.
    assert_eq!(
        fx.ctx.store.config.automation.mode,
        corral_core::OperatingMode::Assist
    );
    fx.ctx.store.config.automation.paused = true;
    fx.ctx.store.save_config().unwrap();
    corral_ops::sync(&fx.ctx).unwrap();
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
    fx.ctx.store.config.automation.paused = false;
    fx.ctx.store.save_config().unwrap();
    corral_ops::sync(&fx.ctx).unwrap();
    corral_ops::sync(&fx.ctx).unwrap();
    let waiting = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(waiting.status, Status::Review);
    assert_eq!(waiting.current_run().unwrap().review_runs.len(), 1);
    assert_eq!(
        kinds(&fx.ctx, &task.id)
            .iter()
            .filter(|kind| *kind == "gate_started")
            .count(),
        1
    );
    // A committed driver transfer while review runs retains that exact attempt. Board
    // continuation needs normal policy, not the original assistant's Ship flag or host.
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.cowork
                .as_mut()
                .unwrap()
                .completion
                .as_mut()
                .unwrap()
                .driver = corral_core::CompletionDriver::Board;
            Ok(())
        })
        .unwrap();
    fx.ctx
        .store
        .update_assistant("A-1", |session| {
            session.ended = Some(jiff::Timestamp::now());
            Ok(())
        })
        .unwrap();
    corral_ops::sync(&fx.ctx).unwrap();
    assert_eq!(
        fx.ctx
            .store
            .get(&task.id)
            .unwrap()
            .current_run()
            .unwrap()
            .review_runs
            .len(),
        1
    );
    assert_eq!(
        kinds(&fx.ctx, &task.id)
            .iter()
            .filter(|kind| *kind == "gate_started")
            .count(),
        1
    );
    fx.delegate_board(&task.id);
    corral_ops::verdict(
        &fx.ctx,
        &task.id,
        Verdict::Approve,
        "reviewed independently",
    )
    .unwrap();
    corral_ops::sync(&fx.ctx).unwrap();
    let task = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(task.status, Status::Done);
    assert!(
        !task
            .attention
            .records
            .values()
            .any(|record| record.kind == "owner_accept")
    );
    assert_eq!(
        task.landed.as_ref().map(|l| l.sha.as_str()),
        Some(finished.head.as_str())
    );
    assert_eq!(git(&fx.app, &["rev-parse", "main"]), finished.head);
    assert!(!made.path.exists());
    // Then the session lists the landed task and commits and forgets the scratch.
    let session = fx.ctx.store.get_assistant("A-1").unwrap();
    assert!(session.scratch.is_empty());
    let landed = &session.landed[0];
    assert_eq!(
        (landed.task.as_str(), landed.run.as_str()),
        (task.id.as_str(), "R1")
    );
    assert_eq!(
        (landed.sha.as_str(), landed.base.as_str()),
        (finished.head.as_str(), "main")
    );
    assert_eq!(landed.commits, std::slice::from_ref(&finished.head));
    let seen = kinds(&fx.ctx, &task.id);
    for kind in [
        "created",
        "cowork_finished",
        "gate",
        "approved",
        "cowork_landed",
    ] {
        assert!(
            seen.iter().any(|k| k == kind),
            "{kind} missing from {seen:?}"
        );
    }
    // And stats count the finish, its review choice and the landing.
    let role = corral_ops::assistant_role_stats(&fx.ctx.store, None).unwrap();
    let counts = (
        role.cowork.finished,
        role.cowork.review_none,
        role.cowork.landed,
    );
    assert_eq!(counts, (1, 0, 1));
    assert_eq!(role.sessions[0].landed, 1);
}

#[test]
fn finish_usage_window_uses_the_same_ordered_clock_as_scratch_creation() {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            corral_core::clock::pin_thread(None);
        }
    }
    let fx = Fx::new(None);
    // Model a process clock ahead of the raw wall clock after a backward wall step.
    let ordered = jiff::Timestamp::now()
        .checked_add(jiff::SignedDuration::from_hours(1))
        .unwrap();
    corral_core::clock::pin_thread(Some(ordered));
    let _reset = Reset;
    let made = fx.scratch("ordered-clock");
    fx.commit(&made.path, "work.txt");
    fx.usage_around(made.created);
    let submitted = scratch::finish(
        &fx.ctx,
        "A-1",
        "ordered-clock",
        &Finish {
            title: Some("Ordered completion window"),
            ..Default::default()
        },
    )
    .unwrap();
    let task = fx.ctx.store.get(&submitted.task).unwrap();
    let run = task.current_run().unwrap();
    assert!(
        run.ended.unwrap() >= run.started,
        "completion must not end before scratch creation"
    );
    assert_eq!(run.usage.as_ref().map(|usage| usage.input), Some(10));
}

#[test]
fn finish_records_note_when_zero_usage_is_followed_by_partial_response() {
    let fx = Fx::new(None);
    let made = fx.scratch("partial");
    fx.commit(&made.path, "partial.txt");
    fx.usage_around(made.created);
    let transcript = fx.ctx.store.dir().join("A-1-session.jsonl");
    // A completed zero-token assistant entry is followed by an unfinished response. The old
    // collector silently dropped the tail and finish persisted a zero-input Usage without a note.
    std::fs::write(&transcript, format!(
        "{{\"type\":\"message\",\"id\":\"zero\",\"timestamp\":\"{}\",\"message\":{{\"role\":\"assistant\",\"usage\":{{\"input\":0,\"output\":0}}}}}}\n{{\"type\":\"message\",\"id\":\"partial\"",
        made.created
    )).unwrap();
    let finished = scratch::finish(
        &fx.ctx,
        "A-1",
        "partial",
        &Finish {
            title: Some("Partial transcript"),
            ..Finish::default()
        },
    )
    .unwrap();
    let task = fx.ctx.store.get(&finished.task).unwrap();
    let run = task.current_run().unwrap();
    assert!(
        run.usage.is_none(),
        "partial snapshot must not be recorded as complete: {:?}",
        run.usage
    );
    assert!(
        run.usage_note
            .as_deref()
            .unwrap_or_default()
            .contains("incomplete trailing line"),
        "{:?}",
        run.usage_note
    );
}

#[test]
fn check_only_runs_normal_verification_and_never_lands() {
    // Given A-1's scratch with a commit in a project reviewed by rev.
    let mut fx = Fx::new(Some("rev"));
    let made = fx.scratch("spinner");
    fx.commit(&made.path, "spin.txt");
    // When the owner finishes it and asks for review.
    let opts = Finish {
        title: Some("Spin the widget"),
        review: Some(CoworkReview::Required),
        intent: Some(corral_core::CompletionIntent::Verify),
        ..Finish::default()
    };
    let finished = scratch::finish(&fx.ctx, "A-1", "spinner", &opts).unwrap();
    // Submission returns before checks; ordinary sync starts the gate and independent review.
    assert_eq!(finished.gate, "queued");
    assert!(finished.reviewer.is_none());
    corral_ops::sync(&fx.ctx).unwrap();
    let task = fx.ctx.store.get(&finished.task).unwrap();
    let run = task.current_run().unwrap();
    assert_eq!(
        run.review.as_ref().map(|r| r.reviewer.as_str()),
        Some("rev")
    );
    assert_eq!(run.review_runs.len(), 1);
    let policy = fx.ctx.store.config.task_review_policy(&task);
    assert_eq!(policy.reviewer, Reviewer::parse("rev"));
    assert_eq!(policy.accept, Accept::Owner);
    corral_ops::verdict(&fx.ctx, &task.id, Verdict::Approve, "checked independently").unwrap();
    corral_ops::sync(&fx.ctx).unwrap();
    assert_eq!(fx.ctx.store.get(&task.id).unwrap().status, Status::Review);
    let refused = approve(&fx, &task.id).unwrap_err();
    assert!(refused.to_string().contains("check_only"), "{refused:#}");
    assert!(made.path.exists());
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.cowork
                .as_mut()
                .unwrap()
                .completion
                .as_mut()
                .unwrap()
                .driver = corral_core::CompletionDriver::Board;
            Ok(())
        })
        .unwrap();
    fx.delegate_board(&task.id);
    let checks = kinds(&fx.ctx, &task.id)
        .iter()
        .filter(|kind| *kind == "gate_started")
        .count();
    fx.commit(&fx.app, "moved-base.txt");
    corral_ops::sync(&fx.ctx).unwrap();
    assert_eq!(fx.ctx.store.get(&task.id).unwrap().status, Status::Review);
    assert_eq!(
        kinds(&fx.ctx, &task.id)
            .iter()
            .filter(|kind| *kind == "gate_started")
            .count(),
        checks,
        "base movement/board authority cannot turn Verify into more automatic checks or landing"
    );
    assert!(!fx.app.join("spin.txt").exists());
}

#[test]
fn stale_choice_and_unsubmitted_changes_cannot_replace_the_queued_candidate() {
    // Given A-1's scratch whose candidate the owner was shown, then one more commit on it.
    let fx = Fx::new(Some("rev"));
    let made = fx.scratch("cache-fix");
    let shown = fx.commit(&made.path, "cache.txt");
    let later = fx.commit(&made.path, "more.txt");
    let review = |head| Finish {
        title: Some("Fix the cache"),
        review: Some(CoworkReview::Required),
        expected_head: Some(head),
        ..Finish::default()
    };
    // When the owner's answer finishes the candidate they saw, it refuses and changes nothing;
    // so does a value that is no commit SHA.
    let stale = scratch::finish(&fx.ctx, "A-1", "cache-fix", &review(&shown[..7])).unwrap_err();
    assert!(
        format!("{stale:#}").contains("stale_candidate"),
        "{stale:#}"
    );
    let bad = scratch::finish(&fx.ctx, "A-1", "cache-fix", &review("-v")).unwrap_err();
    assert!(
        format!("{bad:#}").contains("--expected-head takes"),
        "{bad:#}"
    );
    assert!(fx.ctx.store.list().unwrap().is_empty());
    assert_eq!(fx.kind("cache-fix").as_deref(), Some("spike"));
    assert_eq!(git(&made.path, &["rev-parse", "HEAD"]), later);
    let events = fx.ctx.store.events_shared().unwrap();
    assert!(!events.iter().any(|e| e.kind == "cowork_finished"));
    // When the owner was shown the current candidate and asks for independent review.
    let finished = scratch::finish(&fx.ctx, "A-1", "cache-fix", &review(&later[..12])).unwrap();
    // Submission pins that commit; a later ordinary commit must not silently replace it.
    assert_eq!(
        (finished.head.as_str(), finished.gate.as_str()),
        (later.as_str(), "queued")
    );
    assert!(finished.reviewer.is_none());
    let unsubmitted = fx.commit(&made.path, "unsubmitted.txt");
    corral_ops::sync(&fx.ctx).unwrap();
    let task = fx.ctx.store.get(&finished.task).unwrap();
    assert_eq!(task.runs.len(), 1);
    assert_eq!(task.status, Status::NeedsMe);
    let run = task.current_run().unwrap();
    assert!(run.gate.is_none());
    assert!(run.review_runs.is_empty());
    assert!(
        run.autopilot
            .as_ref()
            .unwrap()
            .reason
            .as_deref()
            .unwrap()
            .contains("submitted candidate changed")
    );
    let events = fx.ctx.store.events_shared().unwrap();
    let event = events.iter().find(|e| e.kind == "cowork_finished").unwrap();
    assert_eq!(event.detail["candidate"], later.as_str());
    let refreshed = scratch::finish(
        &fx.ctx,
        "A-1",
        "cache-fix",
        &Finish {
            into: Some(&task.id),
            expected_head: Some(&unsubmitted),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        (
            refreshed.task.as_str(),
            refreshed.run.as_str(),
            refreshed.gate.as_str()
        ),
        (task.id.as_str(), "R2", "queued")
    );
    assert_eq!(fx.ctx.store.get(&task.id).unwrap().rounds, 0);
}

fn approve(fx: &Fx, id: &str) -> anyhow::Result<String> {
    corral_ops::approve(&fx.ctx, id, ApproveStrategy::FastForward, true, false)
}

#[test]
fn review_required_binds_approval_when_no_reviewer_resolves() {
    // Given a project whose review policy resolves no reviewer, and A-1's scratch with a commit.
    let fx = Fx::new(None);
    let made = fx.scratch("cache-fix");
    fx.commit(&made.path, "cache.txt");
    // When the owner finishes it with review required.
    let opts = Finish {
        title: Some("Fix the cache"),
        review: Some(CoworkReview::Required),
        ..Finish::default()
    };
    let finished = scratch::finish(&fx.ctx, "A-1", "cache-fix", &opts).unwrap();
    let id = finished.task.clone();
    // Then the finish names how to start a review, and approval refuses until one approves.
    assert!(finished.reviewer.is_none());
    corral_ops::sync(&fx.ctx).unwrap();
    let blocked = fx.ctx.store.get(&id).unwrap();
    let note = blocked
        .current_run()
        .unwrap()
        .autopilot
        .as_ref()
        .unwrap()
        .reason
        .as_deref()
        .unwrap();
    assert!(note.contains("independent reviewer"), "{note}");
    corral_ops::gate_start(&fx.ctx, &id).unwrap();
    corral_ops::gate_wait(&fx.ctx, &id).unwrap();
    let main = git(&fx.app, &["rev-parse", "main"]);
    let refused = approve(&fx, &id).unwrap_err();
    assert!(
        format!("{refused:#}").contains("review_required"),
        "{refused:#}"
    );
    assert_eq!(fx.ctx.store.get(&id).unwrap().status, Status::Review);
    assert_eq!(git(&fx.app, &["rev-parse", "main"]), main);
    // When a named reviewer reviews the candidate and approves it, the task lands.
    corral_ops::review_run(&fx.ctx, &id, Some("rev")).unwrap();
    corral_ops::verdict(&fx.ctx, &id, Verdict::Approve, "reviewed").unwrap();
    approve(&fx, &id).unwrap();
    assert_eq!(fx.ctx.store.get(&id).unwrap().status, Status::Done);
}

#[test]
fn review_required_into_a_task_whose_reviewer_is_none_cannot_land_unreviewed() {
    // Given a project reviewed by rev, and T-001 overriding its own reviewer to none.
    let fx = Fx::new(Some("rev"));
    let task = (fx.ctx.store)
        .create("Cache layer", Some("app"), Status::Backlog, "")
        .unwrap();
    let none = |t: &mut corral_core::Task| {
        t.reviewer = Some("none".into());
        Ok(())
    };
    fx.ctx.store.update(&task.id, none).unwrap();
    let made = fx.scratch("cache-fix");
    fx.commit(&made.path, "cache.txt");
    // When the owner finishes the scratch into T-001 with review required.
    let opts = Finish {
        into: Some(&task.id),
        review: Some(CoworkReview::Required),
        ..Finish::default()
    };
    let finished = scratch::finish(&fx.ctx, "A-1", "cache-fix", &opts).unwrap();
    // Then no review starts on its own, and approval refuses without one.
    assert!(finished.reviewer.is_none());
    corral_ops::sync(&fx.ctx).unwrap();
    corral_ops::gate_start(&fx.ctx, &task.id).unwrap();
    corral_ops::gate_wait(&fx.ctx, &task.id).unwrap();
    let refused = approve(&fx, &task.id).unwrap_err();
    assert!(
        format!("{refused:#}").contains("review_required"),
        "{refused:#}"
    );
    assert_eq!(fx.ctx.store.get(&task.id).unwrap().status, Status::Review);
}

#[test]
fn reviewer_none_applies_only_to_the_cowork_run() {
    // Given a project whose policy names reviewer rev and accepts automatically.
    let mut fx = Fx::new(Some("rev"));
    fx.ctx.store.config.review.accept = Some(Accept::Auto);
    let task = fx
        .ctx
        .store
        .create("Plain", Some("app"), Status::Review, "")
        .unwrap();
    let run = |id: &str| -> Run {
        serde_json::from_value(json!({"id": id, "agent": "fake", "worktree": "/tmp/w",
            "branch": "b", "base": "main", "base_sha": "0", "started": task.created}))
        .unwrap()
    };
    let policy = |task: &corral_core::Task| {
        let policy = fx.ctx.store.config.task_review_policy(task);
        (policy.reviewer, policy.accept)
    };
    let rev = Reviewer::parse("rev");
    // An ordinary task keeps the project's reviewer and acceptance.
    let mut plain = task.clone();
    plain.runs.push(run("R1"));
    assert_eq!(policy(&plain), (rev.clone(), Accept::Auto));
    // A co-work run with review none resolves no reviewer, landing by the owner only.
    let mut cowork = plain.clone();
    cowork.runs.push(run("R2"));
    cowork.cowork = Some(Cowork {
        completion: None,
        session: "A-1".into(),
        scratch: "s".into(),
        run: "R2".into(),
        review: CoworkReview::None,
        owner_confirmed: false,
        review_waiver: None,
    });
    assert_eq!(policy(&cowork), (Reviewer::None, Accept::Owner));
    // A later ordinary run of the same task is reviewed and accepted as the project says.
    let mut later = cowork.clone();
    later.runs.push(run("R3"));
    assert_eq!(policy(&later), (rev, Accept::Auto));
}

#[test]
fn finish_into_an_existing_task_adds_a_run_and_refuses_task_busy_while_one_is_live() {
    // Given T-001 dispatched with the fake agent, so its run R1 is live, and A-1's scratch.
    let fx = Fx::new(None);
    let created = fx
        .ctx
        .store
        .create("Cache layer", Some("app"), Status::Ready, "")
        .unwrap();
    let opts = DispatchOpts {
        profile: Some("fake".into()),
        ..DispatchOpts::default()
    };
    let started = corral_ops::dispatch(&fx.ctx, &created.id, &opts).unwrap();
    assert!(matches!(started, DispatchOutcome::Started { .. }));
    let made = fx.scratch("cache-fix");
    fx.commit(&made.path, "cache.txt");
    let into = Finish {
        into: Some(&created.id),
        ..Finish::default()
    };
    // When the owner finishes the scratch into T-001 while R1 is live, it refuses task_busy.
    let before = fx.ctx.store.get(&created.id).unwrap();
    let busy = scratch::finish(&fx.ctx, "A-1", "cache-fix", &into).unwrap_err();
    assert!(format!("{busy:#}").contains("task_busy"), "{busy:#}");
    assert_eq!(fx.ctx.store.get(&created.id).unwrap(), before);
    assert_eq!(fx.kind("cache-fix").as_deref(), Some("spike"));
    // When R1 has ended, the same finish adds run R2 on the scratch to T-001.
    fx.ctx
        .store
        .update(&created.id, |task| {
            let run = task.current_run_mut().unwrap();
            run.outcome = Some(RunOutcome::Done);
            run.ended = Some(jiff::Timestamp::now());
            task.status = Status::Review;
            Ok(())
        })
        .unwrap();
    let finished = scratch::finish(&fx.ctx, "A-1", "cache-fix", &into).unwrap();
    assert_eq!(
        (finished.task.as_str(), finished.run.as_str()),
        (created.id.as_str(), "R2")
    );
    let task = fx.ctx.store.get(&created.id).unwrap();
    assert_eq!(
        (task.title.as_str(), task.runs.len(), task.status),
        ("Cache layer", 2, Status::Review)
    );
    assert_eq!(task.runs[1].branch, made.branch);
    assert_eq!(task.cowork.as_ref().map(|c| c.run.as_str()), Some("R2"));
    assert!(task.labels.iter().any(|label| label == "cowork"));
    assert_eq!(fx.kind("cache-fix").as_deref(), Some("cowork"));
    // A repair sent back to this assistant reuses its attached scratch, never a worker run.
    fx.ctx
        .store
        .update(&created.id, |task| {
            task.status = Status::NeedsMe;
            task.current_run_mut().unwrap().autopilot = Some(corral_core::AutopilotState {
                stage: corral_core::AutoStage::Changes,
                reason: Some("repair in A-1".into()),
                sha: None,
                at: jiff::Timestamp::now(),
            });
            Ok(())
        })
        .unwrap();
    fx.commit(&made.path, "repaired.txt");
    corral_ops::gate_start(&fx.ctx, &created.id).unwrap();
    let checking = fx.ctx.store.get(&created.id).unwrap();
    let busy = scratch::finish(&fx.ctx, "A-1", "cache-fix", &into).unwrap_err();
    assert!(busy.to_string().contains("task_busy"), "{busy:#}");
    assert_eq!(fx.ctx.store.get(&created.id).unwrap(), checking);
    corral_ops::gate_collect(&fx.ctx, &created.id)
        .unwrap()
        .unwrap();
    let repaired = scratch::finish(&fx.ctx, "A-1", "cache-fix", &into).unwrap();
    assert_eq!(repaired.run, "R3");
    assert_eq!(
        fx.ctx
            .store
            .get(&created.id)
            .unwrap()
            .current_run()
            .unwrap()
            .worktree,
        made.path
    );
}

#[test]
fn finish_refuses_empty_dirty_and_finished_scratch_without_filing() {
    let fx = Fx::new(None);
    let opts = Finish {
        title: Some("Nothing yet"),
        ..Finish::default()
    };
    // An empty scratch and one with uncommitted work file nothing.
    fx.scratch("empty");
    let empty = scratch::finish(&fx.ctx, "A-1", "empty", &opts).unwrap_err();
    assert!(format!("{empty:#}").contains("scratch_empty"), "{empty:#}");
    let dirty = fx.scratch("dirty");
    fx.commit(&dirty.path, "kept.txt");
    std::fs::write(dirty.path.join("loose.txt"), "loose\n").unwrap();
    let refused = scratch::finish(&fx.ctx, "A-1", "dirty", &opts).unwrap_err();
    assert!(
        format!("{refused:#}").contains("scratch_dirty"),
        "{refused:#}"
    );
    assert!(fx.ctx.store.list().unwrap().is_empty());
    // A scratch finishes once; a second finish names the task that holds it.
    std::fs::remove_file(dirty.path.join("loose.txt")).unwrap();
    let finished = scratch::finish(&fx.ctx, "A-1", "dirty", &opts).unwrap();
    let again = scratch::finish(&fx.ctx, "A-1", "dirty", &opts).unwrap_err();
    assert!(
        format!("{again:#}").contains("scratch_attached"),
        "{again:#}"
    );
    assert_eq!(fx.ctx.store.list().unwrap().len(), 1);
    assert_eq!(finished.task, fx.ctx.store.list().unwrap()[0].id);
    // Discarding the co-work task removes its worktree, and the session forgets the scratch.
    corral_ops::discard(&fx.ctx, &finished.task).unwrap();
    assert!(!dirty.path.exists());
    assert_eq!(fx.kind("dirty"), None);
}
