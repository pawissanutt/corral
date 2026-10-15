use super::*;

#[test]
fn isolated_sync_delivers_changes_after_main_moves_without_regating() {
    // Given a recorded changes verdict from the real CLI and a moved main.
    let Some((tmp, store, mut panes, id)) = live_fixture(Accept::Auto) else {
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
    ok(root, &["sync"]);
    let task = store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    assert_eq!(run.autopilot.as_ref().unwrap().stage, AutoStage::Reviewing);
    let review = run.review.as_ref().unwrap();
    panes.ids.push(review.pane.clone().unwrap());
    let head = review.sha.clone();
    let notes = "Reproduce P2 with an empty widget.\nReject it before indexing.";
    ok(root, &["verdict", &id, "changes", "--notes", notes]);
    let app = root.join("app");
    std::fs::write(app.join("unrelated.txt"), "another task\n").unwrap();
    git(&app, &["add", "--", "unrelated.txt"]);
    git(&app, &["commit", "-q", "-m", "another task landed"]);
    let base = git(&app, &["rev-parse", "main"]);
    assert_ne!(base, review.identity.as_ref().unwrap().integration_base);
    let before = store.events().unwrap();

    // When a single real sync processes the now-stale verification identity.
    let output = ok(root, &["sync"]);

    // Then the real isolated transport delivers changes without a gate or review launch.
    let task = store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    assert_eq!(task.status, Status::Running);
    assert_eq!(task.rounds, 1);
    assert_eq!(run.autopilot.as_ref().unwrap().stage, AutoStage::Changes);
    assert_eq!(git(&run.worktree, &["rev-parse", "HEAD"]), head);
    assert!(run.pending_reply.is_none());
    let artifact = task
        .artifacts
        .iter()
        .find(|a| a.title == "Autopilot changes (round 1)")
        .unwrap();
    assert!(
        std::fs::read_to_string(store.dir().join(&artifact.path))
            .unwrap()
            .contains(notes)
    );
    let events = store.events().unwrap();
    let delivered = events
        .iter()
        .find(|e| e.kind == "changes_requested")
        .unwrap();
    assert_eq!(delivered.detail["submitted"], true);
    assert!(
        (1..=u64::from(corral_herdr::ENTER_ATTEMPTS))
            .contains(&delivered.detail["attempts"].as_u64().unwrap()),
        "{delivered:?}"
    );
    for kind in ["gate_started", "review_started", "integration_request"] {
        assert_eq!(
            events.iter().filter(|e| e.kind == kind).count(),
            before.iter().filter(|e| e.kind == kind).count(),
            "unexpected {kind}"
        );
    }
    println!(
        "LIVE_CHANGES_AFTER_MAIN_MOVED {}",
        json!({
            "session":"corral-test", "status":task.status, "rounds":task.rounds,
            "candidate":head, "main":base, "delivered":delivered,
            "notes_artifact":artifact.path, "new_gates":0, "new_reviews":0,
            "stdout":String::from_utf8_lossy(&output.stdout)
        })
    );
}
