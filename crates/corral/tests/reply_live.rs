//! Real paste/submit acceptance, confined to the isolated corral-test server.

use std::os::unix::net::UnixDatagram;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use corral_core::{AgentCmd, Store};
use corral_herdr::{AgentStatus, ENTER_ATTEMPTS, Herdr};

const SESSION: &str = "corral-test";

// Time is the behavior under test here: a busy agent delays displaying its paste.
// Unit tests inject their poller instead. Socket events synchronize this live test.
const AGENT: &str = r#"import os, select, socket, sys, termios, time, tty
with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as sock:
    sock.connect(sys.argv[1])
    fd = sys.stdin.fileno()
    original = termios.tcgetattr(fd)
    tty.setraw(fd)
    try:
        print("\x1b[?2004h\x1b[2J\x1b[HBusy fake OmO agent\r\n❯ ", end="", flush=True)
        sock.send(b"READY")
        pending = b""
        visible = False
        deadline = None
        lost = False
        while True:
            timeout = None if deadline is None else max(0, deadline - time.monotonic())
            ready, _, _ = select.select([fd], [], [], timeout)
            if ready:
                data = os.read(fd, 4096)
                if not data:
                    break
                if b"\r" in data or b"\n" in data:
                    if not visible or not lost:
                        lost = True
                        sock.send(b"ENTER_LOST")
                    else:
                        text = pending.decode()
                        print("\x1b[2J\x1b[HBusy fake OmO agent\r\nAccepted: " + text + "\r\n❯ ", end="", flush=True)
                        sock.send(b"SUBMITTED:" + pending)
                        pending = b""
                        visible = False
                else:
                    pending += data
                    deadline = time.monotonic() + 0.25
            if deadline is not None and time.monotonic() >= deadline:
                pending = pending.replace(b"\x1b[200~", b"").replace(b"\x1b[201~", b"")
                print("\x1b[2J\x1b[HBusy fake OmO agent\r\n❯ " + pending.decode(), end="", flush=True)
                visible = True
                deadline = None
                sock.send(b"PASTE_VISIBLE")
    finally:
        termios.tcsetattr(fd, termios.TCSANOW, original)
"#;

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn command(root: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_corral"));
    cmd.arg("--root")
        .arg(root)
        .args(args)
        .env_remove("HERDR_BIN_PATH")
        .env("CORRAL_ROOT", root)
        .env_remove("CORRAL_GATE_ATTEMPT")
        .env_remove("CORRAL_CANDIDATE_HEAD")
        .env_remove("CORRAL_INTEGRATION_BASE")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_TAB_ID")
        .env("CORRAL_HERDR_SESSION", SESSION)
        .stdin(Stdio::null());
    cmd
}

fn ok(root: &Path, args: &[&str]) -> String {
    let out = command(root, args).output().unwrap();
    assert!(
        out.status.success(),
        "corral {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .env("CORRAL_ROOT", root)
        .env_remove("CORRAL_GATE_ATTEMPT")
        .env_remove("CORRAL_CANDIDATE_HEAD")
        .env_remove("CORRAL_INTEGRATION_BASE")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_TAB_ID")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct PaneGuard {
    herdr: Herdr,
    pane: String,
}

impl Drop for PaneGuard {
    fn drop(&mut self) {
        if let Err(err) = self.herdr.pane_close(&self.pane) {
            eprintln!("closing isolated pane {}: {err:#}", self.pane);
        }
    }
}

#[test]
fn busy_slow_paste_agent_receives_reply_after_lost_enter() {
    // Given the real isolated server and a busy fake agent with delayed paste ingestion.
    let herdr = Herdr::with_bin("herdr".into(), Some(SESSION.into()));
    if !herdr.available() {
        println!("SKIP: herdr session {SESSION} unavailable");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let app = root.join("app");
    std::fs::create_dir(&app).unwrap();
    git(&app, &["init", "-q", "-b", "main"]);
    git(&app, &["config", "user.name", "corral live test"]);
    git(&app, &["config", "user.email", "corral@test.invalid"]);
    git(&app, &["config", "commit.gpgsign", "false"]);
    git(&app, &["commit", "--allow-empty", "-q", "-m", "init"]);
    let socket_path = root.join("agent.sock");
    let socket = UnixDatagram::bind(&socket_path).unwrap();
    // Socket events, not elapsed time, establish readiness and submission.
    socket
        .set_read_timeout(Some(Duration::from_secs(120)))
        .unwrap();
    let mut store = Store::init(root).unwrap();
    store.config.agents.insert(
        "slow-agent".into(),
        AgentCmd {
            command: format!(
                "python3 -u -c {} {}",
                quote(AGENT),
                quote(&socket_path.display().to_string())
            ),
        },
    );
    store.config.herdr.session = Some(SESSION.into());
    store.save_config().unwrap();
    let id = ok(root, &["new", "Busy slow delivery", "-p", "app", "--ready"])
        .trim()
        .to_string();
    ok(root, &["dispatch", &id, "--profile", "slow-agent"]);
    let task = store.get(&id).unwrap();
    let pane = task
        .current_run()
        .unwrap()
        .herdr
        .as_ref()
        .unwrap()
        .pane_id
        .clone();
    let guard = PaneGuard { herdr, pane };
    let mut bytes = [0; 4096];
    let n = socket.recv(&mut bytes).unwrap();
    assert_eq!(&bytes[..n], b"READY");
    guard
        .herdr
        .pane_report_agent(&guard.pane, "omo", AgentStatus::Working, None)
        .unwrap();
    let before = guard.herdr.pane_read(&guard.pane, 12).unwrap();
    // When the real CLI replies during work, and the first Enter is discarded.
    let output = ok(root, &["reply", &id, "T-051-slow-paste-delivered"]);
    // Then the agent, not only the transport, acknowledges the exact submitted message.
    let mut signals = Vec::new();
    loop {
        let n = socket.recv(&mut bytes).unwrap();
        let signal = String::from_utf8(bytes[..n].to_vec()).unwrap();
        let submitted = signal == "SUBMITTED:T-051-slow-paste-delivered";
        signals.push(signal);
        if submitted {
            break;
        }
        assert!(signals.len() < 8, "unexpected agent signals: {signals:?}");
    }
    assert!(signals.iter().any(|s| s == "PASTE_VISIBLE"));
    assert!(signals.iter().any(|s| s == "ENTER_LOST"));
    let after = guard.herdr.pane_read(&guard.pane, 12).unwrap();
    let events = store.events().unwrap();
    let event = events.iter().find(|e| e.kind == "replied").unwrap();
    assert_eq!(event.detail["via"], "send_text");
    assert_eq!(event.detail["submitted"], true);
    assert!(
        (1..=u64::from(ENTER_ATTEMPTS)).contains(&event.detail["attempts"].as_u64().unwrap()),
        "{event:?}"
    );
    assert!(
        store
            .get(&id)
            .unwrap()
            .current_run()
            .unwrap()
            .pending_reply
            .is_none()
    );
    println!("LIVE session={SESSION} pane={} status=working", guard.pane);
    println!("BEFORE:\n{before}\nSIGNALS: {signals:?}\nCLI: {output}AFTER:\n{after}");
    println!("DELIVERY: {}", event.detail);
}
