#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use corral_core::{HookIdentity, PostLandHook, Run, Status, Store, Task};
use corral_herdr::Herdr;
use corral_ops::{
    ApproveStrategy, Ctx, HookState, PostLandOperation, approve, done, new_task,
    operational_blocker, post_land_operations, reconcile_post_land, retry_post_land,
};
use serde_json::{Value, json};

const FIXTURE: &str = r#"#!/usr/bin/python3
import json, os, pathlib, socket, sys
assert sys.stdin.buffer.read() == b'', 'hook stdin must be null'
name = sys.argv[1]
root = pathlib.Path(os.environ['FIXTURE_ROOT'])
sha = os.environ['CORRAL_LANDED_SHA']
assert os.environ['CORRAL_BUILD_SHA'] == sha
assert os.environ['CORRAL_PROJECT'] == 'app'
assert pathlib.Path.cwd() == pathlib.Path(os.environ['EXPECTED_CWD'])
assert sys.argv[2:] == ['literal ; $HOME', 'argument with spaces']
with (root / 'trace').open('a') as f:
    f.write(json.dumps([name, sha, sys.argv[2:]]) + '\n')
if name == 'check' and (root / 'barrier').exists():
    (root / 'barrier').unlink()
    with socket.socket(socket.AF_UNIX) as s:
        s.settimeout(120)
        s.connect(str(root / 'barrier.sock'))
        s.sendall(b'entered\n')
        assert s.recv(1) == b'!'
if name == 'install':
    binary = root / 'prefix' / 'tool'
    binary.write_text('#!/usr/bin/python3\nimport sys\nassert sys.argv[1:] == ["--build-sha"]\nprint(' + repr(sha) + ')\n')
    binary.chmod(0o755)
if (root / ('fail-' + name)).exists():
    print('fixture stdout ' + name, flush=True)
    print('fixture stderr ' + name, file=sys.stderr, flush=True)
    sys.exit(23)
if name == 'smoke':
    import subprocess
    actual = subprocess.check_output([str(root / 'prefix' / 'tool'), '--build-sha'], stdin=subprocess.DEVNULL, text=True).strip()
    assert actual == sha
"#;

const FAKE_NOTIFIER: &str = r#"#!/usr/bin/python3
import json, pathlib, sys
assert sys.stdin.buffer.read() == b''
assert sys.argv[1:3] == ['--session', 'corral-test']
args = sys.argv[3:]
if args[:2] != ['notification', 'show']:
    sys.exit(1)
with pathlib.Path(__file__).with_name('notifications.jsonl').open('a') as log:
    log.write(json.dumps(args) + '\n')
print(json.dumps({'id': 'fake-notifier', 'result': {'shown': True}}))
"#;

struct Workspace {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
    ctx: Ctx,
}

fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn retry_interrupted<T>(mut operation: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    loop {
        match operation() {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn commit(repo: &Path, file: &str, text: &str) {
    fs::write(repo.join(file), text).unwrap();
    git(repo, &["add", file]);
    git(repo, &["commit", "-qm", file]);
}

impl Workspace {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let repo = root.join("app");
        fs::create_dir(&repo).unwrap();
        fs::create_dir(root.join("prefix")).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.name", "Hook Test"]);
        git(&repo, &["config", "user.email", "hooks@example.invalid"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        commit(&repo, ".gitignore", ".worktrees/\n");
        commit(&repo, "tracked", "base\n");
        executable(&root.join("hook"), FIXTURE);
        executable(&root.join("fake-notifier"), FAKE_NOTIFIER);
        executable(&root.join("prefix/tool"), "#!/bin/sh\nprintf 'prior\\n'\n");
        let mut ctx = Ctx {
            store: Store::init(&root).unwrap(),
            herdr: Herdr::with_bin(root.join("fake-notifier"), Some("corral-test".into())),
        };
        let hooks = ["check", "install", "smoke"].map(|name| PostLandHook {
            name: name.into(),
            argv: vec![
                root.join("hook").to_string_lossy().into(),
                name.into(),
                "literal ; $HOME".into(),
                "argument with spaces".into(),
            ],
            cwd: ".".into(),
            env: BTreeMap::from([
                ("FIXTURE_ROOT".into(), root.to_string_lossy().into()),
                ("EXPECTED_CWD".into(), repo.to_string_lossy().into()),
            ]),
            // Fixture execution is not a latency test; only guard against hangs.
            timeout_secs: 120,
            max_attempts: 2,
            identity: (name == "install").then(|| HookIdentity {
                binary: root.join("prefix/tool").to_string_lossy().into(),
                args: vec!["--build-sha".into()],
                expected_stdout: "{sha}".into(),
            }),
        });
        ctx.store.config.projects.get_mut("app").unwrap().post_land = hooks.into();
        Self {
            _tmp: tmp,
            root,
            repo,
            ctx,
        }
    }

    fn candidate(&self, file: &str) -> Task {
        let task = new_task(&self.ctx, file, Some("app"), &self.root, Status::Ready, "").unwrap();
        let worktree = self.repo.join(".worktrees").join(&task.id);
        let branch = format!("corral/{}", task.id);
        corral_git::worktree_add(&self.repo, &worktree, &branch, "main").unwrap();
        let run: Run = serde_json::from_value(json!({
            "id": "R1", "agent": "generic", "worktree": worktree,
            "branch": branch, "base": "main", "base_sha": git(&self.repo, &["rev-parse", "HEAD"]),
            "started": "2026-10-01T00:00:00Z"
        }))
        .unwrap();
        self.ctx
            .store
            .update(&task.id, |t| {
                t.runs.push(run.clone());
                t.status = Status::Running;
                Ok(())
            })
            .unwrap();
        fs::write(worktree.join(file), format!("feature {file}\n")).unwrap();
        done(&self.ctx, &task.id, "fixture completed").unwrap()
    }

    fn land(&self, task: &Task) {
        approve(
            &self.ctx,
            &task.id,
            ApproveStrategy::FastForward,
            false,
            true,
        )
        .unwrap();
    }

    fn operation(&self) -> PostLandOperation {
        let operations = post_land_operations(&self.ctx, Some("app")).unwrap();
        assert_eq!(
            operations.len(),
            1,
            "approve must persist a post-land operation"
        );
        operations.into_iter().next().unwrap()
    }

    fn trace(&self) -> Vec<Value> {
        match fs::read_to_string(self.root.join("trace")) {
            Ok(text) => text
                .lines()
                .map(|s| serde_json::from_str(s).unwrap())
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(e) => panic!("reading trace: {e}"),
        }
    }

    fn names(&self) -> Vec<String> {
        self.trace()
            .iter()
            .map(|v| v[0].as_str().unwrap().into())
            .collect()
    }

    // Fixture-only crash injection; production writers are never called directly.
    fn save_crash(&self, operation: &PostLandOperation) {
        let dir = self.ctx.store.state_dir().join("post-land");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{}.json", operation.key)),
            serde_json::to_vec_pretty(operation).unwrap(),
        )
        .unwrap();
    }

    fn restarted(&self) -> Ctx {
        Ctx {
            store: Store::open(&self.root).unwrap(),
            herdr: self.ctx.herdr.clone(),
        }
    }
}

#[test]
fn clean_approval_runs_ordered_hooks_once_and_persists_sha_bound_evidence() {
    // Given real Git and an isolated installation prefix.
    let ws = Workspace::new();
    let task = ws.candidate("feature");
    let sha = git(
        &task.current_run().unwrap().worktree,
        &["rev-parse", "HEAD"],
    );
    // When a clean candidate lands and restart reconciliation runs twice.
    ws.land(&task);
    reconcile_post_land(&ws.restarted()).unwrap();
    reconcile_post_land(&ws.restarted()).unwrap();
    // Then all three operations ran once, in order, for the exact landed commit.
    assert_eq!(ws.names(), ["check", "install", "smoke"]);
    assert!(ws.trace().iter().all(|v| v[1] == sha));
    let op = ws.operation();
    assert!(op.ready());
    assert_eq!(op.landed_sha, sha);
    assert_eq!(ws.ctx.store.get(&task.id).unwrap().status, Status::Done);
    assert!(operational_blocker(&ws.ctx, "app").unwrap().is_none());
    assert!(
        post_land_operations(&ws.ctx, Some("unknown"))
            .unwrap()
            .is_empty()
    );
    for receipt in &op.hooks {
        assert_eq!(receipt.attempts.len(), 1);
        let attempt = &receipt.attempts[0];
        assert_eq!(attempt.exit_code, Some(0));
        assert!(!attempt.timed_out);
        assert_eq!(attempt.cwd, ws.repo);
        assert_eq!(attempt.env["CORRAL_LANDED_SHA"], sha);
        assert!(attempt.finished.is_some());
        assert!(attempt.log.is_file());
    }
    assert_eq!(
        op.hooks[1].attempts[0].identity_stdout.as_deref(),
        Some(sha.as_str())
    );
    assert!(op.hooks[1].attempts[0].installed_sha256.is_some());
}

#[test]
fn keep_branch_does_not_create_or_execute_post_land_operations() {
    // Given a candidate with post-land hooks enabled.
    let ws = Workspace::new();
    let task = ws.candidate("kept");
    let before = git(&ws.repo, &["rev-parse", "HEAD"]);
    // When the owner approves without merging.
    approve(&ws.ctx, &task.id, ApproveStrategy::KeepBranch, false, true).unwrap();
    reconcile_post_land(&ws.ctx).unwrap();
    // Then no hook obligation exists and the base is unchanged.
    assert!(ws.trace().is_empty());
    assert!(post_land_operations(&ws.ctx, None).unwrap().is_empty());
    assert_eq!(git(&ws.repo, &["rev-parse", "HEAD"]), before);
}

#[test]
fn failed_merge_does_not_execute_hooks_or_complete_task() {
    // Given a candidate whose base diverged.
    let ws = Workspace::new();
    let task = ws.candidate("feature");
    commit(&ws.repo, "other", "divergent\n");
    let before = git(&ws.repo, &["rev-parse", "HEAD"]);
    // When fast-forward approval fails.
    assert!(approve(&ws.ctx, &task.id, ApproveStrategy::FastForward, false, true).is_err());
    reconcile_post_land(&ws.ctx).unwrap();
    // Then neither reconciliation nor the failed approval starts hooks.
    assert!(ws.trace().is_empty());
    assert_eq!(git(&ws.repo, &["rev-parse", "HEAD"]), before);
    assert_eq!(ws.ctx.store.get(&task.id).unwrap().status, Status::Review);
    assert!(
        post_land_operations(&ws.ctx, None)
            .unwrap()
            .iter()
            .all(|op| !op.landed)
    );
}

#[test]
fn owner_wait_before_approval_does_not_run_hooks() {
    // Given a finished candidate awaiting the owner.
    let ws = Workspace::new();
    let task = ws.candidate("waiting");
    let before = git(&ws.repo, &["rev-parse", "HEAD"]);
    // When restart reconciliation runs before approval.
    reconcile_post_land(&ws.restarted()).unwrap();
    // Then review alone is not a landing.
    assert!(ws.trace().is_empty());
    assert!(post_land_operations(&ws.ctx, None).unwrap().is_empty());
    assert_eq!(ws.ctx.store.get(&task.id).unwrap().status, Status::Review);
    assert_eq!(git(&ws.repo, &["rev-parse", "HEAD"]), before);
}

#[test]
fn failed_check_blocks_install_but_landed_task_stays_done() {
    // Given an explicitly failing check.
    let ws = Workspace::new();
    fs::write(ws.root.join("fail-check"), "").unwrap();
    let task = ws.candidate("feature");
    // When approval lands the candidate.
    ws.land(&task);
    // Then operational failure does not undo the landing or install anything.
    let op = ws.operation();
    assert!(op.landed);
    assert!(!op.ready());
    assert_eq!(ws.names(), ["check"]);
    assert_eq!(op.hooks[0].state, HookState::Failed);
    assert_eq!(op.hooks[1].state, HookState::Pending);
    assert!(op.hooks[1].attempts.is_empty());
    assert!(op.hooks[2].attempts.is_empty());
    assert_eq!(ws.ctx.store.get(&task.id).unwrap().status, Status::Done);
    assert_eq!(git(&ws.repo, &["rev-parse", "HEAD"]), op.landed_sha);
    assert!(operational_blocker(&ws.ctx, "app").unwrap().is_some());
    assert_eq!(
        fs::read_to_string(ws.root.join("prefix/tool")).unwrap(),
        "#!/bin/sh\nprintf 'prior\\n'\n"
    );
}

#[test]
fn failed_install_restores_prior_executable_and_records_exact_command_and_exit() {
    // Given a usable prior installation and an installer that overwrites it then fails.
    let ws = Workspace::new();
    let prior = fs::read(ws.root.join("prefix/tool")).unwrap();
    fs::write(ws.root.join("fail-install"), "").unwrap();
    let task = ws.candidate("feature");
    // When the candidate lands.
    ws.land(&task);
    // Then install failure is durable, full output is retained, and smoke never runs.
    let op = ws.operation();
    let receipt = &op.hooks[1];
    let attempt = &receipt.attempts[0];
    assert_eq!(ws.names(), ["check", "install"]);
    assert_eq!(receipt.state, HookState::Failed);
    assert_eq!(attempt.exit_code, Some(23));
    assert!(!attempt.timed_out);
    assert_eq!(fs::read(ws.root.join("prefix/tool")).unwrap(), prior);
    assert_eq!(
        fs::metadata(ws.root.join("prefix/tool"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(fs::read(attempt.backup.as_ref().unwrap()).unwrap(), prior);
    let log = fs::read_to_string(&attempt.log).unwrap();
    assert!(log.contains(&format!("argv: {:?}", receipt.hook.argv)));
    assert!(log.contains("fixture stdout install"));
    assert!(log.contains("fixture stderr install"));
    assert!(attempt.error.is_some());
    assert_eq!(ws.ctx.store.get(&task.id).unwrap().status, Status::Done);
}

#[test]
fn bounded_retry_never_repeats_successful_hooks() {
    // Given a successful check and failed installer.
    let ws = Workspace::new();
    fs::write(ws.root.join("fail-install"), "").unwrap();
    ws.land(&ws.candidate("feature"));
    let op = ws.operation();
    // When the only allowed retry fails, followed by an exhausted retry request.
    let retried = retry_post_land(&ws.ctx, "app", &op.landed_sha, false).unwrap();
    assert!(retry_post_land(&ws.ctx, "app", &op.landed_sha, false).is_err());
    // Then the successful check is not repeated and no third installer starts.
    assert_eq!(ws.names(), ["check", "install", "install"]);
    assert_eq!(retried.hooks[0].attempts.len(), 1);
    assert_eq!(retried.hooks[1].attempts.len(), 2);
    assert!(retried.hooks[2].attempts.is_empty());
    assert_eq!(retried.hooks[1].attempts[1].number, 2);
}

#[test]
fn explicit_retry_resumes_remaining_hooks_without_repeating_successes() {
    // Given a failed installer whose external failure is now resolved.
    let ws = Workspace::new();
    fs::write(ws.root.join("fail-install"), "").unwrap();
    ws.land(&ws.candidate("feature"));
    let op = ws.operation();
    fs::remove_file(ws.root.join("fail-install")).unwrap();
    // When the exact landed SHA is retried.
    let retried = retry_post_land(&ws.ctx, "app", &op.landed_sha, false).unwrap();
    reconcile_post_land(&ws.restarted()).unwrap();
    // Then only failed/pending hooks run and readiness is restored.
    assert!(retried.ready());
    assert_eq!(ws.names(), ["check", "install", "install", "smoke"]);
    assert!(operational_blocker(&ws.ctx, "app").unwrap().is_none());
}

#[test]
fn interrupted_running_receipt_without_identity_requires_acknowledged_retry() {
    // Given a crash after check side effects, before the success receipt.
    let ws = Workspace::new();
    ws.land(&ws.candidate("feature"));
    let mut op = ws.operation();
    op.hooks[0].state = HookState::Running;
    op.hooks[0].attempts[0].finished = None;
    op.hooks[0].attempts[0].exit_code = None;
    op.hooks[0].attempts[0].pid = None;
    op.hooks[0].attempts[0].process_start = None;
    ws.save_crash(&op);
    let before = ws.trace();
    // When a fresh supervisor reconciles and refuses an unacknowledged retry.
    let ctx = ws.restarted();
    reconcile_post_land(&ctx).unwrap();
    assert!(retry_post_land(&ctx, "app", &op.landed_sha, false).is_err());
    // Then absence of a live process is not evidence of success.
    let recovered = ws.operation();
    assert_eq!(recovered.hooks[0].state, HookState::Uncertain);
    assert_eq!(recovered.hooks[0].attempts.len(), 1);
    assert_eq!(ws.trace(), before);
    assert!(operational_blocker(&ctx, "app").unwrap().is_some());
    let retried = retry_post_land(&ctx, "app", &op.landed_sha, true).unwrap();
    assert!(retried.ready());
    assert_eq!(ws.names(), ["check", "install", "smoke", "check"]);
}

#[test]
fn installed_identity_proves_interrupted_install_without_live_process() {
    // Given a completed installation whose supervisor missed the final receipt.
    let ws = Workspace::new();
    ws.land(&ws.candidate("feature"));
    let mut op = ws.operation();
    op.hooks[1].state = HookState::Running;
    let attempt = &mut op.hooks[1].attempts[0];
    attempt.finished = None;
    attempt.exit_code = None;
    attempt.pid = None;
    attempt.process_start = None;
    attempt.installed_sha256 = None;
    attempt.identity_stdout = None;
    ws.save_crash(&op);
    let before = ws.trace();
    // When a new supervisor observes the installed executable's exact identity.
    reconcile_post_land(&ws.restarted()).unwrap();
    // Then installation is proven without repeating side effects or fabricating an exit code.
    let recovered = ws.operation();
    assert!(recovered.ready());
    assert_eq!(ws.trace(), before);
    let attempt = &recovered.hooks[1].attempts[0];
    assert_eq!(attempt.exit_code, None);
    assert!(attempt.finished.is_some());
    assert_eq!(
        attempt.identity_stdout.as_deref(),
        Some(op.landed_sha.as_str())
    );
    assert!(attempt.installed_sha256.is_some());
}

#[test]
fn sha_drift_prevents_pending_hook_execution_and_wrong_sha_retry() {
    // Given a crash before hooks and a subsequently advanced base.
    let ws = Workspace::new();
    ws.land(&ws.candidate("feature"));
    let mut op = ws.operation();
    for receipt in &mut op.hooks {
        receipt.state = HookState::Pending;
        receipt.attempts.clear();
    }
    ws.save_crash(&op);
    commit(&ws.repo, "later", "advanced\n");
    let before = ws.trace();
    // When reconciliation sees a different checkout and retries target the wrong identity.
    reconcile_post_land(&ws.restarted()).unwrap();
    let current = git(&ws.repo, &["rev-parse", "HEAD"]);
    assert!(retry_post_land(&ws.ctx, "app", &current, true).is_err());
    assert!(retry_post_land(&ws.ctx, "app", &op.landed_sha[..7], true).is_err());
    // Then no command starts against the wrong SHA.
    assert_eq!(ws.trace(), before);
    assert_eq!(ws.operation().hooks[0].state, HookState::Uncertain);
    assert!(ws.operation().hooks[0].attempts.is_empty());
}

#[test]
fn concurrent_lands_hold_repository_lock_through_hook_completion() {
    // Given two independent candidates and an IPC barrier inside the first check.
    let ws = Workspace::new();
    let first = ws.candidate("first");
    let second = ws.candidate("second");
    fs::write(ws.root.join("barrier"), "").unwrap();
    let listener = UnixListener::bind(ws.root.join("barrier.sock")).unwrap();
    let (connected_tx, connected_rx) = mpsc::channel();
    let acceptor = std::thread::spawn(move || {
        connected_tx
            .send(retry_interrupted(|| listener.accept()).unwrap().0)
            .unwrap();
    });
    let ctx = ws.ctx.clone();
    let first_id = first.id.clone();
    let first_thread = std::thread::spawn(move || {
        approve(&ctx, &first_id, ApproveStrategy::FastForward, false, true)
    });
    let mut stream = connected_rx.recv_timeout(Duration::from_secs(120)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(120)))
        .unwrap();
    let mut entered = [0; 8];
    stream.read_exact(&mut entered).unwrap();
    assert_eq!(&entered, b"entered\n");
    acceptor.join().unwrap();
    let lock_is_held = corral_git::try_landing_lock(&ws.repo).unwrap().is_none();
    let ctx = ws.ctx.clone();
    let second_id = second.id.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let second_thread = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        approve(&ctx, &second_id, ApproveStrategy::Rebase, false, true)
    });
    started_rx.recv_timeout(Duration::from_secs(120)).unwrap();
    // When the first hook is released, allowing both landings to finish.
    stream.write_all(b"!").unwrap();
    first_thread.join().unwrap().unwrap();
    second_thread.join().unwrap().unwrap();
    // Then no second landing advanced the checkout during the first operation.
    assert!(lock_is_held, "landing lock must remain held during hooks");
    assert_eq!(
        ws.names(),
        ["check", "install", "smoke", "check", "install", "smoke"]
    );
    let trace = ws.trace();
    assert!(trace[..3].iter().all(|v| v[1] == trace[0][1]));
    assert!(trace[3..].iter().all(|v| v[1] == trace[3][1]));
    assert_ne!(trace[0][1], trace[3][1]);
    let operations = post_land_operations(&ws.ctx, None).unwrap();
    assert_eq!(operations.len(), 2);
    assert!(operations.iter().all(PostLandOperation::ready));
    assert_eq!(ws.ctx.store.get(&first.id).unwrap().status, Status::Done);
    assert_eq!(ws.ctx.store.get(&second.id).unwrap().status, Status::Done);
}

#[test]
fn failed_readiness_blocks_project_and_dependent_dispatch_without_repeated_notifications() {
    // Given a landed failure and Ready tasks in the same project and a dependent project.
    let mut ws = Workspace::new();
    fs::write(ws.root.join("fail-check"), "").unwrap();
    let landed = ws.candidate("failure");
    ws.land(&landed);
    let project = ws.ctx.store.config.projects["app"].clone();
    ws.ctx
        .store
        .config
        .projects
        .insert("dependent".into(), project);
    let own = new_task(&ws.ctx, "own", Some("app"), &ws.root, Status::Ready, "").unwrap();
    let dependent = new_task(
        &ws.ctx,
        "dependent",
        Some("dependent"),
        &ws.root,
        Status::Ready,
        "",
    )
    .unwrap();
    ws.ctx
        .store
        .update(&dependent.id, |t| {
            t.depends_on.push(landed.id.clone());
            Ok(())
        })
        .unwrap();
    let opts = corral_ops::DispatchOpts {
        profile: Some("generic".into()),
        queue_if_blocked: true,
        ..Default::default()
    };
    // When dispatch and repeated restart reconciliation observe the operational failure.
    for id in [&own.id, &dependent.id] {
        assert!(matches!(
            corral_ops::dispatch(&ws.ctx, id, &opts).unwrap(),
            corral_ops::DispatchOutcome::Queued { .. }
        ));
    }
    reconcile_post_land(&ws.restarted()).unwrap();
    reconcile_post_land(&ws.restarted()).unwrap();
    // Then no pane or worktree was needed and the failure notification is not duplicated.
    assert!(ws.ctx.store.get(&own.id).unwrap().runs.is_empty());
    assert!(ws.ctx.store.get(&dependent.id).unwrap().runs.is_empty());
    assert_eq!(
        ws.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "blocked")
            .count(),
        1
    );
    let notifications: Vec<Value> = fs::read_to_string(ws.root.join("notifications.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0][0], "notification");
    assert_eq!(notifications[0][1], "show");
    assert_eq!(notifications[0][3], "--body");
    assert!(
        notifications[0][4]
            .as_str()
            .unwrap()
            .contains(&ws.operation().landed_sha)
    );
}

#[test]
fn restart_recovers_git_advance_before_landed_and_task_receipts() {
    // Given a crash-shaped prepared operation after Git advanced but before task/receipt persistence.
    let ws = Workspace::new();
    let task = ws.candidate("feature");
    ws.land(&task);
    let mut op = ws.operation();
    op.landed = false;
    for receipt in &mut op.hooks {
        receipt.state = HookState::Pending;
        receipt.attempts.clear();
    }
    ws.save_crash(&op);
    ws.ctx
        .store
        .update(&task.id, |t| {
            t.status = Status::Review;
            Ok(())
        })
        .unwrap();
    // When a restarted supervisor reconciles observed Git state.
    reconcile_post_land(&ws.restarted()).unwrap();
    // Then the task is durably Done and the prepared hook policy finishes at the exact SHA.
    assert_eq!(ws.ctx.store.get(&task.id).unwrap().status, Status::Done);
    assert!(ws.operation().ready());
    assert_eq!(
        ws.names(),
        ["check", "install", "smoke", "check", "install", "smoke"]
    );
}

#[test]
fn live_interrupted_process_prevents_identity_reconciliation_and_duplicate_retry() {
    // Given an interrupted install whose process is still alive at a deterministic socket barrier.
    let ws = Workspace::new();
    ws.land(&ws.candidate("feature"));
    let mut op = ws.operation();
    let listener = UnixListener::bind(ws.root.join("live.sock")).unwrap();
    let (tx, rx) = mpsc::channel();
    let acceptor = std::thread::spawn(move || {
        tx.send(retry_interrupted(|| listener.accept()).unwrap().0)
            .unwrap();
    });
    let mut child = Command::new("/usr/bin/python3")
        .args(["-c", "import socket,sys; s=socket.socket(socket.AF_UNIX); s.settimeout(120); s.connect(sys.argv[1]); s.sendall(b'!'); s.recv(1)"])
        .arg(ws.root.join("live.sock"))
        .env_clear().envs(&op.hooks[1].attempts[0].env)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().unwrap();
    let mut stream = rx.recv_timeout(Duration::from_secs(120)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(120)))
        .unwrap();
    let mut entered = [0];
    stream.read_exact(&mut entered).unwrap();
    acceptor.join().unwrap();
    let attempt = &mut op.hooks[1].attempts[0];
    attempt.pid = Some(child.id());
    use corral_ops::ProcessControl;
    attempt.process_start = Some(
        corral_ops::SystemProcesses
            .inspect(child.id())
            .unwrap()
            .unwrap()
            .started,
    );
    attempt.finished = None;
    attempt.exit_code = None;
    op.hooks[1].state = HookState::Running;
    ws.save_crash(&op);
    // When restart and an acknowledged retry see the live process.
    reconcile_post_land(&ws.restarted()).unwrap();
    let blocked = ws.operation().hooks[1].state == HookState::Uncertain;
    let retry_refused = retry_post_land(&ws.ctx, "app", &op.landed_sha, true).is_err();
    stream.write_all(b"!").unwrap();
    assert!(retry_interrupted(|| child.wait()).unwrap().success());
    // Then no side effect repeats, and after process exit its installed identity proves success.
    assert!(blocked);
    assert!(retry_refused);
    reconcile_post_land(&ws.restarted()).unwrap();
    assert!(ws.operation().ready());
    assert_eq!(ws.names(), ["check", "install", "smoke"]);
}

#[test]
fn hook_timeout_records_signal_and_stops_downstream_commands() {
    // Given a check blocked on an established socket and a one-second execution deadline.
    let mut ws = Workspace::new();
    ws.ctx
        .store
        .config
        .projects
        .get_mut("app")
        .unwrap()
        .post_land[0]
        .timeout_secs = 1;
    fs::write(ws.root.join("barrier"), "").unwrap();
    let listener = UnixListener::bind(ws.root.join("barrier.sock")).unwrap();
    let (tx, rx) = mpsc::channel();
    let observer = std::thread::spawn(move || {
        let mut stream = retry_interrupted(|| listener.accept()).unwrap().0;
        stream
            .set_read_timeout(Some(Duration::from_secs(120)))
            .unwrap();
        let mut entered = [0; 8];
        stream.read_exact(&mut entered).unwrap();
        let mut byte = [0];
        tx.send(retry_interrupted(|| stream.read(&mut byte)))
            .unwrap();
    });
    let task = ws.candidate("timeout");
    // When the bounded check times out without release of its socket barrier.
    ws.land(&task);
    // Then the process group exits, the exact signal is retained, and install does not start.
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(120)).unwrap().unwrap(),
        0
    );
    observer.join().unwrap();
    let op = ws.operation();
    assert_eq!(op.hooks[0].state, HookState::Failed);
    assert!(op.hooks[0].attempts[0].timed_out);
    assert_eq!(op.hooks[0].attempts[0].signal, Some(9));
    assert!(op.hooks[1].attempts.is_empty());
    assert_eq!(ws.ctx.store.get(&task.id).unwrap().status, Status::Done);
}
