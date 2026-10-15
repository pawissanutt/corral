use super::*;
use serde_json::{Value, json};

const PACKET: &str = "Integration repair packet";

fn configured() -> Fx {
    Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Auto);
        r.max_rounds = Some(3);
        r.remove_worktree = Some(false);
    })
}

fn commit(repo: &Path, subject: &str) -> String {
    git(repo, &["add", "-A", "--", ".", ":!.worktrees"]);
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["commit", "-q", "-m", subject])
        .env("GIT_AUTHOR_DATE", "2026-10-15T08:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-10-15T08:00:00Z")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    git(repo, &["rev-parse", "HEAD"])
}

fn main_change(fx: &Fx, path: &str, text: &str, subject: &str) -> String {
    let file = fx.app.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
    commit(&fx.app, subject)
}

fn rust_lines(count: usize) -> String {
    let mut text = String::from("// ignored header\n\n    // ignored indented comment\n");
    for i in 0..count {
        text.push_str(&format!("const LINE_{i}: usize = {i}; // counted\n"));
    }
    text.push_str("\n// ignored footer\n");
    text
}

fn packet(fx: &Fx) -> (PathBuf, Vec<u8>, Value) {
    let task = fx.task();
    let artifact = task
        .artifacts
        .iter()
        .rev()
        .find(|a| a.title == PACKET)
        .unwrap();
    let path = fx.ctx.store.dir().join(&artifact.path);
    let bytes = std::fs::read(&path).unwrap();
    let value = serde_json::from_slice(&bytes).unwrap();
    (path, bytes, value)
}

fn packets(fx: &Fx) -> usize {
    fx.task()
        .artifacts
        .iter()
        .filter(|a| a.title == PACKET)
        .count()
}

fn requests(fx: &Fx) -> Vec<Value> {
    fx.ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "integration_request")
        .map(|e| e.detail)
        .collect()
}

fn key(fx: &Fx, head: &str, base: &str, reason: &str) -> Value {
    json!({"task": fx.id, "run": fx.run().id, "head": head, "base": base, "reason": reason})
}

fn assert_request(fx: &Fx, expected_key: &Value, path: &Path) {
    let events = requests(fx);
    let detail = events.last().unwrap();
    assert_eq!(detail["key"], *expected_key);
    let digest = detail["digest"].as_str().unwrap();
    assert!(!digest.is_empty());
    assert!(
        path.file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with(&format!("-{digest}.json"))
    );
    let recorded = Path::new(detail["packet"].as_str().unwrap());
    assert!(
        recorded == path || fx.ctx.store.dir().join(recorded) == path,
        "{recorded:?} != {path:?}"
    );
}

fn assert_no_launches(fx: &Fx) {
    assert_eq!(fx.events("gate_started"), 0);
    assert_eq!(fx.events("review_started"), 0);
    assert!(fx.run().gate.is_none());
    assert!(fx.run().review.is_none());
}

fn assert_restored(fx: &Fx, head: &str, tree: &str) {
    let worktree = fx.run().worktree;
    assert_eq!(git(&worktree, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&worktree, &["write-tree"]), tree);
    assert_eq!(git(&worktree, &["status", "--porcelain"]), "");
    assert_eq!(git(&worktree, &["ls-files", "-u"]), "");
    for name in ["rebase-merge", "rebase-apply", "MERGE_HEAD"] {
        let path = git(&worktree, &["rev-parse", "--git-path", name]);
        assert!(!Path::new(&path).exists(), "{name} left behind");
    }
}

fn approved_change(fx: &Fx, title: &str, path: &str, text: &str) -> Value {
    let task = fx
        .ctx
        .store
        .create(title, Some("app"), Status::Done, "")
        .unwrap();
    let subject = format!("Landed {title}");
    let sha = main_change(fx, path, text, &subject);
    let branch = format!("corral/{}", task.id);
    git(&fx.app, &["branch", &branch, &sha]);
    fx.ctx
        .store
        .log_event(
            Some(&task.id),
            "approved",
            json!({"sha": sha, "run": "R7", "strategy": "rebase"}),
        )
        .unwrap();
    git(&fx.app, &["branch", "-D", &branch]);
    assert_eq!(git(&fx.app, &["branch", "--list", &branch]), "");
    json!({
        "sha": sha, "subject": subject,
        "tasks": [{"id": task.id, "title": title, "run": "R7", "approved_sha": sha}]
    })
}

