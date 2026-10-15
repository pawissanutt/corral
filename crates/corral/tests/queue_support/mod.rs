use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use corral_core::{Status, Store, Task};
use corral_herdr::Herdr;
use corral_ops::Ctx;
use serde_json::Value;

// A stateful protocol fixture, not a mocked dispatch service. Atomic JSON state
// preserves actual external side effects when the calling corral process dies.
const HERDR: &str = r#"#!/usr/bin/env python3
import fcntl, json, os, pathlib, signal, sys
root = pathlib.Path(__file__).parent
args = sys.argv[1:]
if args[:1] == ["--session"]: args = args[2:]
key = " ".join(args[:2])
def hook(point):
    marker = root / "fault"
    if marker.exists() and marker.read_text() == point:
        marker.unlink()
        if os.environ.get("CRASH_LAUNCH") == "1":
            os.kill(os.getppid(), signal.SIGKILL)
            sys.exit(0)
        print(json.dumps({"error":{"code":"injected","message":point}}))
        sys.exit(1)
with (root / "protocol.lock").open("a") as lock:
    fcntl.flock(lock, fcntl.LOCK_EX)
    path = root / "protocol.json"
    state = json.loads(path.read_text()) if path.exists() else {"panes":[], "workspaces":[], "creates":0, "submits":0}
    hook("before " + key)
    result = {}
    if key == "workspace list": result = {"workspaces":state["workspaces"]}
    elif key in ["workspace create", "tab create"]:
        state["creates"] += 1
        n = str(state["creates"])
        wid = args[args.index("--workspace")+1] if key == "tab create" else "w" + n
        pane = {"pane_id":wid+":p"+n, "workspace_id":wid, "tab_id":wid+":t"+n,
                "cwd":args[args.index("--cwd")+1], "agent_status":"unknown"}
        ws = {"workspace_id":wid, "label":args[args.index("--label")+1]}
        if key == "workspace create": state["workspaces"].append(ws)
        state["panes"].append(pane)
        result = {"workspace":ws, "root_pane":pane}
    elif key == "pane list": result = {"panes":state["panes"]}
    elif key == "pane get":
        pane = next((p for p in state["panes"] if p["pane_id"] == args[2]), None)
        if pane is None:
            print('{"error":{"code":"pane_not_found","message":"gone"}}'); sys.exit(1)
        result = {"pane":pane}
    elif key == "pane close":
        state["panes"] = [p for p in state["panes"] if p["pane_id"] != args[2]]
        state["workspaces"] = [w for w in state["workspaces"] if any(p["workspace_id"] == w["workspace_id"] for p in state["panes"])]
    elif key in ["pane run", "agent prompt"]:
        state["submits"] += 1
    elif key in ["pane report-metadata", "notification show"]: pass
    else:
        print(json.dumps({"error":{"code":"unexpected","message":key}})); sys.exit(1)
    temp = path.with_suffix(".tmp")
    temp.write_text(json.dumps(state)); temp.replace(path)
    hook("after " + key)
    print(json.dumps({"result":result}))
"#;

pub struct Fixture {
    pub temp: tempfile::TempDir,
    pub ctx: Ctx,
    pub bin: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        for project in ["app", "other"] {
            let repo = temp.path().join(project);
            std::fs::create_dir(&repo).unwrap();
            git(&repo, &["init", "-q", "-b", "main"]);
            git(&repo, &["config", "user.name", "queue fixture"]);
            git(&repo, &["config", "user.email", "queue@example.invalid"]);
            git(&repo, &["commit", "-q", "--allow-empty", "-m", "initial"]);
        }
        let bin = temp.path().join("herdr-fixture");
        std::fs::write(&bin, HERDR).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut store = Store::init(temp.path()).unwrap();
        store.config.defaults.profile = Some("generic".into());
        store.config.limits.running = 1;
        store.config.review.autopilot = Some(false);
        store.save_config().unwrap();
        let ctx = Ctx {
            store,
            herdr: Herdr::with_bin(bin.clone(), Some("corral-test".into())),
        };
        Self { temp, ctx, bin }
    }

    pub fn task(&self, project: &str, status: Status) -> Task {
        self.ctx
            .store
            .create("Queue fixture", Some(project), status, "")
            .unwrap()
    }

    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
        command
            .arg("--root")
            .arg(self.temp.path())
            .args(args)
            .env_remove("CORRAL_ROOT")
            .env("HERDR_BIN_PATH", &self.bin)
            .env("CORRAL_HERDR_SESSION", "corral-test")
            .stdin(Stdio::null());
        command
    }

    pub fn cli(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    pub fn ok(&self, args: &[&str]) -> Output {
        let out = self.cli(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    pub fn state(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.temp.path().join("protocol.json")).unwrap())
            .unwrap()
    }

    pub fn fault(&self, point: &str) {
        std::fs::write(self.temp.path().join("fault"), point).unwrap();
    }
}

pub fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
