//! Concrete executor revocation at an acknowledged IPC boundary before final submission.
#![cfg(unix)]

#[path = "operating_launch/fixture.rs"]
mod fixture;
#[path = "operating_launch/planning.rs"]
mod planning;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

use corral_core::{
    AuthorizedSubmission, LaunchStage, OperatingActionKind, OperatingAuthority, OperatingOrigin,
    OperatingSelection, Status, Store, SubmissionDisposition,
};
use fixture::{Fx, now};
use rustix::event::{PollFd, PollFlags, Timespec, poll};

struct Barrier {
    listener: UnixListener,
    path: PathBuf,
}

impl Barrier {
    fn subscribe(fx: &Fx) -> Self {
        let path = fx.bin.join("barrier.sock");
        let listener = UnixListener::bind(&path).unwrap();
        Self { listener, path }
    }

    fn arrival(&self) -> UnixStream {
        let mut descriptors = [PollFd::new(&self.listener, PollFlags::IN)];
        assert_eq!(
            poll(
                &mut descriptors,
                Some(&Timespec {
                    tv_sec: 10,
                    tv_nsec: 0
                })
            )
            .unwrap(),
            1,
            "executor never reached its prepared IPC barrier"
        );
        let (stream, _) = self.listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut request = String::new();
        BufReader::new(&stream).read_line(&mut request).unwrap();
        assert!(request.contains("report-metadata"));
        stream
    }
}

impl Drop for Barrier {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).unwrap();
    }
}

#[derive(Clone, Copy, Debug)]
enum Revoke {
    Pause,
    Assist,
    Member,
    Dispatch,
}

fn revoke(fx: &Fx, included: &str, keep: &str, change: Revoke) {
    let store = Store::open(fx.temp.path()).unwrap();
    match change {
        Revoke::Pause => {
            store.pause_operating_policy(true).unwrap();
        }
        Revoke::Assist => {
            store.assist_operating_policy().unwrap();
        }
        Revoke::Member | Revoke::Dispatch => {
            let selection = OperatingSelection {
                tasks: if matches!(change, Revoke::Member) {
                    [keep.to_owned()].into()
                } else {
                    [included.to_owned(), keep.to_owned()].into()
                },
                authority: OperatingAuthority {
                    dispatch: !matches!(change, Revoke::Dispatch),
                    accept: true,
                    plan_accept: true,
                },
                ..Default::default()
            };
            let preview = store.preview_operating_policy(&selection).unwrap();
            store
                .confirm_operating_policy(&preview, "fixture owner", now())
                .unwrap();
        }
    }
}

fn authorization(fx: &Fx, task: &str, kind: OperatingActionKind) -> AuthorizedSubmission {
    let mut matching = std::fs::read_dir(fx.ctx.store.state_dir().join("action-authorizations"))
        .unwrap()
        .map(|entry| {
            serde_json::from_slice::<AuthorizedSubmission>(
                &std::fs::read(entry.unwrap().path()).unwrap(),
            )
            .unwrap()
        })
        .filter(|record| {
            record.authorization.action.task == task
                && record.authorization.action.kind == kind
                && matches!(
                    record.authorization.origin,
                    OperatingOrigin::Automatic { .. }
                )
        })
        .collect::<Vec<_>>();
    assert_eq!(matching.len(), 1);
    matching.pop().unwrap()
}

