use super::*;
use corral_core::ReviewRecord;
use std::os::unix::net::UnixDatagram;
use std::time::{Duration, Instant};

const RESPONSE: &str = r#"import json
import socket
import sys
from pathlib import Path

target, launch_path = Path(sys.argv[1]), Path(sys.argv[2])
reply_path = launch_path.with_suffix(".reply")
with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as channel:
    channel.bind(str(reply_path))
    channel.settimeout(10)
    channel.sendto(b"review-launch", str(launch_path))
    timestamp = channel.recv(128).decode("ascii")
reply_path.unlink()
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
    let socket = ctx.store.root().join("review-launch.sock");
    reviewer_profile(
        ctx,
        format!(
            "python3 '{}' '{}' '{}' '{}'",
            script.display(),
            path.display(),
            socket.display(),
            worktree.display()
        ),
    );
}

pub(super) fn launch_transcript_reviewer(ctx: &Ctx, id: &str) -> ReviewRecord {
    // Subscribe before submission. The fake pane_run cannot finish until the
    // prepared accounting window has been observed and supplied to its transcript.
    let path = ctx.store.root().join("review-launch.sock");
    let launch = UnixDatagram::bind(&path).unwrap();
    let (record, prepared_at) = std::thread::scope(|scope| {
        let review = scope.spawn(|| corral_ops::review_run(ctx, id, Some("rev")));
        let mut message = [0; 32];
        let deadline = Instant::now() + Duration::from_secs(10);
        let (length, sender) = loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("reviewer launch notification timed out");
            launch.set_read_timeout(Some(remaining)).unwrap();
            match launch.recv_from(&mut message) {
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                result => break result.unwrap(),
            }
        };
        assert_eq!(&message[..length], b"review-launch");
        let task = ctx.store.get(id).unwrap();
        let run = task.current_run().unwrap();
        let prepared = run.review_runs.last().unwrap();
        assert_eq!(prepared.started, run.review.as_ref().unwrap().at);
        launch
            .send_to(
                prepared.started.to_string().as_bytes(),
                sender.as_pathname().unwrap(),
            )
            .unwrap();
        (review.join().unwrap().unwrap(), prepared.started)
    });
    drop(launch);
    std::fs::remove_file(path).unwrap();
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
