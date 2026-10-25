use super::*;
use corral_core::ReviewRecord;
use std::sync::{Arc, Mutex};

const RESPONSE: &str = r#"import json
import sys
from pathlib import Path

target, launch_path = Path(sys.argv[1]), Path(sys.argv[2])
timestamp = launch_path.read_text()
with target.open("w") as transcript:
    transcript.write(json.dumps({"type": "session", "cwd": sys.argv[3]}) + "\n")
    transcript.write(json.dumps({
        "type": "message", "timestamp": timestamp,
        "message": {"role": "assistant", "content": "First review",
                    "usage": {"input": 13, "output": 7, "cost": 0.25}},
    }) + "\n")
"#;

pub(super) fn transcript_reviewer_profile(ctx: &mut Ctx, worktree: &Path) {
    let script = ctx.store.root().join("review-transcript.py");
    std::fs::write(&script, RESPONSE).unwrap();
    let path = transcript_path(ctx, worktree);
    let launch_at = ctx.store.root().join("review-launch.at");
    reviewer_profile(
        ctx,
        format!(
            "python3 '{}' '{}' '{}' '{}'",
            script.display(),
            path.display(),
            launch_at.display(),
            worktree.display()
        ),
    );
}

pub(super) fn launch_transcript_reviewer(ctx: &Ctx, id: &str) -> ReviewRecord {
    // Subscribe before submission, at the commit immediately before pane_run.
    // Preparation may take arbitrary time; it is not the behavior under test.
    let path = ctx.store.root().join("review-launch.at");
    let store = ctx.store.clone();
    let task_id = id.to_owned();
    let prepared_at = Arc::new(Mutex::new(None));
    let observed = Arc::clone(&prepared_at);
    let launching = Ctx {
        store: ctx.store.with_submission_barrier(Arc::new(move |point| {
            if point == "authorization_committed" {
                let task = store.get(&task_id).unwrap();
                let run = task.current_run().unwrap();
                let prepared = run.review_runs.last().unwrap();
                assert_eq!(prepared.started, run.review.as_ref().unwrap().at);
                assert!(observed.lock().unwrap().replace(prepared.started).is_none());
                std::fs::write(&path, prepared.started.to_string()).unwrap();
            }
        })),
        ..ctx.clone()
    };
    let record = corral_ops::review_run(&launching, id, Some("rev")).unwrap();
    let prepared_at = prepared_at.lock().unwrap().unwrap();
    let saved = ctx.store.get(id).unwrap();
    assert_eq!(
        saved
            .current_run()
            .unwrap()
            .review_runs
            .last()
            .unwrap()
            .started,
        prepared_at,
        "submission must not move the prepared reviewer boundary"
    );
    assert_eq!(record.at, prepared_at);
    record
}
