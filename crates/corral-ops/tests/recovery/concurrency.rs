//! One fake-engine operation regression, not actual-model or interactive owner evidence.
#![cfg(unix)]

use super::*;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::process::{Pid, Signal, kill_process_group};
use std::io::Read;
use std::os::unix::{
    fs::PermissionsExt,
    net::UnixListener,
    process::{CommandExt, ExitStatusExt},
};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const NAME: &str = "concurrency::concurrent_apply_and_process_loss_never_replay_one_reviewer";
const CHILD_INPUT: &str = "RECOVERY_CONCURRENCY_APPLY_INPUT";
const BARRIER_BUDGET: Duration = Duration::from_secs(30);

// The existing fake accepts and records the command first. Its successful JSON is withheld
// from recover_apply until release, so process loss cannot be mistaken for a known refusal.
const HERDR: &str = r#"#!/usr/bin/env python3
import os
from pathlib import Path
import socket
import subprocess
import sys

root = Path(__file__).resolve().parent
engine = str(root / "plan_engine.sh")
args = sys.argv[1:]
operation = args[2:] if args[:1] == ["--session"] else args
if operation[:2] != ["pane", "run"]:
    os.execv(engine, [engine, *args])
accepted = subprocess.run([engine, *args], stdout=subprocess.PIPE, check=True)
with (root / "launch-count").open("ab", buffering=0) as counter:
    counter.write(b"pane run\n")
    os.fsync(counter.fileno())
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as barrier:
    barrier.settimeout(60)
    barrier.connect(str(root / "apply.sock"))
    barrier.sendall(b"A")
    if barrier.recv(1) != b"R":
        raise RuntimeError("parent closed without releasing accepted submission")
sys.stdout.buffer.write(accepted.stdout)
"#;

#[derive(serde::Serialize, serde::Deserialize)]
struct ApplyInput {
    root: PathBuf,
    id: String,
    preview: RecoveryPreview,
    input: RecoveryInput,
}

fn reopen(root: &Path) -> Ctx {
    let mut ctx = crate::resources::FixedResourceProbe::ctx(Store::open_unobserved(root).unwrap());
    ctx.herdr = corral_herdr::Herdr::with_bin(root.join("herdr"), Some("corral-test".into()));
    ctx
}

// A separate group owns only this libtest child and its fake-engine descendants. Cleanup also
// runs on assertion failure; it never scans for processes or signals the surrounding test group.
struct OwnedApply {
    child: Option<Child>,
    group: Pid,
}

impl OwnedApply {
    fn spawn(mut command: Command) -> Self {
        let child = command.process_group(0).spawn().unwrap();
        let group = Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap();
        Self {
            child: Some(child),
            group,
        }
    }

    fn kill_and_reap(&mut self) -> ExitStatus {
        match kill_process_group(self.group, Signal::KILL) {
            Ok(()) | Err(rustix::io::Errno::SRCH) => {}
            Err(error) => panic!("killing owned recovery child group: {error}"),
        }
        let status = self.child.as_mut().unwrap().wait().unwrap();
        self.child = None;
        status
    }
}

impl Drop for OwnedApply {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = kill_process_group(self.group, Signal::KILL);
            let _ = child.wait();
        }
    }
}

// Only interrupted blocking syscalls retry, always against the same monotonic deadline.
fn readable(fd: &impl rustix::fd::AsFd, deadline: Instant) -> bool {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        let timeout = Timespec::try_from(remaining).unwrap();
        match poll(&mut [PollFd::new(fd, PollFlags::IN)], Some(&timeout)) {
            Ok(count) => return count != 0,
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => panic!("polling recovery submission barrier: {error}"),
        }
    }
}

fn assert_one_launch(f: &Fixture) {
    assert_eq!(
        std::fs::read(f._tmp.path().join("launch-count")).unwrap(),
        b"pane run\n",
        "the accepted fake pane command must never be replayed"
    );
}

fn assert_append_only(before: &RecoveryRecord, after: &RecoveryRecord) {
    assert_eq!(after.version, before.version);
    assert_eq!(after.id, before.id);
    assert_eq!(after.preview, before.preview);
    assert_eq!(after.authorization, before.authorization);
    assert!(after.outcomes.starts_with(&before.outcomes));
    assert!(after.links.starts_with(&before.links));
}

