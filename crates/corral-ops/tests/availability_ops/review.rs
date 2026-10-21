use super::*;

fn reviewer_candidates(ids: &[(&str, &str)]) -> Vec<ExecutionCandidate> {
    ids.iter()
        .map(|(id, provider)| candidate(id, provider, &format!("owner-{provider}"), "high"))
        .collect()
}

/// Sol implements; the reviewer profile must come from another model family.
fn in_review(reviewers: Vec<ExecutionCandidate>) -> Fx {
    let mut enabled: Vec<String> = [sol(), opus()].iter().map(|c| c.model.clone()).collect();
    enabled.extend(reviewers.iter().map(|c| c.model.clone()));
    let fx = Fx::new(Setup {
        enabled,
        configure: Box::new(move |store| {
            store.config.profiles.insert(
                "review-general".into(),
                profile(reviewers, ProfileRole::ImplementationReview),
            );
            store.config.review.reviewer = Some("review-general".into());
            store.config.review.require_cross_family = Some(true);
            store.config.review.gate = Some(vec!["true".into()]);
        }),
        ..Setup::default()
    });
    let run = fx.run();
    std::fs::write(run.worktree.join("widget.txt"), "widget\n").unwrap();
    corral_git::commit_all(&run.worktree, "T-001: widget").unwrap();
    fx.checkpoint();
    corral_ops::done(&fx.ctx, &fx.id, "made the widget").unwrap();
    // The review automation gates the candidate, then starts the configured reviewer.
    let started = fx.sync(1).current_run().unwrap().review_runs.len();
    assert_eq!(started, 1);
    fx
}

#[test]
fn reviewer_fallback_is_a_new_attempt_that_keeps_independence() {
    // Given a cross-family review on Claude-like, with GPT-like and Gemini-like approved next.
    let fx = in_review(reviewer_candidates(&[
        ("rv-claude", "anthropic"),
        ("rv-gpt", "openai"),
        ("rv-gemini", "google"),
    ]));
    let run = fx.run();
    let first = run.review_runs[0].clone();
    assert_eq!(first.model.as_deref(), Some("anthropic/rv-claude-fixture"));
    let sha = run.review.as_ref().unwrap().sha.clone();
    let notes = corral_ops::review_notes_path(&fx.ctx, &fx.id, &run.id);
    std::fs::write(&notes, "Partial: looks fine so far").unwrap();
    // When the reviewer's quota is exhausted mid-review.
    fx.fail_turn(&first, "r1.jsonl", "quota exceeded for model rv-claude", 10);
    let task = fx.sync(900);
    // Then a new review attempt for the same candidate uses the next eligible family.
    assert_eq!(task.status, Status::Review);
    let run = task.current_run().unwrap();
    assert_eq!(run.review_runs.len(), 2);
    assert_eq!(run.review_runs[0].outcome, Some(RunOutcome::Interrupted));
    assert!(
        run.review_runs[0]
            .review
            .as_ref()
            .is_none_or(|r| r.verdict.is_none()),
        "partial notes never become a verdict"
    );
    let second = &run.review_runs[1];
    assert_eq!(second.agent, "review-general");
    assert_eq!(second.model.as_deref(), Some("google/rv-gemini-fixture"));
    let review = run.review.as_ref().unwrap();
    assert_eq!(review.verdict, None);
    assert_eq!(review.sha, sha);
    let selected = fx.events("availability_fallback_selected");
    assert_eq!(selected[0]["skipped"][0]["candidate"], "rv-gpt");
    assert!(
        selected[0]["skipped"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("cross-family")
    );
    assert_eq!(fx.events("review_interrupted").len(), 1);
    let kept = notes.with_file_name(format!("{}-{}-interrupted-notes.md", fx.id, first.id));
    assert_eq!(
        std::fs::read_to_string(kept).unwrap(),
        "Partial: looks fine so far"
    );
}

#[test]
fn reviewer_fallback_never_downgrades_required_independence() {
    // Given only a same-family alternative to the failing cross-family reviewer.
    let fx = in_review(reviewer_candidates(&[
        ("rv-claude", "anthropic"),
        ("rv-gpt", "openai"),
    ]));
    let first = fx.run().review_runs[0].clone();
    // When the reviewer becomes unavailable.
    fx.fail_turn(&first, "r1.jsonl", "model_not_found: rv-claude", 10);
    let task = fx.sync(900);
    // Then no same-family review is started; the owner gets the reason instead.
    assert_eq!(task.status, Status::NeedsMe);
    let run = task.current_run().unwrap();
    assert_eq!(run.review_runs.len(), 1);
    let excerpt = run.blocked_excerpt.clone().unwrap();
    assert!(excerpt.contains("cross-family"), "{excerpt}");
}
