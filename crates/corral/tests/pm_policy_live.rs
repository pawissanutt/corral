//! Candidate CLI / actual herdr PM policy journey. Run inside scripts/test-herdr.sh.
#![cfg(unix)]

use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use corral_core::{AgentCmd, OperatingAuthority, OperatingSelection, Status, Store};
use corral_herdr::{AgentStatus, ENTER_ATTEMPTS, Herdr};
use serde_json::{Value, json};

struct Journey {
    root: tempfile::TempDir,
    evidence: PathBuf,
    socket: UnixDatagram,
    herdr: Herdr,
    pane: Option<String>,
}

impl Drop for Journey {
    fn drop(&mut self) {
        if let Some(pane) = &self.pane
            && let Err(error) = self.herdr.pane_close(pane)
        {
            eprintln!("closing private R3 PM {pane}: {error:#}");
        }
    }
}

impl Journey {
    fn save(&self, name: &str, bytes: &[u8]) {
        std::fs::write(self.evidence.join(name), bytes).unwrap();
    }

    fn cli(&self, label: &str, args: &[&str]) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_corral"))
            .env("CORRAL_ROOT", self.root.path())
            .env("CORRAL_HERDR_SESSION", "corral-test")
            .env_remove("HERDR_BIN_PATH")
            .env_remove("HERDR_PANE_ID")
            .env_remove("HERDR_WORKSPACE_ID")
            .env_remove("HERDR_TAB_ID")
            .env_remove("CORRAL_GATE_ATTEMPT")
            .env_remove("CORRAL_CANDIDATE_HEAD")
            .env_remove("CORRAL_INTEGRATION_BASE")
            .arg("--root")
            .arg(self.root.path())
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        self.save(&format!("{label}.stdout"), &out.stdout);
        self.save(&format!("{label}.stderr"), &out.stderr);
        println!(
            "R3 candidate={} --root {} {args:?} exit={}",
            env!("CARGO_BIN_EXE_corral"),
            self.root.path().display(),
            out.status
        );
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    fn receive(&self, kind: &str, label: &str) -> Value {
        let mut bytes = vec![0; 65536];
        let n = self.socket.recv(&mut bytes).unwrap();
        self.save(&format!("{label}.json"), &bytes[..n]);
        let message: Value = serde_json::from_slice(&bytes[..n]).unwrap();
        assert_eq!(message["kind"], kind, "{message}");
        message
    }

    fn control(&self, command: &str) {
        self.socket
            .send_to(command.as_bytes(), self.root.path().join("control.sock"))
            .unwrap();
        self.receive(command, command);
    }

    fn fence(&self, label: &str, count: u64) {
        // The datagram receiver was bound before any CLI action. Send the fence
        // through herdr's same terminal input stream, after the CLI has exited.
        let marker = format!("R3-FENCE-{label}");
        for args in [
            vec!["pane", "send-text", self.pane.as_deref().unwrap(), &marker],
            vec!["pane", "send-keys", self.pane.as_deref().unwrap(), "enter"],
        ] {
            let out = Command::new("herdr")
                .args(["--session", "corral-test"])
                .args(&args)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
        }
        let response = self.receive("fence", label);
        assert_eq!(response["text"], marker);
        assert_eq!(response["count"], count);
    }

    fn receipt(&self, label: &str) -> Value {
        let bytes = std::fs::read(self.root.path().join(".corral/state/pm-policy.json")).unwrap();
        self.save(&format!("{label}-receipt.json"), &bytes);
        serde_json::from_slice::<Value>(&bytes).unwrap()["panes"][self.pane.as_deref().unwrap()]
            .clone()
    }

    fn prompt(&self, label: &str) -> Value {
        let bytes = std::fs::read(self.root.path().join(".corral/prompts/pm.md")).unwrap();
        self.save(&format!("{label}-prompt.md"), &bytes);
        parse_policy(std::str::from_utf8(&bytes).unwrap())
    }

    fn confirmed(&self, label: &str, message: &Value) -> Value {
        let policy = parse_policy(message["text"].as_str().unwrap());
        let effective = Store::open_unobserved(self.root.path())
            .unwrap()
            .effective_operating_policy()
            .unwrap();
        assert_eq!(policy["revision"], effective.revision);
        let receipt = self.receipt(label);
        assert_eq!(self.prompt(label), policy);
        assert_eq!(receipt["snapshot"], policy);
        assert_eq!(receipt["disposition"], "confirmed");
        assert_eq!(receipt["delivery"]["submitted"], true);
        // This fixture exercises the real terminal fallback, not a native
        // provider. Assert the observed receipt, not an assumed transport.
        assert_eq!(receipt["delivery"]["via"], "send_text");
        assert!(
            (1..=u64::from(ENTER_ATTEMPTS))
                .contains(&receipt["delivery"]["attempts"].as_u64().unwrap())
        );
        let capture = self
            .herdr
            .pane_read(self.pane.as_deref().unwrap(), 200)
            .unwrap();
        self.save(&format!("{label}-pane.txt"), capture.as_bytes());
        println!(
            "R3 {label}: revision={} receipt={receipt}",
            policy["revision"]
        );
        policy
    }
}