#[test]
fn conflict_packet_has_complete_landed_identities_paths_and_exact_sloc() {
    // Given three recorded approvals and an unattributed commit with a misleading task subject.
    let fx = configured();
    let dispatch = fx.run().base_sha;
    fx.change("README.md", "candidate\n");
    fx.change("large.rs", &rust_lines(716));
    fx.change("candidate.txt", "candidate only\n");
    fx.done();
    let head = git(&fx.run().worktree, &["rev-parse", "HEAD"]);
    let tree = git(&fx.run().worktree, &["write-tree"]);
    let a = approved_change(&fx, "Readme work", "README.md", "main\n");
    let b = approved_change(&fx, "Rust work", "large.rs", &rust_lines(720));
    let c = approved_change(&fx, "Other work", "main.txt", "main only\n");
    let subject = "T-002: not actually an approved task";
    let base = main_change(&fx, "unattributed.txt", "manual\n", subject);
    let history = json!([
        {"sha": base, "subject": subject, "tasks": []}, c, b, a
    ]);

    // When sync detects the known conflict before starting either validation stage.
    let task = fx.sync();

    // Then every structured fact is complete and attributable only to approval events.
    assert_eq!(task.status, Status::Running, "{}", fx.reason());
    assert_eq!(task.rounds, 1);
    assert_eq!(fx.stage(), Some(AutoStage::Changes));
    assert_no_launches(&fx);
    assert_restored(&fx, &head, &tree);
    let (path, bytes, p) = packet(&fx);
    let expected_key = key(&fx, &head, &base, "merge_conflict");
    assert_eq!(p["key"], expected_key);
    assert_eq!(p["dispatch_base"], dispatch);
    assert_eq!(p["merge_base"], dispatch);
    assert_eq!(p["dispatch_history"], history);
    assert_eq!(p["missing_from_candidate"], history);
    assert_eq!(p["conflict_paths"], json!(["README.md", "large.rs"]));
    assert_eq!(
        p["candidate_changed_paths"],
        json!(["README.md", "candidate.txt", "large.rs"])
    );
    assert_eq!(
        p["main_changed_paths"],
        json!(["README.md", "large.rs", "main.txt", "unattributed.txt"])
    );
    assert_eq!(p["overlaps"], json!(["README.md", "large.rs"]));
    assert_eq!(
        p["file_sizes"],
        json!([
            {"path":"README.md","candidate_sloc":1,"main_sloc":1,"rust_ceiling":null},
            {"path":"candidate.txt","candidate_sloc":1,"main_sloc":null,"rust_ceiling":null},
            {"path":"large.rs","candidate_sloc":716,"main_sloc":720,"rust_ceiling":800},
            {"path":"main.txt","candidate_sloc":null,"main_sloc":1,"rust_ceiling":null},
            {"path":"unattributed.txt","candidate_sloc":null,"main_sloc":1,"rust_ceiling":null}
        ])
    );
    assert_eq!(p["gate_commands"], json!(["true"]));
    assert!(p["git_details"].is_string());
    assert_eq!(p["reviewer_note"], Value::Null);
    assert_request(&fx, &expected_key, &path);
    let count = packets(&fx);
    for _ in 0..3 {
        fx.sync();
    }
    assert_eq!(packet(&fx), (path, bytes, p));
    assert_eq!(packets(&fx), count);
    assert_eq!(requests(&fx).len(), 1);
    assert_eq!(fx.events("changes_requested"), 1);
    assert_no_launches(&fx);
}

