use super::*;
use crate::agent::{BindTarget, test_support as ts};
use corral_core::connection::ConnectionBinding;
use corral_core::{
    AvailabilityFallback, CandidateCost, ExecutionCandidate, ProfileEligibility, ProfileMetadata,
    ProfileRole, SelectionGuidance, SessionAction, SpendingBound,
};
use serde_json::json;
use std::collections::BTreeSet;

struct Review {
    fx: ts::Fx,
    id: String,
    host: ts::Host,
    api: ResolvedProfile,
    sub: ConnectionBinding,
}

fn git(path: &std::path::Path, args: &[&str]) -> String {
    let out = crate::test_support::command("git", path)
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap().trim().into()
}

fn review() -> Review {
    let mut fx = ts::fx();
    let candidate = |id: &str, connection: &str, cost| ExecutionCandidate {
        id: id.into(),
        harness: "corral-pi".into(),
        connection: connection.into(),
        provider: "openai".into(),
        model: "same-model".into(),
        effort: None,
        args: vec![],
        evidence: "synthetic-approval".into(),
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
    let metadata = ProfileMetadata {
        display_name: "fixture".into(),
        purpose: "fixture".into(),
        guidance: SelectionGuidance::default(),
        instructions: "fixture".into(),
        eligibility: ProfileEligibility {
            enabled: true,
            automatic: true,
            roles: BTreeSet::from([ProfileRole::ImplementationReview]),
            projects: BTreeSet::new(),
        },
        revision: "fixture".into(),
        evidence: Some("synthetic".into()),
        candidates: vec![sub.clone(), api.clone()],
        fallback: Some(AvailabilityFallback {
            automatic: true,
            paid_roles: BTreeSet::from([AgentRole::Reviewer]),
            approved_providers: BTreeSet::from(["openai".into()]),
            max_switches: 1,
            allow_unknown_subscription_cost: true,
            spending: Some(SpendingBound {
                currency: "USD".into(),
                maximum_micros: 1,
            }),
            approval: "synthetic-approval".into(),
        }),
    };
    fx.ctx.store.config.profiles.insert("pi".into(), serde_json::from_value(json!({"harness":"corral-pi","vendor":"openai","model":"same-model","metadata":metadata})).unwrap());
    fx.ctx.store.config.review =
        serde_json::from_value(json!({"require_cross_family":true,"reviewer_session":"reuse"}))
            .unwrap();
    fx.ctx.store.save_config().unwrap();
    let resolved_sub = fx.ctx.store.config.resolve_candidate("pi", &sub).unwrap();
    let sub = resolved_sub.connection_binding().unwrap().unwrap();
    let api = fx.ctx.store.config.resolve_candidate("pi", &api).unwrap();
    let dir = fx._tmp.path().join("candidate");
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-qb", "main"]);
    git(&dir, &["config", "user.name", "fixture"]);
    git(&dir, &["config", "user.email", "fixture@example.invalid"]);
    git(&dir, &["commit", "--allow-empty", "-qm", "initial"]);
    let sha = git(&dir, &["rev-parse", "HEAD"]);
    let (id, target) = ts::reviewer_task(&fx);
    fx.ctx
        .store
        .update(&id, |task| {
            let record: corral_core::ReviewRecord = serde_json::from_value(
                json!({"sha":sha,"reviewer":"pi","at":"2026-10-23T00:00:00Z"}),
            )?;
            let parent = &mut task.runs[0];
            parent.vendor = Some("anthropic".into());
            parent.worktree = dir.clone();
            parent.herdr.as_mut().unwrap().socket = Some(fx.ctx.herdr.socket_path()?);
            parent.review = Some(record.clone());
            let child = &mut parent.review_runs[0];
            child.worktree = dir.clone();
            child.herdr.as_mut().unwrap().socket = Some(fx.ctx.herdr.socket_path()?);
            child.harness = Some("corral-pi".into());
            child.vendor = Some("openai".into());
            child.model = Some("same-model".into());
            child.execution = Some(corral_core::ExecutionSnapshot::new(
                &fx.ctx.store.config,
                resolved_sub.clone(),
            ));
            child.review = Some(record);
            Ok(())
        })
        .unwrap();
    let host = ts::bind(&fx, &target, "p2", "old-review-session");
    ts::committed(&ts::hello(&fx, &host));
    let source = container(&id, "R1", "R1-review1");
    continuation::with_state(&fx.ctx, &source, |state| {
        state.generations[0].connection = Some(sub.clone());
        state.turns.push(serde_json::from_value(json!({
            "generation":1,"seq":1,"origin":{"kind":"bridge"},"started":"2026-10-23T00:00:00Z","ended":"2026-10-23T00:00:01Z","class":"provider_failed",
            "provider_failure":{"version":1,"connection":"openai-subscription","category":"subscription_quota","scope":"account"}
        }))?);
        Ok(())
    }).unwrap();
    let path = host.sessions.join("session.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        format!(
            "{}\n",
            json!({"type":"session","id":"old-review-session","cwd":dir})
        ),
    )
    .unwrap();
    let notes = crate::reviewer::review_notes_path(&fx.ctx, &id, "R1").unwrap();
    std::fs::create_dir_all(notes.parent().unwrap()).unwrap();
    std::fs::write(notes, "partial observations, not a verdict").unwrap();
    Review {
        fx,
        id,
        host,
        api,
        sub,
    }
}