fn parse_policy(text: &str) -> Value {
    let block = text.rsplit_once("```json\n").unwrap().1;
    serde_json::from_str(block.split_once("\n```").unwrap().0).unwrap()
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

#[test]
fn live_policy_replacement_retains_uncertainty_without_replay_with_push_disabled() {
    let herdr = Herdr::with_bin("herdr".into(), Some("corral-test".into()));
    if !herdr.available() {
        println!("SKIP: herdr session corral-test unavailable");
        return;
    }
    // Never admit a reachable owner-configured session to this fixture.
    // The helper's shell-local CORRAL_TEST_HERDR_ROOT is not exported.
    // Its HOME/XDG layout and server log identify the private environment.
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    let private = home.parent().unwrap();
    assert!(
        private
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("corral-herdr-"),
        "run in the fresh scripts/test-herdr.sh subshell"
    );
    assert_eq!(
        std::env::var_os("HOME"),
        Some(private.join("home").into_os_string())
    );
    assert!(private.join("server.log").is_file());
    assert_eq!(
        std::env::var_os("XDG_CONFIG_HOME"),
        Some(private.join("config").into_os_string())
    );
    let root = tempfile::tempdir().unwrap();
    let socket = UnixDatagram::bind(root.path().join("receiver.sock")).unwrap();
    // Only bounds a broken IPC journey; no assertion infers success from silence.
    socket
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let evidence = std::env::var_os("CORRAL_R3_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.path().join("evidence"))
        .join(format!("r3-policy-{}", std::process::id()));
    std::fs::create_dir_all(&evidence).unwrap();
    let mut fx = Journey {
        root,
        evidence,
        socket,
        herdr,
        pane: None,
    };
    println!("R3 evidence={}", fx.evidence.display());

    // Given an actual PM process launched by the candidate, notifications off,
    // and a receiver bound before startup. The DSR barrier confirms rendering.
    std::fs::create_dir_all(fx.root.path().join("app/.git")).unwrap();
    let mut store = Store::init(fx.root.path()).unwrap();
    store.config.agents.insert(
        "r3-local-pm".into(),
        AgentCmd {
            command: format!(
                "python3 -u {} {} {} {{prompt_file}}",
                quote(
                    &Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/pm_policy_live/r3_receiver.py")
                ),
                quote(&fx.root.path().join("receiver.sock")),
                quote(&fx.root.path().join("control.sock")),
            ),
        },
    );
    store.config.pm.agent = "r3-local-pm".into();
    store.config.pm.push_events = false;
    store.config.herdr.session = Some("corral-test".into());
    store.save_config().unwrap();
    let opened = fx.cli("01-open", &["pm"]);
    fx.pane = Some(opened.trim().strip_prefix("PM pane ").unwrap().into());
    let startup = fx.receive("ready", "02-startup");
    assert_eq!(
        parse_policy(startup["text"].as_str().unwrap())["mode"],
        "assist"
    );
    fx.herdr
        .pane_report_agent(fx.pane.as_deref().unwrap(), "omo", AgentStatus::Idle, None)
        .unwrap();

    // Mode editors are outside T-145. Change policy through the real Store
    // publication API, then drive delivery only through fresh candidate CLIs.
    let task = store
        .create("R3 finite task", Some("app"), Status::Backlog, "")
        .unwrap();
    store
        .update(&task.id, |task| {
            task.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    let selection = OperatingSelection {
        tasks: [task.id.clone()].into(),
        authority: OperatingAuthority {
            dispatch: true,
            accept: false,
            plan_accept: true,
        },
        ..Default::default()
    };
    let preview = store.preview_operating_policy(&selection).unwrap();
    store
        .confirm_operating_policy(
            &preview,
            "R3 isolated owner",
            "2026-10-19T00:00:00Z".parse().unwrap(),
        )
        .unwrap();

    // When an already-live PM is reconciled, it receives the exact replacement.
    fx.cli("03-reconcile", &["sync"]);
    let update = fx.receive("policy", "04-update");
    let policy = fx.confirmed("05-granted", &update);
    assert_eq!(policy["mode"], "autopilot");
    assert_eq!(policy["paused"], false);
    assert_eq!(policy["scope"]["tasks"].as_object().unwrap().len(), 1);
    assert!(policy["scope"]["tasks"].get(&task.id).is_some());
    assert_ne!(
        policy["revision"],
        parse_policy(startup["text"].as_str().unwrap())["revision"]
    );
    assert_eq!(update["count"], 1);
    let confirmed = fx.receipt("06-before-dedup");
    assert_eq!(fx.cli("07-reopen", &["pm"]), opened);
    fx.cli("08-unchanged-sync", &["sync"]);
    fx.fence("09-unchanged-fence", 1);
    assert_eq!(fx.receipt("10-after-dedup"), confirmed);

    // Lifecycle changes do not replace the owner's policy or release its hold.
    store
        .update(&task.id, |task| {
            task.status = Status::Ready;
            Ok(())
        })
        .unwrap();
    fx.cli("10-lifecycle-sync", &["sync"]);
    fx.fence("10-lifecycle-fence", 1);
    assert_eq!(fx.receipt("10-lifecycle-receipt"), confirmed);

    // A rendered collapsed composer has an opaque outcome despite zero Enter
    // attempts. It stays alive and reports the original receive count.
    fx.control("block");
    store.pause_operating_policy(true).unwrap();
    fx.cli("11-failed-sync", &["sync"]);
    let failed = fx.receipt("12-failed");
    assert_eq!(failed["disposition"], "uncertain");
    assert_eq!(failed["delivery"]["via"], "send_text");
    assert_eq!(failed["delivery"]["submitted"], false);
    assert_eq!(failed["delivery"]["attempts"], 0);
    assert!(failed["error"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(failed["snapshot"]["paused"], true);
    assert_eq!(failed["snapshot"]["permissions"][&task.id], json!([]));
    assert_ne!(failed["snapshot"]["revision"], policy["revision"]);
    assert_eq!(fx.prompt("13-failed"), failed["snapshot"]);
    fx.fence("14-failure-fence", 1);
    // A separate read-only candidate process observes the retained outcome.
    let brief: Value =
        serde_json::from_str(&fx.cli("15-reopened-brief", &["pm", "brief", "--json"])).unwrap();
    let health = &brief["health"]["policy_delivery"]["panes"][fx.pane.as_deref().unwrap()];
    assert_eq!(health["record"], failed);
    assert_eq!(health["current"], false);
    assert_eq!(health["uncertain"], true);
    assert_eq!(fx.receipt("16-retained-failure"), failed);

    // Clearing a composer is not an acknowledgement of the previous delivery.
    // Reopen and sync repeatedly without replaying, even after another revision.
    fx.control("unblock");
    for round in 0..3 {
        assert_eq!(fx.cli(&format!("17-reopen-{round}"), &["pm"]), opened);
        fx.cli(&format!("18-sync-{round}"), &["sync"]);
        fx.fence(&format!("19-fence-{round}"), 1);
    }
    store.pause_operating_policy(false).unwrap();
    fx.cli("20-new-revision-sync", &["sync"]);
    fx.fence("21-new-revision-fence", 1);
    assert_eq!(fx.receipt("24-final"), failed);
    assert!(
        !Store::open_unobserved(fx.root.path())
            .unwrap()
            .config
            .pm
            .push_events
    );
    // Disabled pushes checkpoint EOF, but must never record event delivery.
    assert!(
        store
            .events()
            .unwrap()
            .iter()
            .all(|e| e.kind != "pm_event_delivery")
    );
    let task = store.get(&task.id).unwrap();
    assert!(task.dispatch_hold);
    assert!(task.runs.is_empty(), "{task:?}");
    assert!(task.plan_runs.is_empty(), "{task:?}");
    fx.save(
        "25-held-task.json",
        &serde_json::to_vec_pretty(&task).unwrap(),
    );
    println!(
        "R3 PASS: same pane={}, captures=1, pushes=false; lifecycle dedup and uncertain delivery barrier verified across composer clearing and policy revision",
        fx.pane.as_deref().unwrap()
    );
    let pane = fx.pane.as_deref().unwrap().to_owned();
    fx.herdr.pane_close(&pane).unwrap();
    assert!(fx.herdr.pane_get(&pane).unwrap().is_none());
    fx.pane = None;
    println!("R3 CLEANUP: private PM pane {pane} is absent");
}