#[test]
fn clean_packet_is_attached_before_gate_and_reviewer_launches() {
    // Given a gate that consumes the machine-readable packet from the artifact directory.
    let mut fx = configured();
    let integration = fx.ctx.store.state_dir().join("integration");
    let command = format!(
        "test -n \"$(find '{}' -name '*.json' -print -quit)\"",
        integration.display()
    );
    fx.ctx.store.config.review.gate = Some(vec![command.clone()]);
    fx.ctx.store.save_config().unwrap();
    fx.change("widget.txt", "widget\n");
    fx.done();
    let run = fx.run();
    let head = git(&run.worktree, &["rev-parse", "HEAD"]);

    // When sync runs a clean candidate through validation.
    assert_eq!(fx.sync().status, Status::Review, "{}", fx.reason());

    // Then the gate observed the packet and the reviewer follows the passing gate.
    let (_, _, p) = packet(&fx);
    assert_eq!(p["key"], key(&fx, &head, &run.base_sha, "preflight_clean"));
    assert_eq!(p["dispatch_base"], run.base_sha);
    assert_eq!(p["merge_base"], run.base_sha);
    assert_eq!(p["dispatch_history"], json!([]));
    assert_eq!(p["missing_from_candidate"], json!([]));
    assert_eq!(p["conflict_paths"], json!([]));
    assert_eq!(p["candidate_changed_paths"], json!(["widget.txt"]));
    assert_eq!(p["main_changed_paths"], json!([]));
    assert_eq!(p["overlaps"], json!([]));
    assert_eq!(p["gate_commands"], json!([command]));
    assert_eq!(p["git_details"], Value::Null);
    assert_eq!(p["reviewer_note"], Value::Null);
    assert_eq!(fx.run().gate.unwrap().result, GateResult::Pass);
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
    assert_eq!(fx.events("gate_started"), 1);
    assert_eq!(fx.events("review_started"), 1);
    assert!(requests(&fx).is_empty());
}

#[test]
fn same_candidate_is_not_requested_again_after_done_but_main_movement_refreshes() {
    // Given an integration request already delivered for these exact commits.
    let fx = configured();
    fx.change("README.md", "candidate\n");
    fx.done();
    let head = git(&fx.run().worktree, &["rev-parse", "HEAD"]);
    main_change(&fx, "README.md", "main one\n", "first conflict");
    fx.sync();
    let (old_path, old_bytes, _) = packet(&fx);
    let old_count = packets(&fx);

    // When the unchanged candidate is resubmitted, then the same key consumes no new round.
    fx.done();
    assert_eq!(fx.sync().status, Status::Review, "{}", fx.reason());
    assert_eq!(fx.task().rounds, 1);
    assert_eq!(requests(&fx).len(), 1);
    assert_eq!(packets(&fx), old_count);
    assert_eq!(std::fs::read(&old_path).unwrap(), old_bytes);
    assert_no_launches(&fx);

    // When main moves while the resubmitted candidate waits, the new identity permits one request.
    let base = main_change(&fx, "README.md", "main two\n", "second conflict");
    assert_eq!(fx.sync().status, Status::Running, "{}", fx.reason());
    assert_eq!(fx.task().rounds, 2);
    let (path, _, p) = packet(&fx);
    assert_ne!(path, old_path);
    let expected_key = key(&fx, &head, &base, "merge_conflict");
    assert_eq!(p["key"], expected_key);
    assert_request(&fx, &expected_key, &path);
    assert_eq!(requests(&fx).len(), 2);
    assert_eq!(std::fs::read(old_path).unwrap(), old_bytes);
    assert_no_launches(&fx);
}

#[test]
fn three_moving_main_conflicts_use_three_rounds_and_fourth_escalates() {
    // Given a three-round integration repair budget.
    let fx = configured();
    fx.change("README.md", "candidate\n");
    fx.done();
    for round in 1..=4 {
        main_change(
            &fx,
            "README.md",
            &format!("main {round}\n"),
            &format!("move {round}"),
        );
        // When each distinct main SHA conflicts with the same candidate.
        let task = fx.sync();
        // Then exactly the permitted three requests are delivered, and the fourth escalates.
        if round <= 3 {
            assert_eq!(task.status, Status::Running, "{}", fx.reason());
            assert_eq!(task.rounds, round);
            assert_eq!(requests(&fx).len(), usize::try_from(round).unwrap());
            fx.done();
        } else {
            assert_eq!(task.status, Status::NeedsMe, "{}", fx.reason());
            assert_eq!(task.rounds, 3);
            assert_eq!(fx.stage(), Some(AutoStage::Escalated));
            assert_eq!(requests(&fx).len(), 3);
        }
    }
    assert_eq!(fx.events("changes_requested"), 3);
    assert_no_launches(&fx);
}