impl Review {
    fn prepare(&self) -> Result<ContinuationIntent> {
        let task = self.fx.ctx.store.head(&self.id)?;
        let run = task.current_run.as_deref().unwrap();
        prepare(
            &self.fx.ctx,
            &task,
            run,
            run.current_child.as_deref(),
            "R1-review2",
            &self.api,
        )
    }

    fn fresh_child(&self, intent: &ContinuationIntent, healthy: bool) -> ts::Host {
        let task = self.fx.ctx.store.head(&self.id).unwrap();
        let parent = task.current_run.as_deref().unwrap();
        let mut record = parent.review.clone().unwrap();
        record.at = corral_core::clock::now();
        record.notes = None;
        record.pane = Some("p3".into());
        let mut child = crate::reviewer::launch::review_child(
            &self.fx.ctx,
            parent,
            &self.api,
            &corral_core::Reviewer::Profile("pi".into()),
            "R1-review2",
            &record,
        )
        .unwrap();
        child.herdr = Some(corral_core::HerdrRef {
            workspace_id: "w1".into(),
            pane_id: "p3".into(),
            socket: Some(self.fx.ctx.herdr.socket_path().unwrap()),
        });
        child.session =
            Some(crate::role_session::reviewer_head(&self.fx.ctx, &task, parent, &child).unwrap());
        assert_eq!(child.session.as_ref().unwrap().action, SessionAction::New);
        assert_ne!(
            child.session.as_ref().unwrap().id.as_deref(),
            Some("old-review-session")
        );
        self.fx
            .ctx
            .store
            .update(&self.id, |task| {
                task.runs[0].review_runs.push(child.clone());
                task.runs[0].review = Some(record.clone());
                Ok(())
            })
            .unwrap();
        continuation::require_launch(
            &self.fx.ctx.store,
            &intent.successor.container,
            &intent.successor.binding,
        )
        .unwrap();
        let mut host = ts::bind(
            &self.fx,
            &BindTarget::Reviewer {
                task: self.id.clone(),
                run: "R1".into(),
                review: "R1-review2".into(),
                callback: "new-callback".into(),
            },
            "p3",
            "new-review-session",
        );
        host.sessions = child.session.as_ref().unwrap().directory.clone();
        continuation::with_state(&self.fx.ctx, &intent.successor.container, |state| {
            state.generations[0].connection = Some(intent.successor.binding.clone());
            Ok(())
        })
        .unwrap();
        if !healthy {
            return host;
        }
        let payload = json!({"binding_digest":intent.successor.binding.digest,"ctl_cap_sha256":corral_core::agent::paths::sha256_hex(host.ctl_cap.as_bytes()),"session_file":host.sessions.join("session.jsonl"),"pid":4242,"start_time":"123456","pane":host.pane});
        ts::committed(&ts::send(
            &self.fx,
            &ts::envelope(&host, "hello", "ev-1-0", payload, true),
            "hello",
        ));
        host
    }
}

