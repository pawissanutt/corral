//! Real reviewer session reuse reads the previous child's keyed native generation.
use super::*;
use corral_core::{
    RunOutcome,
    work::{self, Counter, HotScope},
};

#[test]
fn reviewer_reuses_exact_native_conversation_over_decoy_and_retained_history() {
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let (_tmp, ctx, mut task, mut parent, _) = super::tests::fixture("omo");
        let mut previous = parent.clone();
        previous.id = "R1-review-live".into();
        previous.harness = Some(corral_core::PI_NATIVE_HARNESS.into());
        previous.outcome = Some(RunOutcome::Done);
        previous.session = Some(fresh(&ctx, &task, &previous, true).unwrap());
        let directory = &previous.session.as_ref().unwrap().directory;
        std::fs::create_dir_all(directory).unwrap();
        let transcript = directory.join("native.jsonl");
        let decoy = directory.join("decoy.jsonl");
        for (path, id) in [
            (&transcript, "native-exact"),
            (&decoy, "wrong-conversation"),
        ] {
            std::fs::write(
                path,
                format!(
                    "{}\n",
                    json!({"type":"session", "id":id, "cwd":previous.worktree})
                ),
            )
            .unwrap();
        }
        let session = previous.session.as_mut().unwrap();
        session.id = Some("native-exact".into());
        session.transcript = Some(decoy);
        let mut native = corral_core::agent::AgentRunState::new(serde_json::from_value(json!({
            "role":"reviewer", "store":"fixture", "project":"app", "generation":10000, "health":"healthy"
        })).unwrap());
        for n in 1..=2 * scale {
            native.generations.push(
                serde_json::from_value(json!({"n":n,
                "bind_nonce_sha256":"0".repeat(64), "pane":"old", "created":parent.started,
                "fenced":{"reason":"replaced", "at":parent.started}}))
                .unwrap(),
            );
        }
        native.generations.push(
            serde_json::from_value(json!({"n":10000,
            "bind_nonce_sha256":"1".repeat(64), "pane":"current", "created":parent.started,
            "bound":{"host_cap_sha256":"2".repeat(64), "ctl_cap_sha256":"3".repeat(64),
                "session_id":"native-exact", "session_file":transcript, "pid":std::process::id(),
                "start_time":"fixture", "request_id":"hello", "at":parent.started}}))
            .unwrap(),
        );
        previous.agent_host = Some(Box::new(native));
        // Each retained review is a sibling, not a recursive copy of all earlier siblings.
        let template = parent.clone();
        for n in 0..2 * scale {
            let mut old = template.clone();
            old.id = format!("old-review-{n}");
            old.session = None;
            old.outcome = Some(RunOutcome::Done);
            parent.review_runs.push(old);
        }
        parent.review_runs.push(previous.clone());
        task.runs.push(parent);
        ctx.store.save(&task).unwrap();
        let head = ctx.store.head(&task.id).unwrap();
        let parent = head.current_run.as_deref().unwrap();
        let mut child = previous;
        child.id = "new-review".into();
        child.session = None;
        child.agent_host = None;
        child.outcome = None;
        let before = work::snapshot();
        let hot = HotScope::enter();
        let selected = reviewer_head(&ctx, &head, parent, &child).unwrap();
        let delta = work::snapshot().since(&before);
        drop(hot);
        delta.check_identity().unwrap();
        assert_eq!(selected.action, SessionAction::Resumed);
        assert_eq!(selected.id.as_deref(), Some("native-exact"));
        assert_eq!(selected.transcript.as_ref(), Some(&transcript));
        assert!(selected.diagnostic.is_none());
        assert_eq!(delta.cells(Counter::FullTaskHydrations).total, 0);
        measured.push(Counter::ALL.map(|counter| delta.cells(counter).unattributed));
        // The legacy full input API projects the same actual native evidence, too.
        let full = ctx.store.get(&task.id).unwrap();
        let selected_full = reviewer(&ctx, &full, full.current_run().unwrap(), &child).unwrap();
        assert_eq!(selected_full, selected);
    }
    assert_eq!(
        measured[0], measured[1],
        "every reviewer selection counter is history-independent"
    );
}