#[test]
fn every_coding_revocation_retains_preparation_without_submitting_or_stopping_running_work() {
    for change in [
        Revoke::Pause,
        Revoke::Assist,
        Revoke::Member,
        Revoke::Dispatch,
    ] {
        // Given a finite grant and an independent, genuinely submitted gate.
        let mut fx = Fx::new();
        let task = fx.task("automatic coding", Status::Ready);
        let running = fx.task("already running", Status::Ready);
        fx.start(&running.id);
        std::fs::write(fx.bin.join("hold-gates"), "").unwrap();
        let gate = corral_ops::gate_start(&fx.ctx, &running.id).unwrap();
        let running_paths = corral_ops::gate_paths(&fx.ctx, &running.id, "R1");
        fx.grant(&[&task.id, &running.id]);
        let before = fx.records("submissions.jsonl").len();
        let barrier = Barrier::subscribe(&fx);
        std::thread::scope(|scope| {
            // Arrival and release subscriptions exist before sync triggers the executor.
            let worker = scope.spawn(|| corral_ops::sync_at(&fx.fresh(), now()).unwrap());
            let mut release = barrier.arrival();
            let prepared = fx.ctx.store.get(&task.id).unwrap();
            assert_eq!(prepared.launch.as_ref().unwrap().stage, LaunchStage::Pane);
            let original = authorization(&fx, &task.id, OperatingActionKind::Dispatch);
            assert!(matches!(
                original.authorization.origin,
                OperatingOrigin::Automatic { .. }
            ));
            assert_eq!(original.disposition, SubmissionDisposition::Prepared);
            // When a second context commits revocation before releasing final submission.
            revoke(&fx, &task.id, &running.id, change);
            release.write_all(b"R").unwrap();
            worker.join().unwrap();
            // Then exact preparation, identity and definite refusal survive.
            let refused = fx.ctx.store.get(&task.id).unwrap();
            let claim = refused.launch.as_ref().unwrap();
            assert_eq!(claim.stage, LaunchStage::Pane);
            assert!(!claim.reserved);
            assert!(
                claim
                    .error
                    .as_ref()
                    .unwrap()
                    .contains("authorization refused")
            );
            let run = refused.current_run().unwrap();
            assert_eq!(run.id, "R1");
            assert_eq!(run.herdr, prepared.current_run().unwrap().herdr);
            assert!(run.worktree.is_dir());
            assert!(
                fx.ctx
                    .store
                    .prompts_dir()
                    .join(format!("{}-R1.md", task.id))
                    .is_file()
            );
            let record = authorization(&fx, &task.id, OperatingActionKind::Dispatch);
            assert_eq!(record.authorization, original.authorization);
            assert!(matches!(
                record.disposition,
                SubmissionDisposition::AuthorizationRefused { .. }
            ));
            assert_eq!(fx.records("submissions.jsonl").len(), before);
        });
        drop(barrier);
        // Produce the independent gate's exact receipt while new actions remain revoked.
        let result = std::process::Command::new("bash")
            .arg(&running_paths.script)
            .env("CORRAL_ROOT", fx.temp.path())
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        corral_ops::sync_at(&fx.fresh(), now()).unwrap();
        let collected = fx.ctx.store.get(&running.id).unwrap();
        assert_eq!(
            collected
                .current_run()
                .unwrap()
                .gate
                .as_ref()
                .unwrap()
                .result,
            corral_core::GateResult::Pass
        );
        assert_eq!(
            collected.current_run().unwrap().gate.as_ref().unwrap().pane,
            gate.pane
        );
        assert!(fx.records("closed.jsonl").is_empty());
        assert_eq!(fx.records("submissions.jsonl").len(), before);
        assert_eq!(fx.ctx.store.get(&task.id).unwrap().runs.len(), 1);

        // Explicitly restoring the same identities re-admits exactly the prepared action.
        fx.grant(&[&task.id, &running.id]);
        corral_ops::sync_at(&fx.fresh(), now()).unwrap();
        assert_eq!(fx.ctx.store.get(&task.id).unwrap().status, Status::Running);
        assert_eq!(fx.ctx.store.get(&task.id).unwrap().runs.len(), 1);
        assert_eq!(fx.records("submissions.jsonl").len(), before + 1);
    }
}

