use super::*;

#[test]
fn isolated_sync_rebases_before_claiming_the_first_gate() {
    // Given a committed candidate and main movement after done, before admission.
    let Some((tmp, mut store, mut panes, id)) = live_fixture(Accept::Auto) else {
        return;
    };
    let root = tmp.path();
    store.config.review.gate = Some(vec![
        "git merge-base --is-ancestor main HEAD".into(),
        "test -f widget.txt && test -f unrelated.txt".into(),
    ]);
    store.save_config().unwrap();
    checkpoint(root, &id);
    ok(root, &["done", &id, "--summary", SUMMARY]);
    let original = store.get(&id).unwrap().current_run().unwrap().clone();
    let old_head = git(&original.worktree, &["rev-parse", "HEAD"]);
    let app = root.join("app");
    std::fs::write(app.join("unrelated.txt"), "landed\n").unwrap();
    git(&app, &["add", "--", "unrelated.txt"]);
    git(&app, &["commit", "-q", "-m", "another task landed"]);
    let base = git(&app, &["rev-parse", "main"]);
    grant(root, &id);

    // When the real sync CLI admits its first gate in the isolated server.
    let output = ok(root, &["sync"]);

    // Then the first attempt already includes main, and its real commands pass.
    let task = store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    let gate = run.gate.as_ref().unwrap();
    panes.ids.push(gate.pane.clone().unwrap());
    assert_ne!(gate.sha, old_head);
    assert_eq!(gate.identity.as_ref().unwrap().integration_base, base);
    assert_eq!(git(&run.worktree, &["merge-base", &base, &gate.sha]), base);
    let ctx = corral_ops::Ctx {
        resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
        store: Store::open(root).unwrap(),
        herdr: panes.herdr.clone(),
    };
    let finished = corral_ops::gate_wait(&ctx, &id).unwrap();
    assert_eq!(finished.result, corral_core::GateResult::Pass);
    let events = store.events().unwrap();
    let rebase = events
        .iter()
        .position(|e| e.kind == "candidate_rebased")
        .unwrap();
    let started = events
        .iter()
        .position(|e| e.kind == "gate_started")
        .unwrap();
    assert!(rebase < started);
    assert_eq!(
        events.iter().filter(|e| e.kind == "gate_started").count(),
        1
    );
    ok(root, &["sync"]);
    let task = store.get(&id).unwrap();
    panes.ids.push(
        task.current_run()
            .unwrap()
            .review
            .as_ref()
            .unwrap()
            .pane
            .clone()
            .unwrap(),
    );
    ok(
        root,
        &[
            "verdict",
            &id,
            "approve",
            "--notes",
            "Integrated patch reviewed.",
        ],
    );
    ok(root, &["sync"]);
    assert_eq!(store.get(&id).unwrap().status, Status::Done);
    assert_eq!(git(&app, &["rev-parse", "main"]), finished.sha);
    assert_eq!(
        store
            .events()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "gate_started")
            .count(),
        1
    );
    println!(
        "LIVE_REBASE_BEFORE_GATE {}",
        json!({
            "session":"corral-test", "original_head":old_head, "base":base,
            "gate":finished, "gate_attempts":1, "status":"done",
            "rebase_before_claim":true, "stdout":String::from_utf8_lossy(&output.stdout)
        })
    );
}
