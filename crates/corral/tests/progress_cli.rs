//! Real CLI round trips and the bytes consumed by two generic harnesses.
use std::io::Read;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use corral_core::{AgentCmd, Store};
use serde_json::{Value, json};

fn command(program: &str, args: &[&str], cwd: &Path) -> Output {
    Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env_remove("CORRAL_ROOT")
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn cli(root: &Path, args: &[&str]) -> Output {
    let mut all = vec!["--root", root.to_str().unwrap()];
    all.extend(args);
    command(env!("CARGO_BIN_EXE_corral"), &all, root)
}

fn ok(root: &Path, args: &[&str]) -> String {
    let out = cli(root, args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn value(root: &Path, args: &[&str]) -> Value {
    serde_json::from_str(&ok(root, args)).unwrap()
}

fn git(root: &Path, args: &[&str]) {
    let mut all = vec![
        "-c",
        "user.name=fixture",
        "-c",
        "user.email=fixture@test.invalid",
        "-c",
        "commit.gpgsign=false",
    ];
    all.extend(args);
    let out = command("git", &all, root);
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn workspace() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    std::fs::create_dir(&app).unwrap();
    git(&app, &["init", "-q", "-b", "main"]);
    std::fs::write(app.join("README.md"), "portable fixture\n").unwrap();
    git(&app, &["add", "."]);
    git(&app, &["commit", "-qm", "fixture"]);
    ok(temp.path(), &["init"]);
    temp
}

/// Subscribe before launch; a bounded receive observes real prompt consumption, not timing.
fn receiver(store: &mut Store, name: &str) -> mpsc::Receiver<String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut prompt = String::new();
        socket.read_to_string(&mut prompt).unwrap();
        tx.send(prompt).unwrap();
    });
    store.config.agents.insert(
        name.into(),
        AgentCmd {
            command: format!(
                "bash -c 'cat \"$1\" > /dev/tcp/127.0.0.1/{port}' -- '{{prompt_file}}'"
            ),
        },
    );
    store.save_config().unwrap();
    rx
}

fn packet(prompt: &str) -> Value {
    let content = prompt
        .split_once("<corral-resume>\n")
        .unwrap()
        .1
        .split_once("\n</corral-resume>")
        .unwrap()
        .0;
    serde_json::from_str(content).unwrap()
}

fn write_json(root: &Path, name: &str, data: &Value) -> PathBuf {
    let path = root.join(name);
    std::fs::write(&path, serde_json::to_vec_pretty(data).unwrap()).unwrap();
    path
}

fn checkpoint(state: &Value) -> Value {
    json!({
        "task":state["task"], "run":state["run"], "spec_revision":state["spec_revision"],
        "expected_revision":0, "progress_revision":state["progress"]["revision"],
        "head":state["head"], "base":state["base"], "tree":state["tree"],
        "criterion_ids":["portable"], "evidence_refs":[],
        "next_action":"Run the retained integration scenario",
        "decisions":["Choose the release branch"],
        "waits":[{"id":"gate","owner":"harness-a","handle":"job-42","condition":"gate exits"}],
        "resources":[{"id":"server","owner":"harness-a","description":"isolated fixture",
            "cleanup":"Close only the fixture-owned listener"}]
    })
}

fn cleanup(store: &Store, id: &str) {
    let ctx = corral_ops::Ctx::from_store(store.clone());
    for run in store.get(id).unwrap().runs {
        if let Some(h) = run.herdr
            && ctx.herdr.pane_get(&h.pane_id).unwrap().is_some()
        {
            ctx.herdr.pane_close(&h.pane_id).unwrap();
        }
    }
}

