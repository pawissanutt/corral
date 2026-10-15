//! Real worker delivery, confined to the isolated corral-test server.
#![cfg(unix)]

use std::os::unix::net::UnixDatagram;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use corral_core::{AgentCmd, Run, Status, Store};
use corral_herdr::{AgentStatus, ENTER_ATTEMPTS, Herdr};
use corral_ops::{ApproveStrategy, Ctx, DispatchOpts, DispatchOutcome};
use jiff::{SignedDuration, Timestamp};
use serde_json::json;
#[path = "support/live.rs"]
mod live;
mod support;

fn git(repo: &Path, args: &[&str]) -> String {
    let out = support::command("git", repo)
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
    String::from_utf8(out.stdout)
        .unwrap()
        .trim_end()
        .to_string()
}

fn commit(repo: &Path) -> String {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-qm", "movement fixture"]);
    git(repo, &["rev-parse", "HEAD"])
}

struct Worker<'a> {
    herdr: &'a Herdr,
    workspace: String,
}

impl Drop for Worker<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.herdr.workspace_close(&self.workspace) {
            eprintln!("isolated worker cleanup: {error:#}");
        }
    }
}

#[test]
fn isolated_worker_receives_main_movement_without_branch_changes() {
    // Given a real isolated worker, with an event receiver armed before launch.
    let tmp = tempfile::tempdir().unwrap();
    let herdr = live::herdr(tmp.path());
    if !herdr.available() {
        println!("SKIP: herdr session corral-test unavailable");
        return;
    }
    let socket_path = tmp.path().join("worker.sock");
    let socket = UnixDatagram::bind(&socket_path).unwrap();
    // Each receive waits for the worker's exact acknowledgement; only bound hangs.
    socket
        .set_read_timeout(Some(Duration::from_secs(120)))
        .unwrap();
    let script = tmp.path().join("worker.py");
    std::fs::write(
        &script,
        r#"import os, socket, sys, termios, tty
s = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
previous = termios.tcgetattr(0)
tty.setraw(0)
os.write(1, b"\x1b[?2004l> ")
s.sendto(b"ready", sys.argv[1])
line = bytearray()
try:
    while True:
        char = os.read(0, 1)
        if not char:
            break
        if char in (b"\r", b"\n"):
            s.sendto(bytes(line) + b"\n", sys.argv[1])
            line.clear()
            os.write(1, b"\r\n> ")
        else:
            line.extend(char)
            os.write(1, char)
finally:
    termios.tcsetattr(0, termios.TCSANOW, previous)
"#,
    )
    .unwrap();
    let app = tmp.path().join("app");
    std::fs::create_dir(&app).unwrap();
    git(&app, &["init", "-qb", "main"]);
    git(&app, &["config", "user.name", "corral test"]);
    git(&app, &["config", "user.email", "corral@test.invalid"]);
    git(&app, &["config", "commit.gpgsign", "false"]);
    std::fs::write(app.join("file.rs"), "fn initial() {}\n").unwrap();
    std::fs::write(app.join(".gitignore"), ".worktrees/\n").unwrap();
    let base = commit(&app);
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.agents.insert(
        "movement-worker".into(),
        AgentCmd {
            command: format!(
                "python3 -u '{}' '{}'",
                script.display(),
                socket_path.display()
            ),
        },
    );
    store.save_config().unwrap();
    let ctx = Ctx { store, herdr };
    let task = ctx
        .store
        .create("T-056 isolated worker", Some("app"), Status::Ready, "")
        .unwrap();
    let DispatchOutcome::Started { task, .. } = corral_ops::dispatch(
        &ctx,
        &task.id,
        &DispatchOpts {
            profile: Some("movement-worker".into()),
            ..DispatchOpts::default()
        },
    )
    .unwrap() else {
        panic!("worker did not dispatch");
    };
    let run = task.current_run().unwrap();
    let pane = run.herdr.as_ref().unwrap();
    let _worker = Worker {
        herdr: &ctx.herdr,
        workspace: pane.workspace_id.clone(),
    };
    let mut buffer = [0_u8; 32768];
    let n = socket.recv(&mut buffer).unwrap();
    assert_eq!(&buffer[..n], b"ready");
    ctx.herdr
        .pane_report_agent(&pane.pane_id, "movement-worker", AgentStatus::Working, None)
        .unwrap();
    // Readiness includes a flushed composer and an acknowledged real terminal submission,
    // not merely a process that has started before inheriting/setting its input mode.
    let handshake = ctx.herdr.agent_prompt(&pane.pane_id, "T056_READY").unwrap();
    assert!(handshake.submitted, "{handshake:?}");
    assert!(
        (1..=ENTER_ATTEMPTS).contains(&handshake.attempts),
        "{handshake:?}"
    );
    let n = socket.recv(&mut buffer).unwrap();
    assert_eq!(&buffer[..n], b"T056_READY\n");
    std::fs::write(run.worktree.join("file.rs"), "fn worker() {}\n").unwrap();
    let worker_head = commit(&run.worktree);
    let index_path = git(&run.worktree, &["rev-parse", "--git-path", "index"]);
    let index_before = std::fs::read(&index_path).unwrap();
    let status_before = git(&run.worktree, &["status", "--porcelain=v1", "-z"]);
    let before = ctx.store.get(&task.id).unwrap();
    let source = ctx
        .store
        .create("Landed fixture feature", Some("app"), Status::Running, "")
        .unwrap();
    let landing_tree = app.join(".worktrees/landing");
    corral_git::worktree_add(&app, &landing_tree, "corral/landing", "main").unwrap();
    std::fs::write(landing_tree.join("file.rs"), "fn landed() {}\n").unwrap();
    let landed_sha = commit(&landing_tree);
    let landing_run: Run = serde_json::from_value(json!({
        "id":"R1","agent":"fixture","worktree":landing_tree,"branch":"corral/landing",
        "base":"main","base_sha":base,"started":Timestamp::now()
    }))
    .unwrap();
    ctx.store
        .update(&source.id, |t| {
            t.runs.push(landing_run);
            Ok(())
        })
        .unwrap();
    corral_ops::done(&ctx, &source.id, "Verified: fixture\nNot verified: none").unwrap();

    // When a real approval lands and sync processes its coalesced overlap batch.
    corral_ops::approve(&ctx, &source.id, ApproveStrategy::FastForward, true, false).unwrap();
    let now = Timestamp::now();
    corral_ops::sync_at(&ctx, now).unwrap();
    corral_ops::sync_at(&ctx, now + SignedDuration::from_secs(2)).unwrap();
    let queue_path = ctx
        .store
        .state_dir()
        .join(format!("main-movement-{}.json", task.id));
    let queue = std::fs::read_to_string(&queue_path).unwrap();
    let receipt: serde_json::Value = serde_json::from_str(&queue).unwrap();
    assert_eq!(
        receipt["notices"][0]["state"],
        "delivered",
        "queue={queue}; task={:?}; pane={:?}; events={:?}",
        ctx.store.get(&task.id).unwrap(),
        ctx.herdr.pane_get(&pane.pane_id),
        ctx.store.events().unwrap()
    );
    let n = socket.recv(&mut buffer).unwrap_or_else(|error| {
        panic!(
            "worker receive: {error}; queue={queue}; pane output={:?}",
            ctx.herdr.pane_read(&pane.pane_id, 80)
        )
    });
    let message = std::str::from_utf8(&buffer[..n]).unwrap();

    // Then the actual worker consumes the exact factual identities and keeps its branch/state.
    assert!(message.contains(&source.id));
    assert!(message.contains(&landed_sha));
    assert!(message.contains("file.rs"));
    corral_ops::sync_at(&ctx, now + SignedDuration::from_secs(3)).unwrap();
    let events = ctx.store.events().unwrap();
    let delivered: Vec<_> = events
        .iter()
        .filter(|e| {
            e.kind == "main_movement_notice"
                && e.task.as_ref() == Some(&task.id)
                && e.detail["state"] == "delivered"
        })
        .collect();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].detail["delivery"]["submitted"], true);
    assert!(
        (1..=u64::from(ENTER_ATTEMPTS)).contains(
            &delivered[0].detail["delivery"]["attempts"]
                .as_u64()
                .unwrap()
        ),
        "{:?}",
        delivered[0]
    );
    assert_eq!(git(&run.worktree, &["rev-parse", "HEAD"]), worker_head);
    assert_eq!(
        git(&run.worktree, &["branch", "--show-current"]),
        run.branch
    );
    assert_eq!(std::fs::read(index_path).unwrap(), index_before);
    assert_eq!(
        git(&run.worktree, &["status", "--porcelain=v1", "-z"]),
        status_before
    );
    let after = ctx.store.get(&task.id).unwrap();
    assert_eq!(after.rounds, before.rounds);
    assert_eq!(after.status, before.status);
    assert_eq!(
        after.current_run().unwrap().pending_reply,
        before.current_run().unwrap().pending_reply
    );
    println!("PASS isolated worker received: {}", message.trim_end());
    println!(
        "PASS branch unchanged: {} at {worker_head}; index and worktree unchanged; rounds={} status={:?}",
        run.branch, after.rounds, after.status
    );
    println!(
        "PASS delivery receipt: {}",
        serde_json::to_string(&delivered[0]).unwrap()
    );
}
