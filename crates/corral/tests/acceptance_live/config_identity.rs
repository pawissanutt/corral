use super::*;

#[test]
fn isolated_autopilot_toggle_preserves_receipts_but_gate_policy_edit_is_stale() {
    for change_policy in [false, true] {
        // Given a real gate and approved reviewer receipt in corral-test.
        let Some((tmp, mut store, mut panes, id)) = live_fixture(Accept::Owner) else {
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
            &["verdict", &id, "approve", "--notes", "Reviewed widget."],
        );
        let before = store.get(&id).unwrap().current_run().unwrap().clone();

        // When the real CLI disables autopilot, optionally followed by a recipe edit.
        ok(root, &["autopilot", "off"]);
        if change_policy {
            store = Store::open(root).unwrap();
            store.config.review.gate = Some(vec!["false".into()]);
            store.save_config().unwrap();
        }
        let output = cli(root, &["approve", &id, "--keep-worktree", "--keep-branch"]);

        // Then only the runtime edit permits landing using the exact same receipts.
        let task = store.get(&id).unwrap();
        let after = task.current_run().unwrap();
        let gate = after.gate.as_ref().unwrap();
        panes.ids.push(gate.pane.clone().unwrap());
        if change_policy {
            assert!(!output.status.success());
            assert_eq!(gate.result, corral_core::GateResult::Fail);
            assert_ne!(
                gate.identity.as_ref().unwrap().policy_digest,
                before
                    .gate
                    .as_ref()
                    .unwrap()
                    .identity
                    .as_ref()
                    .unwrap()
                    .policy_digest
            );
            assert_eq!(after.review, before.review);
            assert_eq!(task.status, Status::Review);
            assert!(!root.join("app/widget.txt").exists());
        } else {
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(task.status, Status::Done);
            assert_eq!(after.gate, before.gate);
            assert_eq!(after.review, before.review);
            assert!(root.join("app/widget.txt").is_file());
        }
        println!(
            "LIVE_CONFIG_IDENTITY {}",
            json!({
                "session": "corral-test", "changed_gate_policy": change_policy,
                "approved": output.status.success(), "gate": after.gate,
                "review": after.review, "stderr": String::from_utf8_lossy(&output.stderr)
            })
        );
    }
}
