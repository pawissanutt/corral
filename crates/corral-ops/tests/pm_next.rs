use std::path::Path;
use std::process::Stdio;

mod support;

use corral_core::{
    DecisionRecord, DecisionRequest, ObligationSource, Run, Status, Store, WaitObservation,
    WaitState,
};
use corral_ops::{Ctx, PmActionKind, PmObservation, capture_messages, pm_card, pm_next, pm_select};
use serde_json::json;

fn git(root: &Path, args: &[&str]) -> String {
    let output = support::command("git", root)
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().into()
}

fn fixture() -> (tempfile::TempDir, Ctx, String) {
    let tmp = tempfile::tempdir().unwrap();
    git(tmp.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(tmp.path().join(".gitignore"), ".corral/\n").unwrap();
    git(tmp.path(), &["add", ".gitignore"]);
    git(
        tmp.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ],
    );
    let store = Store::init(tmp.path()).unwrap();
    let task = store
        .create("Test task", None, Status::Review, "## Goal\nTest policy")
        .unwrap();
    let head = git(tmp.path(), &["rev-parse", "HEAD"]);
    let run: Run = serde_json::from_value(json!({
        "id":"R1","agent":"generic","mode":"interactive","worktree":tmp.path(),
        "branch":"main","base":"main","base_sha":head,"started":"2026-10-15T12:00:00Z",
        "seen_working":true,"agent_state":"working"
    }))
    .unwrap();
    store
        .update(&task.id, |t| {
            t.runs.push(run);
            Ok(())
        })
        .unwrap();
    (
        tmp,
        corral_ops::resources::FixedResourceProbe::ctx(store),
        task.id,
    )
}