#[test]
fn concurrent_apply_and_process_loss_never_replay_one_reviewer() {
    // This first re-exec clears managed owner/agent environment before either fixture or child.
    if isolated(NAME) {
        return;
    }
    if let Some(path) = std::env::var_os(CHILD_INPUT) {
        let saved: ApplyInput = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let ctx = reopen(&saved.root);
        assert_eq!(saved.id, saved.preview.request.target.task);
        let result = recover_apply(
            &ctx,
            &saved.preview,
            &saved.input,
            Some(confirmation(&saved.preview)),
        );
        panic!("accepted submission unexpectedly returned before process loss: {result:?}");
    }

    let mut f = Fixture::new(true);
    let input = f.input();
    let preview = recover_preview(&f.ctx, &f.id, &input).unwrap();
    assert!(preview.remaining_blockers.is_empty());
    let root = f.ctx.store.root().to_path_buf();
    assert!(!root.join("launch-command").exists());
    assert!(!root.join("launch-count").exists());
    std::fs::copy(root.join("herdr"), root.join("plan_engine.sh")).unwrap();
    std::fs::write(root.join("herdr"), HERDR).unwrap();
    std::fs::set_permissions(root.join("herdr"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let payload = root.join("apply-input.json");
    std::fs::write(
        &payload,
        serde_json::to_vec(&ApplyInput {
            root: root.clone(),
            id: f.id.clone(),
            preview: preview.clone(),
            input: input.clone(),
        })
        .unwrap(),
    )
    .unwrap();
    // Listen before spawn; readiness and the acceptance byte share one bounded deadline.
    let listener = UnixListener::bind(root.join("apply.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let log = root.join("apply-child.log");
    let output = std::fs::File::create(&log).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            &format!("recovery::tests::{NAME}"),
            "--nocapture",
        ])
        .env(CHILD_INPUT, &payload)
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(output.try_clone().unwrap())
        .stderr(output);
    let deadline = Instant::now() + BARRIER_BUDGET;
    let mut child = OwnedApply::spawn(command);
    assert!(
        readable(&listener, deadline),
        "accepted fake submission never arrived: {}",
        std::fs::read_to_string(&log).unwrap()
    );
    let (mut barrier, _) = listener.accept().unwrap();
    barrier.set_nonblocking(true).unwrap();
    assert!(readable(&barrier, deadline), "missing acceptance byte");
    let mut accepted = [0];
    barrier.read_exact(&mut accepted).unwrap();
    assert_eq!(accepted, [b'A']);
    assert_one_launch(&f);
    let accepted_command = std::fs::read(root.join("launch-command")).unwrap();
    assert!(!accepted_command.is_empty());

    let history = records(&f.ctx, &f.id).unwrap();
    assert_eq!(history.len(), 1);
    let applying = history[0].clone();
    assert_eq!(applying.preview, preview);
    assert_eq!(applying.authorization.preview_digest, preview.digest);
    assert_eq!(
        applying
            .outcomes
            .iter()
            .map(|o| o.state)
            .collect::<Vec<_>>(),
        vec![RecoveryState::Authorized, RecoveryState::Applying]
    );
    assert!(applying.links.is_empty());
    let task = f.ctx.store.get(&f.id).unwrap();
    assert_eq!(task.plans.len(), 1);
    assert_eq!(task.plans[0].id, "P1");
    assert_eq!(task.plans[0].reviews.len(), 1);
    let pending = task.plans[0].reviews[0].clone();
    assert_eq!(pending.id, "PR1");
    assert_eq!(pending.reviewer, "gpt-astra");
    assert_eq!(pending.policy_digest, preview.contract_digest);
    assert!(pending.herdr.is_some());
    assert!(pending.pending_reply.is_some());
    assert!(pending.verdict.is_none() && !pending.cancelled);
    let journal_id = f
        .ctx
        .store
        .record::<String>("recovery-journals", &applying.id)
        .unwrap()
        .unwrap();
    let journal = f.ctx.store.authorized_submission(&journal_id).unwrap();
    assert_eq!(journal.disposition, SubmissionDisposition::Submitting);
    assert_eq!(journal.authorization.action.task, f.id);
    assert_eq!(
        journal.authorization.action.kind,
        OperatingActionKind::PlanReview
    );
    assert_eq!(journal.authorization.action.plan.as_deref(), Some("P1"));
    assert_eq!(journal.authorization.action.attempt, pending.id);

    let before = f.ctx.store.fixture_task_text(&f.id).unwrap();
    let competing =
        recover_apply(&f.ctx, &preview, &input, Some(confirmation(&preview))).unwrap_err();
    assert!(
        competing
            .to_string()
            .contains("recovery operation already in progress")
    );
    let competing = recover_reconcile(&f.ctx, &f.id, &applying.id).unwrap_err();
    assert!(
        competing
            .to_string()
            .contains("recovery operation already in progress")
    );
    assert_eq!(f.ctx.store.fixture_task_text(&f.id).unwrap(), before);
    assert_eq!(
        f.ctx.store.authorized_submission(&journal_id).unwrap(),
        journal
    );
    assert_one_launch(&f);

    // Never send R: the accepted fake command's JSON must not reach recover_apply.
    assert_eq!(child.kill_and_reap().signal(), Some(9));
    assert!(
        readable(&barrier, Instant::now() + BARRIER_BUDGET),
        "owned fake-engine wrapper survived group kill"
    );
    assert_eq!(barrier.read(&mut accepted).unwrap(), 0);
    drop(barrier);
    f.ctx = reopen(&root);
    let retry = recover_apply(&f.ctx, &preview, &input, None).unwrap();
    assert_eq!(retry, applying);
    assert_eq!(state(&retry).unwrap(), RecoveryState::Applying);
    assert_one_launch(&f);

    // A pane reference alone is not delivery proof: the reply and journal remain uncertain.
    let unknown = recover_reconcile(&f.ctx, &f.id, &applying.id).unwrap();
    assert_append_only(&applying, &unknown);
    assert_eq!(unknown.outcomes.len(), applying.outcomes.len() + 1);
    assert_eq!(state(&unknown).unwrap(), RecoveryState::Unknown);
    assert!(unknown.links.is_empty());
    assert_eq!(
        f.ctx.store.get(&f.id).unwrap().plans[0].reviews,
        vec![pending.clone()]
    );
    assert_eq!(
        f.ctx.store.authorized_submission(&journal_id).unwrap(),
        journal
    );
    assert_eq!(
        recover_apply(&f.ctx, &preview, &input, None).unwrap(),
        unknown
    );
    assert_eq!(
        recover_reconcile(&f.ctx, &f.id, &applying.id).unwrap(),
        unknown
    );
    assert_one_launch(&f);

    // Only the real canonical verdict operation resolves this existing PR1; no new review starts.
    let callback = f.callback("PR1");
    assert_eq!(callback.id, pending.id);
    assert_eq!(callback.reviewer, pending.reviewer);
    assert_eq!(callback.herdr, pending.herdr);
    assert_eq!(callback.policy_digest, pending.policy_digest);
    assert_eq!(callback.verdict, Some(Verdict::Approve));
    let applied = recover_reconcile(&f.ctx, &f.id, &applying.id).unwrap();
    assert_append_only(&unknown, &applied);
    assert_eq!(
        applied.outcomes.iter().map(|o| o.state).collect::<Vec<_>>(),
        vec![
            RecoveryState::Authorized,
            RecoveryState::Applying,
            RecoveryState::Unknown,
            RecoveryState::Applied,
        ]
    );
    assert_eq!(
        applied.outcomes.last().unwrap().actual_effect.as_ref(),
        Some(&preview.proposed_effect)
    );
    assert_eq!(
        applied.links,
        vec![
            RecoveryLink {
                recovery: reference(&applying),
                receipt: RecoveryReceipt::PlanReview {
                    plan: "P1".into(),
                    attempt: pending.id,
                },
                acceptance: None,
            },
            RecoveryLink {
                recovery: reference(&applying),
                receipt: RecoveryReceipt::LaunchJournal {
                    stage: RecoveryAdmissionStage::PlanReview,
                    attempt: journal_id,
                },
                acceptance: None,
            },
        ]
    );
    let task = f.ctx.store.get(&f.id).unwrap();
    assert_eq!(task.plans[0].reviews, vec![callback]);
    assert!(task.plans[0].approval.is_none());
    assert_eq!(records(&f.ctx, &f.id).unwrap(), vec![applied]);
    assert_eq!(
        std::fs::read(root.join("launch-command")).unwrap(),
        accepted_command
    );
    assert_one_launch(&f);
}