#[test]
fn paid_reviewer_fresh_attempt_preserves_only_notes_and_closes_on_verdict() {
    let review = review();
    let intent = review.prepare().unwrap();
    assert!(matches!(
        intent.context,
        ContextSource::FreshReadOnly { .. }
    ));
    assert_eq!(intent.source.binding, review.sub);
    let task = review.fx.ctx.store.get(&review.id).unwrap();
    assert_eq!(
        task.runs[0].review_runs[0].outcome,
        Some(RunOutcome::Interrupted)
    );
    let notes = crate::reviewer::review_notes_path(&review.fx.ctx, &review.id, "R1").unwrap();
    assert!(!notes.exists());
    assert_eq!(
        std::fs::read_to_string(
            notes.with_file_name(format!("{}-R1-review1-interrupted-notes.md", review.id))
        )
        .unwrap(),
        "partial observations, not a verdict"
    );
    let _host = review.fresh_child(&intent, true);
    let task = review.fx.ctx.store.get(&review.id).unwrap();
    let fresh = &task.runs[0].review_runs[1];
    assert!(fresh.review.as_ref().unwrap().verdict.is_none());
    assert!(fresh.review.as_ref().unwrap().notes.is_none());
    let old = ts::send(
        &review.fx,
        &ts::envelope(
            &review.host,
            "verdict",
            "old-verdict",
            json!({"verdict":"approve","notes":"old attempt"}),
            false,
        ),
        "verdict",
    );
    assert!(old.rejection().is_some(), "{old:?}");
    review
        .fx
        .ctx
        .store
        .update(&review.id, |task| {
            task.runs[0].review_runs[1].review.as_mut().unwrap().verdict =
                Some(corral_core::Verdict::Approve);
            verdict_commit(&review.fx.ctx, task, "R1")
        })
        .unwrap();
    assert_eq!(
        continuation::intent_for(&review.fx.ctx.store, &intent.successor.container, 1)
            .unwrap()
            .unwrap()
            .phase,
        IntentPhase::Closed
    );
    // A successful transport submission can still lose its native host before hello.
    let lost = self::review();
    let intent = lost.prepare().unwrap();
    lost.fresh_child(&intent, false);
    continuation::with_state(&lost.fx.ctx, &intent.successor.container, |state| {
        state.binding.health = agent::HostHealth::Lost;
        Ok(())
    })
    .unwrap();
    let panes = std::collections::HashMap::from([(
        "p3".into(),
        corral_herdr::PaneInfo {
            pane_id: "p3".into(),
            workspace_id: "w1".into(),
            tab_id: "w1:t3".into(),
            agent: Some("pi".into()),
            status: corral_herdr::AgentStatus::Idle,
            title: None,
            cwd: None,
        },
    )]);
    crate::reviewer::reconcile_review_at(&lost.fx.ctx, &lost.id, corral_core::clock::now(), &panes)
        .unwrap();
    let paused = continuation::intent_for(&lost.fx.ctx.store, &intent.successor.container, 1)
        .unwrap()
        .unwrap();
    assert_eq!(paused.phase, IntentPhase::Paused);
    assert_eq!(paused.pause, Some(PauseReason::SuccessorAmbiguous));
}