#[test]
fn finished_run_answer_clears_question_and_allows_gate() {
    use corral_core::{ReceiptOutcome, ResumeDelivery, ResumeReceipt, RunOutcome};
    use corral_ops::{ResumeTarget, resume_run};
    let (_tmp, mut ctx, id) = fixture();
    ctx.store.config.review.gate = Some(vec!["cargo test".into()]);
    ctx.store.config.projects.insert(
        "app".into(),
        serde_json::from_value(json!({"path":"."})).unwrap(),
    );
    ctx.store.save_config().unwrap();
    ctx.store
        .update(&id, |task| {
            task.project = Some("app".into());
            Ok(())
        })
        .unwrap();
    let preview = ctx
        .store
        .preview_operating_policy(&corral_core::OperatingSelection {
            tasks: [id.clone()].into(),
            authority: corral_core::OperatingAuthority {
                dispatch: true,
                accept: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    ctx.store.config = ctx
        .store
        .confirm_operating_policy(
            &preview,
            "fixture owner",
            "2026-10-29T08:00:00Z".parse().unwrap(),
        )
        .unwrap();
    let at = "2026-10-15T12:00:00Z".parse().unwrap();
    ctx.store
        .update(&id, |t| {
            let run = t.current_run_mut().unwrap();
            run.outcome = Some(RunOutcome::Done);
            run.obligations.decisions.push(DecisionRecord {
                request: DecisionRequest {
                    version: 1,
                    id: "Q1".into(),
                    run: "R1".into(),
                    question: "A or B?".into(),
                    requested_action: "Choose".into(),
                    owner: "owner".into(),
                    source: ObligationSource::Worker,
                    deadline: None,
                },
                registered_at: at,
                observation: WaitObservation {
                    at,
                    state: WaitState::Pending,
                    detail: String::new(),
                },
                receipt: None,
            });
            Ok(())
        })
        .unwrap();
    assert_eq!(pm_next(&ctx).unwrap().kind, PmActionKind::Question);
    let answer = ResumeReceipt {
        version: 1,
        id: "answer-1".into(),
        run: "R1".into(),
        outcome: ReceiptOutcome::Answered,
        delivery: ResumeDelivery::Acknowledged,
        observed_at: at,
        answer: Some("A".into()),
        data: json!({}),
    };
    let task = resume_run(&ctx, &id, ResumeTarget::Decision("Q1"), answer.clone(), at).unwrap();
    assert_eq!(task.status, Status::Review);
    assert_eq!(task.runs.len(), 1);
    assert_eq!(
        task.current_run().unwrap().obligations.decisions[0].receipt,
        Some(answer.clone())
    );
    resume_run(&ctx, &id, ResumeTarget::Decision("Q1"), answer, at).unwrap();
    let card = pm_next(&ctx).unwrap();
    assert_eq!(card.kind, PmActionKind::Gate, "{card:#?}");
}

#[test]
fn pending_question_outranks_working_and_readback_is_side_effect_free() {
    // Given a working run with an exact pending harness decision.
    let (tmp, ctx, id) = fixture();
    let at = "2026-10-15T12:00:00Z".parse().unwrap();
    ctx.store
        .update(&id, |t| {
            t.status = Status::Running;
            t.current_run_mut()
                .unwrap()
                .obligations
                .decisions
                .push(DecisionRecord {
                    request: DecisionRequest {
                        version: 1,
                        id: "Q1".into(),
                        run: "R1".into(),
                        question: "Select A or B?".into(),
                        requested_action: "Answer A or B".into(),
                        owner: "owner".into(),
                        source: ObligationSource::Adapter,
                        deadline: None,
                    },
                    registered_at: at,
                    observation: WaitObservation {
                        at,
                        state: WaitState::Pending,
                        detail: String::new(),
                    },
                    receipt: None,
                });
            Ok(())
        })
        .unwrap();
    let tasks = ctx.store.list().unwrap();
    let events = ctx.store.events().unwrap();
    let git_before = git(tmp.path(), &["status", "--porcelain"]);
    // When read twice without syncing or any available herdr server.
    let a = pm_next(&ctx).unwrap();
    let b = pm_next(&ctx).unwrap();
    // Then the same single card exposes Needs me without mutating tasks, events, or the worktree.
    assert_eq!(a, b);
    assert_eq!(a.kind, PmActionKind::Question);
    assert!(a.requires_owner);
    assert_eq!(a.recommended, "ask_owner");
    assert_eq!(ctx.store.list().unwrap(), tasks);
    assert_eq!(ctx.store.events().unwrap(), events);
    assert_eq!(git(tmp.path(), &["status", "--porcelain"]), git_before);
}

#[test]
fn unavailable_worktree_uses_shared_diagnostic_sink_and_owner_card() {
    // Given a review whose worktree disappeared.
    let (_tmp, ctx, id) = fixture();
    ctx.store
        .update(&id, |t| {
            t.current_run_mut().unwrap().worktree.push("gone");
            Ok(())
        })
        .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let _guard = capture_messages(tx);
    // When the next action is computed.
    let card = pm_next(&ctx).unwrap();
    // Then failure cannot turn into permission to land, and diagnostics reach the owner sink.
    assert_eq!(card.kind, PmActionKind::Owner);
    let diagnostic = rx.try_recv().unwrap();
    assert_eq!(diagnostic.task.as_deref(), Some(id.as_str()));
    assert_eq!(diagnostic.source, "pm-next");
}

#[test]
fn unspecified_ideas_are_not_promotion_candidates() {
    // Given an idea without a project or acceptance section.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let idea = store
        .create("An idea", None, Status::Idea, "Not a spec")
        .unwrap();
    let ctx = corral_ops::resources::FixedResourceProbe::ctx(store);
    // When the PM asks what comes next.
    let card = pm_next(&ctx).unwrap();
    // Then idle capacity alone does not qualify it for promotion.
    assert_eq!(card.kind, PmActionKind::Wait);
    assert!(card.shortlist.is_empty());
    assert_eq!(ctx.store.get(&idea.id).unwrap().status, Status::Idea);
}

fn promotion_fixture() -> (tempfile::TempDir, Ctx, String) {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("app/.git")).unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.limits.running = 3;
    store.config.limits.review = 1;
    let task = store
        .create(
            "Eligible idea",
            Some("app"),
            Status::Idea,
            "## Acceptance\nTests pass.",
        )
        .unwrap();
    let ctx = corral_ops::resources::FixedResourceProbe::ctx(store);
    (tmp, ctx, task.id)
}

#[test]
fn suggest_card_is_ranked_closed_and_read_only() {
    let (_tmp, ctx, id) = promotion_fixture();
    let second = ctx
        .store
        .create(
            "Another idea",
            Some("app"),
            Status::Idea,
            "## Acceptance\nWorks.",
        )
        .unwrap();
    ctx.store
        .update(&second.id, |t| {
            t.priority = "P0".into();
            Ok(())
        })
        .unwrap();
    let tasks = ctx.store.list().unwrap();
    let events = ctx.store.events().unwrap();
    let first = pm_next(&ctx).unwrap();
    assert_eq!(first, pm_next(&ctx).unwrap());
    assert_eq!(first.kind, PmActionKind::IdeaPromotion);
    assert_eq!(serde_json::to_value(&first).unwrap()["kind"], "promote");
    assert_eq!(first.task.as_deref(), Some(second.id.as_str()));
    assert!(first.requires_owner);
    assert_eq!(
        first.shortlist.iter().map(|c| &c.id).collect::<Vec<_>>(),
        [&second.id, &id]
    );
    assert_eq!(first.options.len(), 3);
    for (candidate, choice) in first.shortlist.iter().zip(&first.options) {
        assert!(choice.requires_owner);
        assert!(
            choice
                .command
                .ends_with(&format!(" move '{}' backlog", candidate.id))
        );
    }
    assert_eq!(first.options.last().unwrap().id, "wait");
    assert_eq!(ctx.store.list().unwrap(), tasks);
    assert_eq!(ctx.store.events().unwrap(), events);
    assert!(!ctx.store.state_dir().join("resources.json").exists());
    assert_eq!(
        serde_json::from_str::<PmActionKind>("\"idea_promotion\"").unwrap(),
        PmActionKind::IdeaPromotion
    );
}

#[test]
fn promotion_cards_only_use_effective_suggest_mode() {
    use corral_core::PromoteIdeas;
    for mode in [
        PromoteIdeas::Never,
        PromoteIdeas::Suggest,
        PromoteIdeas::Auto,
    ] {
        let (_tmp, mut ctx, _id) = promotion_fixture();
        ctx.store.config.dispatch.promote_ideas = mode;
        let expected = if mode == PromoteIdeas::Suggest {
            PmActionKind::IdeaPromotion
        } else {
            PmActionKind::Wait
        };
        assert_eq!(pm_next(&ctx).unwrap().kind, expected, "{mode:?}");
        ctx.store
            .config
            .projects
            .get_mut("app")
            .unwrap()
            .dispatch
            .promote_ideas = Some(PromoteIdeas::Suggest);
        assert_eq!(pm_next(&ctx).unwrap().kind, PmActionKind::IdeaPromotion);
        ctx.store.config.dispatch.promote_ideas = PromoteIdeas::Suggest;
        ctx.store
            .config
            .projects
            .get_mut("app")
            .unwrap()
            .dispatch
            .promote_ideas = Some(PromoteIdeas::Auto);
        assert_eq!(pm_next(&ctx).unwrap().kind, PmActionKind::Wait);
    }
}

#[test]
fn promotion_respects_pause_resource_guard_and_reserved_review_headroom() {
    let (_tmp, mut ctx, _id) = promotion_fixture();
    ctx.store.config.dispatch.paused = true;
    assert_eq!(pm_next(&ctx).unwrap().kind, PmActionKind::Wait);
    ctx.store.config.dispatch.paused = false;
    ctx.store.config.limits.running = 1;
    assert_eq!(pm_next(&ctx).unwrap().kind, PmActionKind::Wait);
    ctx.store.config.limits.running = 3;
    ctx.store.config.limits.per_project.insert("app".into(), 0);
    assert_eq!(pm_next(&ctx).unwrap().kind, PmActionKind::Wait);
    ctx.store.config.limits.per_project.clear();
    let mut probe = corral_ops::resources::FixedResourceProbe::healthy();
    probe.0.mem_available = 0;
    ctx.resource_probe = std::sync::Arc::new(probe);
    assert_eq!(pm_next(&ctx).unwrap().kind, PmActionKind::Wait);
    ctx.resource_probe = corral_ops::resources::FixedResourceProbe::arc();
    assert_eq!(pm_next(&ctx).unwrap().kind, PmActionKind::IdeaPromotion);
}

#[test]
fn blocked_backlog_does_not_hide_eligible_idea_suggestion() {
    let (_tmp, ctx, _id) = promotion_fixture();
    let task = ctx
        .store
        .create("Backlog", Some("app"), Status::Backlog, "Spec")
        .unwrap();
    ctx.store
        .update(&task.id, |t| {
            t.depends_on = vec!["T-999".into()];
            Ok(())
        })
        .unwrap();
    assert_eq!(pm_next(&ctx).unwrap().kind, PmActionKind::IdeaPromotion);
}

#[test]
fn disabled_automatic_backlog_does_not_hide_idea_suggestion() {
    let (_tmp, mut ctx, _id) = promotion_fixture();
    ctx.store.config.dispatch.auto_backlog = false;
    ctx.store
        .create("Backlog", Some("app"), Status::Backlog, "Spec")
        .unwrap();
    assert_eq!(pm_next(&ctx).unwrap().kind, PmActionKind::IdeaPromotion);
}

#[test]
fn ready_dispatch_still_outranks_idea_promotion() {
    let (_tmp, ctx, _id) = promotion_fixture();
    let task = ctx
        .store
        .create("Ready", Some("app"), Status::Ready, "Spec")
        .unwrap();
    let card = pm_next(&ctx).unwrap();
    assert_eq!(card.kind, PmActionKind::Dispatch);
    assert_eq!(card.task.as_deref(), Some(task.id.as_str()));
}

#[test]
fn dispatch_recommendations_include_active_gate_capacity() {
    let (_tmp, mut ctx, id) = fixture();
    ctx.store.config.limits.running = 2;
    ctx.store.save_config().unwrap();
    let project = ctx.store.config.projects.keys().next().unwrap().clone();
    let ready = ctx
        .store
        .create("Ready work", Some(&project), Status::Ready, "Spec")
        .unwrap();
    ctx.store
        .update(&id, |task| {
            task.status = Status::Running;
            task.current_run_mut().unwrap().gate = Some(
                serde_json::from_value(json!({
                    "result":"running","sha":"head","at":"2026-10-15T12:00:00Z"
                }))
                .unwrap(),
            );
            Ok(())
        })
        .unwrap();
    // A running worker's live gate shares its worker's slot, so one of the two slots stays free.
    assert_eq!(pm_next(&ctx).unwrap().kind, PmActionKind::Dispatch);
    ctx.store
        .update(&id, |task| {
            task.current_run_mut().unwrap().gate = None;
            Ok(())
        })
        .unwrap();
    let action = pm_next(&ctx).unwrap();
    assert_eq!(action.kind, PmActionKind::Dispatch);
    assert_eq!(action.task.as_deref(), Some(ready.id.as_str()));
}

#[test]
fn pending_review_prevents_landing_even_when_another_candidate_is_ready() {
    // Given a completed task and another task with a pending review in the same project.
    let (_tmp, mut ctx, id) = fixture();
    ctx.store.config.projects.insert(
        "app".into(),
        serde_json::from_value(json!({"path":"."})).unwrap(),
    );
    ctx.store.save_config().unwrap();
    ctx.store
        .update(&id, |task| {
            task.project = Some("app".into());
            task.current_run_mut().unwrap().outcome = Some(corral_core::RunOutcome::Done);
            Ok(())
        })
        .unwrap();
    let other = ctx
        .store
        .create("Under review", Some("app"), Status::Review, "spec")
        .unwrap();
    let mut run = ctx.store.get(&id).unwrap().current_run().unwrap().clone();
    run.review = Some(
        serde_json::from_value(json!({
            "reviewer":"strong","sha":run.base_sha,"at":"2026-10-15T12:00:00Z",
            "pane":"recorded-reviewer"
        }))
        .unwrap(),
    );
    ctx.store
        .update(&other.id, |t| {
            t.runs.push(run);
            Ok(())
        })
        .unwrap();
    // A missing owner grant is not the blocker this fixture is testing.
    let preview = ctx
        .store
        .preview_operating_policy(&corral_core::OperatingSelection {
            tasks: [id.clone(), other.id.clone()].into(),
            authority: corral_core::OperatingAuthority {
                dispatch: true,
                accept: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    ctx.store.config = ctx
        .store
        .confirm_operating_policy(
            &preview,
            "fixture owner",
            "2026-10-29T08:00:00Z".parse().unwrap(),
        )
        .unwrap();
    // When all cards are ranked.
    let card = pm_next(&ctx).unwrap();
    // Then the only recommendation is to wait, never land.
    assert_eq!(card.kind, PmActionKind::ReviewInFlight, "{card:#?}");
    assert_eq!(card.recommended, "wait");
}

#[test]
fn stale_review_returns_a_factual_rebase_regate_sendback() {
    // Given a verdict bound to a different HEAD.
    let (_tmp, ctx, id) = fixture();
    ctx.store
        .update(&id, |t| {
            t.current_run_mut().unwrap().review = Some(
                serde_json::from_value(json!({
                    "reviewer":"strong","sha":"old","at":"2026-10-15T12:00:00Z","verdict":"approve"
                }))
                .unwrap(),
            );
            Ok(())
        })
        .unwrap();
    // When next is computed.
    let card = pm_next(&ctx).unwrap();
    // Then it recommends the changes command with an explicit plan, not an approval.
    assert_eq!(card.kind, PmActionKind::StaleVerdict);
    assert_eq!(card.recommended, "send_back");
    assert_eq!(card.options.len(), 1);
}

#[test]
fn cards_for_load_failure_require_isolated_evidence_before_filing_flake() {
    // Given one observed failure without an isolated result, then the same facts with a pass.
    for (alone_passed, expected) in [
        (None, "rerun_alone"),
        (Some(false), "rerun_alone"),
        (Some(true), "file_flake"),
    ] {
        let observation = PmObservation::LoadFailure {
            task: "T-001".into(),
            command: "cargo test selected_test".into(),
            alone_passed,
            evidence: "attempt G1 failed under load".into(),
        };
        // When a closed card is built.
        let card = pm_card(&observation, "corral");
        // Then filing depends on the recorded isolated outcome.
        assert_eq!(card.kind, PmActionKind::LoadFailure);
        assert_eq!(card.recommended, expected);
    }
}

#[test]
fn a_question_takes_priority_over_landing_and_dispatch() {
    // Given simultaneous actionable observations.
    let observations = [
        PmObservation::ReadyToLand {
            task: "T-001".into(),
        },
        PmObservation::Dispatch {
            task: "T-002".into(),
        },
        PmObservation::Question {
            task: "T-003".into(),
            question: "A or B?".into(),
            source: "adapter".into(),
            agent_state: "working".into(),
        },
    ];
    // When selecting the single next action.
    let card = pm_select(&observations, "corral");
    // Then the exact question wins.
    assert_eq!(card.kind, PmActionKind::Question);
    assert_eq!(card.task.as_deref(), Some("T-003"));
}

#[test]
fn no_observations_returns_one_wait_action() {
    // Given an unchanged board with no pending work.
    let observations = [];
    // When selecting next.
    let card = pm_select(&observations, "corral");
    // Then one wait option is returned.
    assert_eq!(card.kind, PmActionKind::Wait);
    assert_eq!(card.options.len(), 1);
    assert_eq!(card.recommended, "wait");
}

#[test]
fn pass_event_without_attempt_log_returns_regate_card() {
    // Given a pass event and legacy pass record with no per-attempt proof.
    let (_tmp, mut ctx, id) = fixture();
    ctx.store.config.review.gate = Some(vec!["cargo test".into(), "cargo build".into()]);
    ctx.store.save_config().unwrap();
    ctx.store
        .update(&id, |t| {
            let run = t.current_run_mut().unwrap();
            run.gate = Some(
                serde_json::from_value(json!({
                    "result":"pass","sha":run.base_sha,"at":"2026-10-15T12:00:00Z"
                }))
                .unwrap(),
            );
            Ok(())
        })
        .unwrap();
    ctx.store
        .log_event(Some(&id), "gate", json!({"run":"R1","result":"pass"}))
        .unwrap();
    // When next is computed from both task state and events.
    let card = pm_next(&ctx).unwrap();
    // Then an event is not sufficient authority to land.
    assert_eq!(card.kind, PmActionKind::GateLog);
    assert_eq!(card.recommended, "regate");
}

#[test]
fn evidence_only_card_requires_code_computed_identical_commit_ranges() {
    // Given a fresh review whose machine-readable finding is evidence-only.
    let (tmp, ctx, id) = fixture();
    let base = git(tmp.path(), &["rev-parse", "HEAD"]);
    git(tmp.path(), &["checkout", "-qb", "feature"]);
    std::fs::write(tmp.path().join("widget.txt"), "one\n").unwrap();
    git(tmp.path(), &["add", "widget.txt"]);
    git(
        tmp.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "widget",
        ],
    );
    let head = git(tmp.path(), &["rev-parse", "HEAD"]);
    let notes = json!({"scope":"evidence_only","base":base,"head":head,"finding":"Capture names a previous executable."}).to_string();
    corral_ops::verdict(&ctx, &id, corral_core::Verdict::Changes, &notes).unwrap();
    // When next invokes Git to compare the two explicit commit ranges.
    let card = pm_next(&ctx).unwrap();
    // Then it offers acceptance only to the acting owner.
    assert_eq!(card.kind, PmActionKind::EvidenceOnly);
    assert_eq!(card.recommended, "accept_owner");
    assert!(card.requires_owner);
}

#[test]
fn changed_patches_do_not_receive_evidence_only_acceptance() {
    // Given an evidence-only finding about an older commit range, but a changed patch now.
    let (tmp, ctx, id) = fixture();
    let base = git(tmp.path(), &["rev-parse", "HEAD"]);
    git(tmp.path(), &["checkout", "-qb", "feature"]);
    std::fs::write(tmp.path().join("widget.txt"), "one\n").unwrap();
    git(tmp.path(), &["add", "widget.txt"]);
    git(
        tmp.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "widget",
        ],
    );
    let old = git(tmp.path(), &["rev-parse", "HEAD"]);
    std::fs::write(tmp.path().join("widget.txt"), "two\n").unwrap();
    git(tmp.path(), &["add", "widget.txt"]);
    git(
        tmp.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "changed",
        ],
    );
    let notes = json!({"scope":"evidence_only","base":base,"head":old,"finding":"Old capture."})
        .to_string();
    corral_ops::verdict(&ctx, &id, corral_core::Verdict::Changes, &notes).unwrap();
    // When the actual changed ranges are compared.
    let card = pm_next(&ctx).unwrap();
    // Then the PM must escalate the finding instead of accepting based on prose.
    assert_eq!(card.kind, PmActionKind::Owner);
}

#[test]
fn isolated_pass_artifact_yields_flake_bug_without_waiving_gate() {
    // Given a hash-bound observation of a failed loaded run and an isolated pass.
    let (_tmp, ctx, id) = fixture();
    let head = ctx
        .store
        .get(&id)
        .unwrap()
        .current_run()
        .unwrap()
        .base_sha
        .clone();
    let file = ctx.store.state_dir().join("alone.json");
    std::fs::write(
        &file,
        json!({
            "version":1,"task":id,"run":"R1","head":head,"command":"cargo test selected",
            "under_load_exit":101,"alone_exit":0,"log":"attached-run-log"
        })
        .to_string(),
    )
    .unwrap();
    corral_ops::add_artifact(
        &ctx,
        &id,
        &file,
        Some(corral_core::ArtifactKind::Evidence),
        Some("PM isolated test result"),
    )
    .unwrap();
    // When the code reads the recorded result.
    let card = pm_next(&ctx).unwrap();
    // Then the only recommended mutation files a bug, not a passing verdict.
    assert_eq!(card.kind, PmActionKind::LoadFailure);
    assert_eq!(card.recommended, "file_flake");
    assert_eq!(ctx.store.get(&id).unwrap().status, Status::Review);
}

#[test]
fn compressed_isolated_result_is_decoded_before_hash_and_json_checks() {
    use std::io::Write;
    // Given a retained observation compressed by the shared artifact-storage contract.
    let (_tmp, ctx, id) = fixture();
    let head = ctx
        .store
        .get(&id)
        .unwrap()
        .current_run()
        .unwrap()
        .base_sha
        .clone();
    let bytes = json!({
        "version":1,"task":id,"run":"R1","head":head,"command":"cargo test selected",
        "under_load_exit":101,"alone_exit":0,"log":"attached-run-log"
    })
    .to_string()
    .into_bytes();
    let file = ctx.store.state_dir().join("compressed-alone.json");
    std::fs::write(&file, &bytes).unwrap();
    let artifact = corral_ops::add_artifact(
        &ctx,
        &id,
        &file,
        Some(corral_core::ArtifactKind::Evidence),
        Some("PM isolated test result"),
    )
    .unwrap();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&bytes).unwrap();
    let compressed = encoder.finish().unwrap();
    std::fs::write(ctx.store.dir().join(&artifact.path), &compressed).unwrap();
    ctx.store
        .update(&id, |task| {
            task.artifacts
                .iter_mut()
                .find(|a| a.path == artifact.path)
                .unwrap()
                .compressed_bytes = Some(u64::try_from(compressed.len()).unwrap());
            Ok(())
        })
        .unwrap();
    // When next reads the observation through the same decoder as artifact previews.
    let card = pm_next(&ctx).unwrap();
    // Then compression does not turn a recorded isolated pass into unavailable evidence.
    assert_eq!(card.kind, PmActionKind::LoadFailure);
    assert_eq!(card.recommended, "file_flake");
}