#[test]
fn portable_resume_reaches_fresh_harness_through_actual_cli_prompt() {
    // Given two generic consumers on the isolated engine, not either harness's memory service.
    let check = Command::new("herdr")
        .args(["--session", "corral-test", "workspace", "list"])
        .stdin(Stdio::null())
        .output();
    if !check.is_ok_and(|o| o.status.success()) {
        println!("SKIP: herdr session corral-test unavailable");
        return;
    }
    let temp = workspace();
    let root = temp.path();
    let mut store = Store::open(root).unwrap();
    store.config.herdr.session = Some("corral-test".into());
    let a = receiver(&mut store, "fixture-a");
    let b = receiver(&mut store, "fixture-b");
    let id = ok(
        root,
        &[
            "new",
            "Portable fixture",
            "-p",
            "app",
            "--ready",
            "--body",
            "Keep the canonical portable criterion.",
        ],
    )
    .trim()
    .to_string();
    ok(root, &["dispatch", &id, "--profile", "fixture-a"]);
    // Portability, not clock ordering, is the behavior under test. Use one
    // fixed attempt time so host clock rollback cannot invalidate the fixture.
    let captured: jiff::Timestamp = "2026-10-15T09:00:00Z".parse().unwrap();
    store
        .update(&id, |task| {
            task.current_run_mut().unwrap().started = captured;
            Ok(())
        })
        .unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let prompt_a = a.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(packet(&prompt_a)["status"], "missing");
        let digest = value(root, &["criteria", "list", &id]);
        let manifest = write_json(
            root,
            "criteria.json",
            &json!({
                "version":1, "spec_digest":digest["spec_digest"], "reviewed_by":"owner",
                "audited":false, "require_gate":false, "criteria":[{
                    "id":"portable", "required":true, "live":true,
                    "scenario":"Portable CLI resume", "expected":"criterion linked progress"
                }], "owner_decisions":[]
            }),
        );
        ok(
            root,
            &[
                "criteria",
                "register",
                &id,
                "--file",
                manifest.to_str().unwrap(),
            ],
        );
        let initial = value(root, &["run", "progress", &id, "--json"]);
        let progress = json!({
            "task":id, "run":"R1", "spec_revision":initial["spec_revision"],
            "expected_revision":0, "phases":[
                {"id":"build","label":"Build","steps":[
                    {"id":"implemented","label":"Implement","status":{"state":"done"},"criteria":["portable"]}]},
                {"id":"verify","label":"Verification","steps":[
                    {"id":"scenario","label":"Run scenario","status":{"state":"in_progress"},"criteria":["portable"]}]}
            ]
        });
        let file = write_json(root, "progress.json", &progress);
        let state = value(
            root,
            &[
                "run",
                "progress",
                &id,
                "--file",
                file.to_str().unwrap(),
                "--json",
            ],
        );
        let source = write_json(
            root,
            "capture.json",
            &json!({"criterion":"portable","observed":true}),
        );
        let artifact = ok(
            root,
            &[
                "artifact",
                "add",
                &id,
                source.to_str().unwrap(),
                "--kind",
                "evidence",
            ],
        );
        let artifact = artifact.split_whitespace().last().unwrap();
        store
            .update(&id, |task| {
                for artifact in &mut task.artifacts {
                    artifact.added = captured;
                }
                Ok(())
            })
            .unwrap();
        let observation = write_json(
            root,
            "observation.json",
            &json!({
                "version":1, "key":"portable-live", "criterion":"portable", "run":"R1",
                "attempt":"no-gate", "status":"pass", "action":"Read CLI progress",
                "environment":"corral-test", "expected":"criterion linked progress",
                "observed":"criterion linked progress", "artifacts":[artifact],
                "captured":captured, "producer":"CLI fixture", "live":true
            }),
        );
        let receipt = value(
            root,
            &[
                "evidence",
                "record",
                &id,
                "--file",
                observation.to_str().unwrap(),
            ],
        );
        let mut snapshot = checkpoint(&state);
        snapshot["evidence_refs"] = json!(["portable-live"]);
        let cp = write_json(root, "checkpoint.json", &snapshot);
        let saved = value(
            root,
            &[
                "run",
                "checkpoint",
                &id,
                "--file",
                cp.to_str().unwrap(),
                "--json",
            ],
        );
        let show = value(root, &["show", &id, "--json"]);
        assert_eq!(show["runs"][0]["progress"], saved["progress"]);
        assert_eq!(show["runs"][0]["checkpoints"], saved["checkpoints"]);
        assert_eq!(show["criteria_progress"][0]["criterion"]["id"], "portable");
        assert_eq!(
            show["criteria_progress"][0]["steps"][0]["status"]["state"],
            "done"
        );
        assert!(ok(root, &["show", &id]).contains("criterion portable:"));
        assert_eq!(
            show["criteria_progress"][0]["evidence"],
            json!([receipt.clone()])
        );
        assert!(ok(root, &["show", &id]).contains("evidence portable-live [Pass] run R1:"));
        // When a fresh harness replaces A through the real retry launch surface.
        ok(
            root,
            &["retry", &id, "--run", "R1", "--profile", "fixture-b"],
        );
        let prompt_b = b.recv_timeout(Duration::from_secs(10)).unwrap();
        let resume = packet(&prompt_b);
        // Then stable step/criterion IDs, concrete action and obligations survive.
        assert_eq!(resume["status"], "compatible");
        assert_eq!(resume["criteria"][0]["criterion"]["id"], "portable");
        assert_eq!(resume["criteria"][0]["evidence"], json!([receipt]));
        assert_eq!(
            resume["criteria"][0]["steps"][1]["status"]["state"],
            "in_progress"
        );
        assert_eq!(
            resume["progress"]["phases"][0]["steps"][0]["id"],
            "implemented"
        );
        assert_eq!(
            resume["checkpoint"]["checkpoint"]["completed_steps"],
            json!(["implemented"])
        );
        assert_eq!(
            resume["checkpoint"]["checkpoint"]["input"]["criterion_ids"],
            json!(["portable"])
        );
        assert_eq!(
            resume["checkpoint"]["checkpoint"]["input"]["next_action"],
            checkpoint(&state)["next_action"]
        );
        assert_eq!(
            resume["checkpoint"]["checkpoint"]["input"]["waits"][0]["handle"],
            "job-42"
        );
        assert_eq!(
            resume["checkpoint"]["checkpoint"]["input"]["resources"][0]["id"],
            "server"
        );
        assert_eq!(resume["reusable_evidence"], json!([]));
        let restarted = value(root, &["run", "checkpoint", &id, "--json"]);
        assert_eq!(restarted["checkpoint_status"], "compatible");
        assert_eq!(restarted["progress"], saved["progress"]);
        assert_eq!(restarted["checkpoints"], saved["checkpoints"]);
        let obsolete = cli(root, &["retry", &id, "--run", "R1"]);
        assert!(!obsolete.status.success());
        assert_eq!(store.get(&id).unwrap().runs.len(), 2);
        println!("CLI_PROGRESS {}", serde_json::to_string(&saved).unwrap());
        println!("HARNESS_B_PROMPT_BEGIN\n{prompt_b}HARNESS_B_PROMPT_END");
        if let Some(dir) = std::env::var_os("CORRAL_PROGRESS_EVIDENCE_DIR") {
            let dir = PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("harness-a-prompt.md"), prompt_a).unwrap();
            std::fs::write(dir.join("harness-b-prompt.md"), prompt_b).unwrap();
            std::fs::write(
                dir.join("progress-readback.json"),
                serde_json::to_vec_pretty(&saved).unwrap(),
            )
            .unwrap();
            std::fs::write(
                dir.join("resumed-readback.json"),
                serde_json::to_vec_pretty(&restarted).unwrap(),
            )
            .unwrap();
            std::fs::write(dir.join("fixture-root.txt"), root.to_str().unwrap()).unwrap();
        }
    }));
    cleanup(&store, &id);
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    if std::env::var_os("CORRAL_PROGRESS_EVIDENCE_DIR").is_some() {
        // The evidence runner owns cleanup after driving this same fixture's board.
        println!("Retained fixture: {}", temp.keep().display());
    }
}