#[test]
fn interrupted_coding_stages_keep_automatic_origin_and_never_replay_submitting() {
    for (stage, change) in [
        LaunchStage::Reserved,
        LaunchStage::Worktree,
        LaunchStage::CreatingPane,
        LaunchStage::Pane,
    ]
    .into_iter()
    .flat_map(|stage| {
        [
            Revoke::Pause,
            Revoke::Assist,
            Revoke::Member,
            Revoke::Dispatch,
        ]
        .map(|change| (stage, change))
    }) {
        // Given an actually prepared automatic launch interrupted by a known refusal.
        let mut fx = Fx::new();
        let task = fx.task("recover original coding", Status::Ready);
        let keep = fx.task("independent running", Status::Ready);
        fx.start(&keep.id);
        std::fs::write(fx.bin.join("hold-gates"), "").unwrap();
        let running_gate = corral_ops::gate_start(&fx.ctx, &keep.id).unwrap();
        let running_paths = corral_ops::gate_paths(&fx.ctx, &keep.id, "R1");
        fx.grant(&[&task.id, &keep.id]);
        let before = fx.records("submissions.jsonl").len();
        let barrier = Barrier::subscribe(&fx);
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| corral_ops::sync_at(&fx.fresh(), now()).unwrap());
            let mut release = barrier.arrival();
            revoke(&fx, &task.id, &keep.id, Revoke::Pause);
            release.write_all(b"R").unwrap();
            worker.join().unwrap();
        });
        drop(barrier);
        fx.ctx
            .store
            .update(&task.id, |task| {
                task.launch.as_mut().unwrap().stage = stage;
                Ok(())
            })
            .unwrap();
        fx.grant(&[&task.id, &keep.id]);
        let old = fx.ctx.store.get(&task.id).unwrap();
        let barrier = Barrier::subscribe(&fx);
        std::thread::scope(|scope| {
            // When recovery reaches the same final boundary, revoke from a new context.
            let worker = scope.spawn(|| corral_ops::sync_at(&fx.fresh(), now()).unwrap());
            let mut release = barrier.arrival();
            revoke(&fx, &task.id, &keep.id, change);
            release.write_all(b"R").unwrap();
            worker.join().unwrap();
        });
        drop(barrier);
        // Then recovery reused the run/pane and retained automatic provenance.
        let retained = fx.ctx.store.get(&task.id).unwrap();
        assert_eq!(retained.runs.len(), 1, "{stage:?} {change:?}");
        assert_eq!(
            retained.current_run().unwrap().herdr,
            old.current_run().unwrap().herdr
        );
        assert_eq!(
            fx.records("submissions.jsonl").len(),
            before,
            "{stage:?} {change:?}"
        );
        assert!(matches!(
            authorization(&fx, &task.id, OperatingActionKind::Dispatch)
                .authorization
                .origin,
            OperatingOrigin::Automatic { .. }
        ));
        let receipt = std::process::Command::new("bash")
            .arg(&running_paths.script)
            .env("CORRAL_ROOT", fx.temp.path())
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(
            receipt.status.success(),
            "{}",
            String::from_utf8_lossy(&receipt.stderr)
        );
        corral_ops::sync_at(&fx.fresh(), now()).unwrap();
        let collected = fx.ctx.store.get(&keep.id).unwrap();
        let gate = collected.current_run().unwrap().gate.as_ref().unwrap();
        assert_eq!(gate.result, corral_core::GateResult::Pass);
        assert_eq!(gate.pane, running_gate.pane);
        assert!(fx.records("closed.jsonl").is_empty());
        assert_eq!(fx.records("submissions.jsonl").len(), before);
        fx.ctx
            .store
            .update(&task.id, |task| {
                task.launch.as_mut().unwrap().stage = LaunchStage::Submitting;
                Ok(())
            })
            .unwrap();
        corral_ops::sync_at(&fx.fresh(), now()).unwrap();
        assert_eq!(fx.records("submissions.jsonl").len(), before);
        assert_eq!(fx.ctx.store.get(&task.id).unwrap().runs.len(), 1);
    }
}

