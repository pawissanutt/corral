use super::*;

#[test]
fn isolated_approve_rebases_waits_for_gate_carries_and_lands_in_one_command() {
    // Given a real CLI review and approval, followed by unrelated main movement.
    let Some((tmp, store, mut panes, id)) = live_fixture(Accept::Owner) else {
        return;
    };
    let root = tmp.path();
    checkpoint(root, &id);
    ok(root, &["done", &id, "--summary", SUMMARY]);
    ok(root, &["gate", &id, "--wait"]);
    let task = store.get(&id).unwrap();
    panes.ids.push(
        task.current_run()
            .unwrap()
            .gate
            .as_ref()
            .unwrap()
            .pane
            .clone()
            .unwrap(),
    );
    ok(root, &["review-run", &id]);
    let task = store.get(&id).unwrap();
    let review = task.current_run().unwrap().review.as_ref().unwrap();
    panes.ids.push(review.pane.clone().unwrap());
    let old_sha = review.sha.clone();
    ok(
        root,
        &[
            "verdict",
            &id,
            "approve",
            "--notes",
            "Reviewed unchanged widget patch.",
        ],
    );
    let app = root.join("app");
    std::fs::write(app.join("unrelated.txt"), "main moved\n").unwrap();
    git(&app, &["add", "--", "unrelated.txt"]);
    git(&app, &["commit", "-q", "-m", "another task landed"]);
    let base = git(&app, &["rev-parse", "HEAD"]);

    // When one real approve command waits for a real gate in corral-test.
    let output = ok(root, &["approve", &id, "--keep-worktree", "--keep-branch"]);

    // Then the gate and carried receipt authorize only the integrated candidate.
    let task = store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    let gate = run.gate.as_ref().unwrap();
    panes.ids.push(gate.pane.clone().unwrap());
    let review = run.review.as_ref().unwrap();
    assert_eq!(task.status, Status::Done);
    assert_eq!(gate.result, corral_core::GateResult::Pass);
    assert_eq!(gate.identity, review.identity);
    assert_eq!(gate.identity.as_ref().unwrap().integration_base, base);
    assert_ne!(old_sha, gate.sha);
    assert_eq!(git(&app, &["rev-parse", "HEAD"]), gate.sha);
    assert_eq!(
        std::fs::read_to_string(app.join("widget.txt")).unwrap(),
        "observed\n"
    );
    let events = store.events().unwrap();
    assert_eq!(
        events.iter().filter(|e| e.kind == "review_started").count(),
        1
    );
    let carry = events.iter().find(|e| e.kind == "verdict_carried").unwrap();
    assert_eq!(carry.detail["old_sha"], old_sha);
    assert_eq!(carry.detail["new_sha"], gate.sha);
    println!(
        "LIVE_VERDICT_CARRY {}",
        json!({
            "session":"corral-test", "stdout":String::from_utf8_lossy(&output.stdout),
            "old_sha":old_sha, "base":base, "gate":gate, "review":review, "event":carry
        })
    );
}
