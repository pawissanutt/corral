use super::*;

fn awaiting_verdict(root: &Path, store: &Store, panes: &mut Panes, id: &str) {
    checkpoint(root, id);
    ok(root, &["done", id, "--summary", SUMMARY]);
    ok(root, &["gate", id, "--wait"]);
    // Keep both exact candidates authorized when the peer joins this fixture;
    // otherwise a scope refusal would hide the owner-ready indexing regression.
    let preview = store
        .preview_operating_policy(&corral_core::OperatingSelection {
            tasks: store
                .list()
                .unwrap()
                .into_iter()
                .map(|task| task.id)
                .collect(),
            authority: corral_core::OperatingAuthority {
                dispatch: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    store
        .confirm_operating_policy(&preview, "fixture owner", jiff::Timestamp::now())
        .unwrap();
    ok(root, &["sync"]);
    let task = store.get(id).unwrap();
    let run = task.current_run().unwrap();
    panes
        .ids
        .push(run.gate.as_ref().unwrap().pane.clone().unwrap());
    panes
        .ids
        .push(run.review.as_ref().unwrap().pane.clone().unwrap());
}

#[test]
fn owner_ready_sync_does_not_hold_approval_guard_while_indexing() {
    // Given a genuinely gated and reviewed candidate waiting for owner approval.
    let Some((tmp, store, mut panes, id)) = live_fixture(Accept::Owner) else {
        return;
    };
    let root = tmp.path();
    awaiting_verdict(root, &store, &mut panes, &id);
    ok(
        root,
        &["verdict", &id, "approve", "--notes", "Fixture reviewed."],
    );
    ok(root, &["sync"]);
    assert_eq!(
        store
            .get(&id)
            .unwrap()
            .current_run()
            .unwrap()
            .autopilot
            .as_ref()
            .unwrap()
            .stage,
        AutoStage::Reviewed
    );
    let peer = String::from_utf8(ok(root, &["new", "Peer review", "-p", "app", "--ready"]).stdout)
        .unwrap()
        .trim()
        .to_string();
    ok(root, &["dispatch", &peer, "--profile", "generic-worker"]);
    let peer_worktree = store
        .get(&peer)
        .unwrap()
        .current_run()
        .unwrap()
        .worktree
        .clone();
    std::fs::write(peer_worktree.join("widget.txt"), "peer\n").unwrap();
    git(&peer_worktree, &["add", "-A"]);
    git(&peer_worktree, &["commit", "-q", "-m", "peer widget"]);
    awaiting_verdict(root, &store, &mut panes, &peer);

    // When concurrent approval, dirty-checkout refusal and blocked sync are ordered by syscalls.
    let output = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/queue_support/approval_busy.py"
        ))
        .arg(env!("CARGO_BIN_EXE_corral"))
        .arg(root)
        .arg(&id)
        .arg(&peer)
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    println!("{}", String::from_utf8_lossy(&output.stdout));

    // Then unrelated index work cannot masquerade as an active approval.
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(store.get(&id).unwrap().status, Status::Done);
    drop(panes);
    tmp.close().unwrap();
    println!("APPROVAL_BUSY_CLEANUP: fixture panes closed; temporary store and repository removed");
}