#[test]
fn dispatch_history_retains_integrated_commits_but_missing_only_tracks_new_main() {
    // Given a worker rebased onto two landed commits after dispatch.
    let fx = configured();
    fx.change("widget.txt", "candidate\n");
    commit(&fx.run().worktree, "worker candidate");
    let dispatch = fx.run().base_sha;
    let first = approved_change(&fx, "First landed", "first.txt", "first\n");
    let second = approved_change(&fx, "Second landed", "second.txt", "second\n");
    let integrated = git(&fx.app, &["rev-parse", "HEAD"]);
    git(&fx.run().worktree, &["rebase", "main"]);
    fx.change("README.md", "worker after rebase\n");
    fx.done();
    let head = git(&fx.run().worktree, &["rev-parse", "HEAD"]);
    let third = approved_change(&fx, "Later landed", "README.md", "later main\n");
    let base = third["sha"].as_str().unwrap();

    // When main moves again after that worker rebase.
    fx.sync();

    // Then historical drift and commits still missing from the candidate remain distinct.
    let (_, _, p) = packet(&fx);
    assert_eq!(p["key"], key(&fx, &head, base, "merge_conflict"));
    assert_eq!(p["dispatch_base"], dispatch);
    assert_eq!(p["merge_base"], integrated);
    assert_eq!(p["dispatch_history"], json!([third, second, first]));
    assert_eq!(p["missing_from_candidate"], json!([third]));
    assert_eq!(p["main_changed_paths"], json!(["README.md"]));
    assert_eq!(
        p["candidate_changed_paths"],
        json!(["README.md", "widget.txt"])
    );
    assert_no_launches(&fx);
}

#[test]
fn gone_worker_retry_carries_the_original_packet_path() {
    // Given a conflict and a worker pane that no longer exists.
    let fx = configured();
    fx.change("README.md", "candidate\n");
    fx.done();
    let original = fx.run();
    let head = git(&original.worktree, &["rev-parse", "HEAD"]);
    let base = main_change(&fx, "README.md", "main\n", "conflict");
    std::fs::write(fx.bin_dir.join("gone"), "w1:p1\n").unwrap();

    // When sync must launch a replacement instead of prompting the lost pane.
    assert_eq!(fx.sync().status, Status::Running, "{}", fx.reason());

    // Then R2 receives the exact existing packet path while its identity still names R1.
    let run = fx.run();
    assert_eq!(run.id, "R2");
    assert_eq!(fx.task().rounds, 1);
    assert_eq!(fx.task().runs[0].outcome, Some(RunOutcome::Superseded));
    let (path, _, p) = packet(&fx);
    let expected_key = json!({
        "task":fx.id,"run":original.id,"head":head,"base":base,"reason":"merge_conflict"
    });
    assert_eq!(p["key"], expected_key);
    assert_request(&fx, &expected_key, &path);
    let prompt = std::fs::read_to_string(
        fx.ctx
            .store
            .prompts_dir()
            .join(format!("{}-{}.md", fx.id, run.id)),
    )
    .unwrap();
    assert!(prompt.contains(path.to_str().unwrap()));
    assert!(fx.herdr_log("prompts.log").is_empty());
    assert_no_launches(&fx);
}

#[test]
fn clean_merge_with_conflicting_intermediate_commit_requests_rebase_and_restores() {
    // Given a candidate whose final tree is merge-clean but whose intermediate commit conflicts.
    let fx = Fx::new(|r| {
        r.accept = Some(Accept::Auto);
        r.max_rounds = Some(3);
        r.remove_worktree = Some(false);
    });
    fx.change("README.md", "intermediate worker edit\n");
    commit(&fx.run().worktree, "intermediate edit");
    fx.change("README.md", "hello\n");
    fx.change("widget.txt", "candidate\n");
    fx.done();
    let head = git(&fx.run().worktree, &["rev-parse", "HEAD"]);
    let tree = git(&fx.run().worktree, &["write-tree"]);
    let base = main_change(&fx, "README.md", "main\n", "main edit");
    assert!(matches!(
        corral_git::merge_preflight(&fx.run().worktree, &base, &head).unwrap(),
        corral_git::MergePreflight::Clean { .. }
    ));

    // When landing replays the intermediate edit.
    assert_eq!(fx.sync().status, Status::Running, "{}", fx.reason());

    // Then this is a repair request, not infrastructure escalation, with no dirty rebase state.
    assert_eq!(fx.task().rounds, 1);
    assert_eq!(fx.stage(), Some(AutoStage::Changes));
    assert_restored(&fx, &head, &tree);
    let (path, _, p) = packet(&fx);
    let expected_key = key(&fx, &head, &base, "rebase_conflict");
    assert_eq!(p["key"], expected_key);
    assert_eq!(p["conflict_paths"], json!(["README.md"]));
    assert_request(&fx, &expected_key, &path);
    assert_no_launches(&fx);
}