#[test]
fn json_input_rejects_special_files_and_unknown_fields() {
    // Given an initialized workspace; parsing is independent of a live engine.
    let temp = workspace();
    // When device input or an unknown payload is supplied.
    let device = cli(
        temp.path(),
        &["run", "progress", "T-001", "--file", "/dev/zero", "--json"],
    );
    let file = write_json(temp.path(), "invalid.json", &json!({"unexpected":true}));
    let invalid = cli(
        temp.path(),
        &[
            "run",
            "checkpoint",
            "T-001",
            "--file",
            file.to_str().unwrap(),
            "--json",
        ],
    );
    // Then neither command can mutate task state or consume unbounded terminal/device input.
    assert!(!device.status.success());
    assert!(!invalid.status.success());
    assert!(Store::open(temp.path()).unwrap().list().unwrap().is_empty());
    assert!(serde_json::from_slice::<Value>(&device.stderr).is_ok());
    assert!(serde_json::from_slice::<Value>(&invalid.stderr).is_ok());
}

#[test]
fn shipped_generic_fixture_helper_records_a_current_checkpoint() {
    // Given a committed fixture run, without needing a live terminal engine.
    let temp = workspace();
    let root = temp.path();
    let store = Store::open(root).unwrap();
    let task = store
        .create(
            "Fixture helper",
            Some("app"),
            corral_core::Status::Running,
            "",
        )
        .unwrap();
    let repo = root.join("app");
    let out = command("git", &["rev-parse", "HEAD"], &repo);
    assert!(out.status.success());
    let head = String::from_utf8(out.stdout).unwrap().trim().to_string();
    let run: corral_core::Run = serde_json::from_value(json!({
        "id":"R1", "agent":"generic", "worktree":repo, "branch":"main",
        "base":"main", "base_sha":head, "started":"2026-10-15T00:00:00Z",
        "checkpoint_required":true
    }))
    .unwrap();
    store
        .update(&task.id, |t| {
            t.runs.push(run);
            Ok(())
        })
        .unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/checkpoint-fixture.sh");

    // When the shipped shell fixture publishes its handoff through the real CLI.
    let out = command(
        "bash",
        &[
            script.to_str().unwrap(),
            env!("CARGO_BIN_EXE_corral"),
            &task.id,
        ],
        root,
    );

    // Then its guarded snapshot is current and it has not claimed completion.
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let read = value(root, &["run", "checkpoint", &task.id, "--json"]);
    assert_eq!(read["checkpoint_status"], "compatible");
    assert_eq!(read["checkpoints"][0]["checkpoint"]["input"]["run"], "R1");
    assert_eq!(
        store.get(&task.id).unwrap().status,
        corral_core::Status::Running
    );
}