#[test]
fn concrete_gate_and_profile_reviewer_refuse_all_revocations_and_reuse_preparation() {
    for kind in [OperatingActionKind::Gate, OperatingActionKind::Review] {
        for change in [
            Revoke::Pause,
            Revoke::Assist,
            Revoke::Member,
            Revoke::Dispatch,
        ] {
            // Given a completed candidate and an independently submitted gate.
            let mut fx = Fx::new();
            let task = fx.task("auxiliary candidate", Status::Ready);
            fx.start(&task.id);
            let running = fx.task("independent running", Status::Ready);
            fx.start(&running.id);
            fx.done(&task.id, "candidate.txt");
            if kind == OperatingActionKind::Review {
                corral_ops::gate_start(&fx.ctx, &task.id).unwrap();
                assert!(
                    corral_ops::gate_collect(&fx.ctx, &task.id)
                        .unwrap()
                        .is_some()
                );
            }
            std::fs::write(fx.bin.join("hold-gates"), "").unwrap();
            let running_gate = corral_ops::gate_start(&fx.ctx, &running.id).unwrap();
            let running_paths = corral_ops::gate_paths(&fx.ctx, &running.id, "R1");
            fx.grant(&[&task.id, &running.id]);
            let before = fx.records("submissions.jsonl").len();
            let barrier = Barrier::subscribe(&fx);
            std::thread::scope(|scope| {
                let worker = scope.spawn(|| corral_ops::sync_at(&fx.fresh(), now()).unwrap());
                let mut release = barrier.arrival();
                let original = authorization(&fx, &task.id, kind);
                assert_eq!(original.disposition, SubmissionDisposition::Prepared);
                assert!(matches!(
                    original.authorization.origin,
                    OperatingOrigin::Automatic { .. }
                ));
                let prepared = fx.ctx.store.get(&task.id).unwrap();
                let record_pane = match kind {
                    OperatingActionKind::Gate => prepared
                        .current_run()
                        .unwrap()
                        .gate
                        .as_ref()
                        .unwrap()
                        .pane
                        .clone(),
                    OperatingActionKind::Review => prepared
                        .current_run()
                        .unwrap()
                        .review
                        .as_ref()
                        .unwrap()
                        .pane
                        .clone(),
                    _ => unreachable!(),
                };
                // When a fresh context revokes before the actual final launch boundary.
                revoke(&fx, &task.id, &running.id, change);
                release.write_all(b"R").unwrap();
                worker.join().unwrap();
                // Then no command was submitted and the exact prepared journal is retained.
                assert_eq!(fx.records("submissions.jsonl").len(), before);
                let record = authorization(&fx, &task.id, kind);
                assert_eq!(record.authorization, original.authorization);
                assert!(matches!(
                    record.disposition,
                    SubmissionDisposition::AuthorizationRefused { .. }
                ));
                let key = if kind == OperatingActionKind::Gate {
                    "gate"
                } else {
                    "review"
                };
                let retained: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(
                        fx.ctx
                            .store
                            .state_dir()
                            .join("review-prepared")
                            .join(format!("{}-R1-{key}.json", task.id)),
                    )
                    .unwrap(),
                )
                .unwrap();
                assert_eq!(retained["submitted"], false);
                assert_eq!(retained["authorization"]["id"], original.authorization.id);
                assert_eq!(retained["pane"], record_pane.unwrap());
                let task = fx.ctx.store.get(&task.id).unwrap();
                assert!(task.review_reservation.is_none());
                assert_eq!(task.rounds, 0);
                let run = task.current_run().unwrap();
                assert_eq!(run.failures, 0);
                assert!(run.gate.is_none() || kind == OperatingActionKind::Review);
                assert!(run.review.is_none());
                if kind == OperatingActionKind::Review {
                    assert_eq!(run.review_runs.len(), 1);
                    assert!(run.review_runs[0].ended.is_some());
                    assert!(!run.review_runs[0].review.as_ref().unwrap().process_lost);
                }
            });
            drop(barrier);
            let finished = std::process::Command::new("bash")
                .arg(&running_paths.script)
                .env("CORRAL_ROOT", fx.temp.path())
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap();
            assert!(finished.status.success());
            corral_ops::sync_at(&fx.fresh(), now()).unwrap();
            assert_eq!(fx.records("submissions.jsonl").len(), before);
            let collected = fx.ctx.store.get(&running.id).unwrap();
            assert_eq!(
                collected
                    .current_run()
                    .unwrap()
                    .gate
                    .as_ref()
                    .unwrap()
                    .result,
                corral_core::GateResult::Pass
            );
            assert_eq!(
                collected.current_run().unwrap().gate.as_ref().unwrap().pane,
                running_gate.pane
            );
            assert!(fx.records("closed.jsonl").is_empty());
            // A confirmed amendment restores only the exact prepared attempt.
            fx.grant(&[&task.id, &running.id]);
            corral_ops::sync_at(&fx.fresh(), now()).unwrap();
            assert_eq!(fx.records("submissions.jsonl").len(), before + 1);
            let restored = fx.ctx.store.get(&task.id).unwrap();
            assert_eq!(restored.runs.len(), 1);
            if kind == OperatingActionKind::Review {
                assert_eq!(restored.current_run().unwrap().review_runs.len(), 1);
                assert!(
                    restored.current_run().unwrap().review_runs[0]
                        .ended
                        .is_none()
                );
            }
        }
    }
}