#[test]
fn unresolved_integration_base_is_infrastructure_not_a_rebase_request() {
    // Given a finished run whose configured base ref is no longer resolvable.
    let fx = configured();
    fx.change("widget.txt", "candidate\n");
    fx.done();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.current_run_mut().unwrap().base = "missing-integration-base".into();
            Ok(())
        })
        .unwrap();

    // When preflight cannot obtain a valid Git base.
    fx.sync();
    fx.sync();

    // Then the owner receives one escalation, without spending a repair round or launching validation.
    assert_eq!(fx.task().status, Status::NeedsMe, "{}", fx.reason());
    assert_eq!(fx.stage(), Some(AutoStage::Escalated));
    assert_eq!(fx.task().rounds, 0);
    assert!(requests(&fx).is_empty());
    assert_eq!(fx.events("changes_requested"), 0);
    assert_eq!(fx.transitions(), ["escalated"]);
    assert_no_launches(&fx);
}

#[test]
fn fast_forward_policy_returns_complete_base_movement_packet_without_rebasing() {
    // Given a merge-clean candidate under a policy that requires worker integration.
    let fx = Fx::new(|r| {
        r.merge = Some(corral_core::MergeMode::Ff);
        r.accept = Some(Accept::Auto);
    });
    fx.change("widget.txt", "candidate\n");
    fx.done();
    let run = fx.run();
    let head = git(&run.worktree, &["rev-parse", "HEAD"]);
    let base = main_change(&fx, "main.txt", "landed\n", "main moved");

    // When sync cannot fast-forward the worker's old-base candidate.
    assert_eq!(fx.sync().status, Status::Running, "{}", fx.reason());

    // Then its factual packet describes missing work, not a fabricated merge conflict.
    let (path, _, p) = packet(&fx);
    assert_eq!(p["key"], key(&fx, &head, &base, "base_moved"));
    assert_eq!(p["conflict_paths"], json!([]));
    assert_eq!(p["overlaps"], json!([]));
    assert_eq!(p["missing_from_candidate"][0]["sha"], base);
    assert_eq!(git(&run.worktree, &["rev-parse", "HEAD"]), head);
    assert_request(&fx, &p["key"], &path);
}

#[test]
fn moved_main_packet_keeps_reviewer_authored_notes_at_the_reviewed_sha() {
    // Given an approved candidate and exact reviewer-authored multiline notes.
    let fx = configured();
    fx.change("README.md", "reviewed candidate\n");
    fx.done();
    fx.sync();
    let reviewed = fx.run().review.unwrap();
    let notes = "Keep the parser boundary explicit.\nDo not silently change the accepted format.\n";
    fx.verdict(Verdict::Approve, notes);
    let base = main_change(&fx, "README.md", "main moved\n", "post-review conflict");

    // When the reviewed candidate conflicts with newly moved main.
    fx.sync();

    // Then the packet attributes verbatim notes to that reviewer and full candidate SHA.
    let (_, _, p) = packet(&fx);
    assert_eq!(p["key"], key(&fx, &reviewed.sha, &base, "merge_conflict"));
    assert_eq!(
        p["reviewer_note"],
        json!({
            "reviewer":reviewed.reviewer,"sha":reviewed.sha,"notes":notes
        })
    );
    assert_eq!(fx.task().status, Status::Running, "{}", fx.reason());
    assert_eq!(fx.task().rounds, 1);
    assert_eq!(fx.events("gate_started"), 1);
    assert_eq!(fx.events("review_started"), 1);
}
