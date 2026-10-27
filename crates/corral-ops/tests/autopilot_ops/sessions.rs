use super::*;
use corral_core::{RoleSession, SessionAction, SessionPolicy};
use serde_json::json;

#[test]
fn session_only_configuration_does_not_activate_automatic_review() {
    for reviewer in [false, true] {
        // Given only a fresh session setting, without an automatic review policy.
        let fx = Fx::new(|policy| {
            if reviewer {
                policy.reviewer_session = Some(SessionPolicy::Fresh);
            } else {
                policy.worker_session = Some(SessionPolicy::Fresh);
            }
        });
        fx.change("widget.txt", "w\n");
        fx.done();

        // When sync sees the completed worker.
        let task = fx.sync();

        // Then the existing manual Review flow remains, without starting or requiring a gate.
        assert_eq!(task.status, Status::Review);
        assert_eq!(fx.stage(), None);
        assert_eq!(fx.events("gate_started"), 0);
    }
}

#[test]
fn automatic_missing_worker_context_escalates_once_instead_of_retrying_empty() {
    // Given a default-reuse worker whose pane and conversation are unavailable.
    let fx = Fx::new(|r| {
        r.gate = gate(&["test -f fixed.txt"]);
        r.accept = Some(Accept::Auto);
    });
    fx.change("widget.txt", "w\n");
    fx.done();
    std::fs::write(fx.bin_dir.join("gone"), "w1:p1\n").unwrap();

    // When the automatic loop needs a repair.
    let task = fx.sync();

    // Then it holds for the owner with one diagnostic and no new run or repeated launch.
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(task.runs.len(), 1);
    assert_eq!(task.rounds, 0);
    assert!(fx.reason().contains("worker context cannot be resumed"));
    let launches = fx.herdr_log("runs.log");
    fx.sync();
    assert_eq!(fx.herdr_log("runs.log"), launches);
}

#[test]
fn automatic_gone_worker_resumes_its_exact_session_in_the_same_run() {
    // Given a finished worker with a recorded compatible fake transcript.
    let mut fx = Fx::new(|r| {
        r.gate = gate(&["test -f fixed.txt"]);
        r.accept = Some(Accept::Auto);
    });
    let script = fx.ctx.store.root().join("resume-harness.py");
    std::fs::write(&script, "import json, sys\nfrom pathlib import Path\np = Path(sys.argv[sys.argv.index('--session') + 1])\nassert json.loads(p.read_text().splitlines()[0])['id'] == 'worker-context'\nwith p.open('a') as f: f.write('{\"type\":\"message\"}\\n')\n").unwrap();
    fx.ctx.store.config.harnesses.insert("owned".into(), serde_json::from_value(json!({
        "flags":"omo", "command":format!("python3 '{}' {{model_args}} {{prompt_ref}}", script.display())
    })).unwrap());
    fx.ctx.store.config.profiles.insert(
        "fake".into(),
        serde_json::from_value(json!({"harness":"owned"})).unwrap(),
    );
    fx.ctx.store.save_config().unwrap();
    let directory = fx.ctx.store.state_dir().join("worker-context");
    std::fs::create_dir_all(&directory).unwrap();
    let transcript = directory.join("conversation.jsonl");
    std::fs::write(
        &transcript,
        format!(
            "{}\n",
            json!({
                "type":"session", "id":"worker-context", "cwd":fx.run().worktree
            })
        ),
    )
    .unwrap();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            let run = task.current_run_mut().unwrap();
            run.execution = None;
            run.harness = Some("owned".into());
            run.session = Some(RoleSession {
                policy: SessionPolicy::Reuse,
                id: Some("worker-context".into()),
                transcript: Some(transcript.clone()),
                directory,
                harness: "owned".into(),
                model: run.model.clone(),
                vendor: run.vendor.clone(),
                action: SessionAction::New,
                diagnostic: None,
            });
            Ok(())
        })
        .unwrap();
    fx.change("widget.txt", "w\n");
    fx.done();
    std::fs::write(fx.bin_dir.join("gone"), "w1:p1\n").unwrap();

    // When the automatic review loop requests the repair after the pane disappears.
    let task = fx.sync();

    // Then the same task run resumes the exact conversation under the existing repair authorization.
    assert_eq!(task.status, Status::Running);
    assert_eq!(task.runs.len(), 1);
    assert_eq!(task.rounds, 1);
    let session = task.current_run().unwrap().session.as_ref().unwrap();
    assert_eq!(session.id.as_deref(), Some("worker-context"));
    assert_eq!(session.action, SessionAction::Resumed);
    assert_eq!(
        std::fs::read_to_string(transcript).unwrap().lines().count(),
        2
    );
}