#[test]
fn paid_reviewer_lost_ack_and_entering_verdict_never_create_a_competing_child() {
    // Preparation can commit before the accounting child or its native state exists.
    let unbound = review();
    let intent = unbound.prepare().unwrap();
    let before = ts::task_bytes(&unbound.fx, &unbound.id);
    assert!(
        agent::container_state(
            &unbound.fx.ctx.store.get(&unbound.id).unwrap(),
            &intent.successor.container,
        )
        .is_none()
    );
    let error = crate::reviewer::review_run_automatic(&unbound.fx.ctx, &unbound.id, Some("pi"))
        .unwrap_err();
    assert!(format!("{error:#}").contains("SuccessorAmbiguous"));
    let paused = continuation::task_intent(&unbound.fx.ctx.store, &unbound.id, AgentRole::Reviewer)
        .unwrap()
        .unwrap();
    assert_eq!(paused.phase, IntentPhase::Paused);
    assert_eq!(paused.pause, Some(PauseReason::SuccessorAmbiguous));
    assert_eq!(ts::task_bytes(&unbound.fx, &unbound.id), before);
    assert_eq!(
        unbound.fx.ctx.store.get(&unbound.id).unwrap().runs[0]
            .review_runs
            .len(),
        1
    );

    for state in ["committed", "reserved", "uncertain"] {
        let review = review();
        let source = container(&review.id, "R1", "R1-review1");
        continuation::with_state(&review.fx.ctx, &source, |native| {
            native.receipts.push(serde_json::from_value(json!({"version":1,"request_id":"verdict-receipt","op":"verdict","payload_sha256":"00","identity":review.host.identity,"state":state,"result":{"verdict":"approve"},"at":"2026-10-23T00:00:01Z"}))?);
            Ok(())
        }).unwrap();
        let before = ts::task_bytes(&review.fx, &review.id);
        let task = review.fx.ctx.store.get(&review.id).unwrap();
        let parent = &task.runs[0];
        assert!(
            crate::reviewer::continue_review(
                &review.fx.ctx,
                &task,
                parent,
                &parent.review_runs[0],
                "subscription quota",
                corral_core::clock::now(),
            )
            .unwrap()
            .is_empty()
        );
        assert_eq!(ts::task_bytes(&review.fx, &review.id), before);
        assert!(
            continuation::INTENTS
                .list(&review.fx.ctx.store)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn paid_reviewer_same_family_is_refused_at_launch_and_at_verdict() {
    let review = review();
    review
        .fx
        .ctx
        .store
        .update(&review.id, |task| {
            task.runs[0].vendor = Some("openai".into());
            Ok(())
        })
        .unwrap();
    let before = ts::task_bytes(&review.fx, &review.id);
    assert!(review.prepare().is_err());
    assert_eq!(ts::task_bytes(&review.fx, &review.id), before);
    review
        .fx
        .ctx
        .store
        .update(&review.id, |task| {
            task.runs[0].vendor = Some("anthropic".into());
            Ok(())
        })
        .unwrap();
    let intent = review.prepare().unwrap();
    review.fresh_child(&intent, true);
    review
        .fx
        .ctx
        .store
        .update(&review.id, |task| {
            task.runs[0].vendor = Some("openai".into());
            Ok(())
        })
        .unwrap();
    let task = review.fx.ctx.store.get(&review.id).unwrap();
    let error = verdict_check(&review.fx.ctx, &task, "R1").unwrap_err();
    verdict_pause(&review.fx.ctx, &task, "R1", &error).unwrap();
    let paused = continuation::intent_for(&review.fx.ctx.store, &intent.successor.container, 1)
        .unwrap()
        .unwrap();
    assert_eq!(paused.pause, Some(PauseReason::IndependenceViolated));
    let task = review.fx.ctx.store.get(&review.id).unwrap();
    assert!(
        task.runs[0].review_runs[1]
            .review
            .as_ref()
            .unwrap()
            .verdict
            .is_none()
    );
}
