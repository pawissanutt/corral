use std::process::{Command, Output, Stdio};

use corral_core::{OperatingMode, Status, Store};
use serde_json::Value;

struct Fixture {
    root: tempfile::TempDir,
    store: Store,
    id: String,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("app/.git")).unwrap();
        let store = Store::init(root.path()).unwrap();
        let id = store
            .create("Finite work", Some("app"), Status::Backlog, "Agreed work")
            .unwrap()
            .id;
        Self { root, store, id }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("XDG_CACHE_HOME", self.root.path().join("cache"))
            .env("HERDR_BIN_PATH", "/nonexistent/herdr")
            .current_dir(self.root.path())
            .stdin(Stdio::null())
            .arg("--root")
            .arg(self.root.path());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> Output {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_slice(&self.ok(args).stdout).unwrap()
    }

    fn preview(&self) -> Value {
        self.json(&[
            "mode",
            "autopilot",
            "--task",
            &self.id,
            "--dispatch",
            "--json",
        ])
    }

    fn confirm(&self, preview: &Value) -> Output {
        self.run(&[
            "mode",
            "autopilot",
            "--task",
            &self.id,
            "--dispatch",
            "--confirm",
            preview["id"].as_str().unwrap(),
            "--json",
        ])
    }

    fn grant(&self) -> Value {
        let output = self.confirm(&self.preview());
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

#[test]
fn preview_is_read_only_and_shared_with_legacy_selection() {
    // Given a finite project with an explicit owner-only acceptance exception.
    let fixture = Fixture::new();
    fixture
        .store
        .update(&fixture.id, |task| {
            task.review = Some(corral_core::Accept::Owner);
            Ok(())
        })
        .unwrap();
    let config = std::fs::read(fixture.store.config_path()).unwrap();
    // When both selection entry points preview the same policy.
    let preview = fixture.preview();
    let legacy = fixture.ok(&["automation", "preview", "--task", &fixture.id, "--dispatch"]);
    let text = String::from_utf8(legacy.stdout).unwrap();
    let legacy: Value =
        serde_json::from_str(text.split("\nExact confirmation digest:").next().unwrap()).unwrap();
    // Then identity, exact membership, authority and exceptions agree without a grant.
    assert_eq!(preview, legacy);
    assert_eq!(
        preview["policy"]["scope"]["tasks"][&fixture.id]["project"],
        "app"
    );
    assert_eq!(
        preview["constraints"][&fixture.id]["owner_acceptance"],
        true
    );
    assert_eq!(preview["policy"]["authority"]["dispatch"], true);
    assert_eq!(preview["policy"]["authority"]["accept"], false);
    assert_eq!(std::fs::read(fixture.store.config_path()).unwrap(), config);
    assert!(fixture.store.operating_state().unwrap().approval.is_none());
}

#[test]
fn nonterminal_without_exact_confirmation_never_writes_a_grant() {
    // Given no active authorization.
    let fixture = Fixture::new();
    let config = std::fs::read(fixture.store.config_path()).unwrap();
    // When ordinary nonterminal activation, a blanket yes, or a wrong digest is offered.
    fixture.ok(&["mode", "autopilot", "--task", &fixture.id, "--dispatch"]);
    for tail in [vec!["--yes"], vec!["--confirm", "wrong"]] {
        let mut args = vec!["mode", "autopilot", "--task", &fixture.id, "--dispatch"];
        args.extend(tail);
        assert!(!fixture.run(&args).status.success());
    }
    // Then only an exact owner-confirmed preview can change authority.
    assert_eq!(std::fs::read(fixture.store.config_path()).unwrap(), config);
    assert!(fixture.store.operating_state().unwrap().approval.is_none());
}

#[test]
fn confirmed_project_selection_is_finite_and_preserves_exclusions_and_exceptions() {
    // Given project work, an excluded task, an Idea, and an owner exception.
    let fixture = Fixture::new();
    let excluded = fixture
        .store
        .create("Excluded", Some("app"), Status::Ready, "")
        .unwrap();
    let idea = fixture
        .store
        .create("Future idea", Some("app"), Status::Idea, "")
        .unwrap();
    fixture
        .store
        .update(&fixture.id, |task| {
            task.review = Some(corral_core::Accept::Owner);
            Ok(())
        })
        .unwrap();
    let preview = fixture.json(&[
        "mode",
        "autopilot",
        "--project",
        "app",
        "--exclude",
        &excluded.id,
        "--dispatch",
        "--accept",
        "--plan-accept",
        "--json",
    ]);
    // When the owner explicitly confirms that preview and later files new work.
    let confirmed = fixture.json(&[
        "mode",
        "autopilot",
        "--project",
        "app",
        "--exclude",
        &excluded.id,
        "--dispatch",
        "--accept",
        "--plan-accept",
        "--confirm",
        preview["id"].as_str().unwrap(),
        "--json",
    ]);
    let future = fixture
        .store
        .create("New backlog", Some("app"), Status::Backlog, "")
        .unwrap();
    // Then the approval binds the displayed manifest, never future work or owner acceptance.
    assert_eq!(confirmed["mode"], "autopilot");
    assert_eq!(confirmed["approval"]["authorization"], preview["id"]);
    assert_eq!(confirmed["approval"]["digest"], preview["grant_digest"]);
    let current = fixture.json(&["mode", "status", "--json"]);
    let members = current["policy"]["scope"]["tasks"].as_object().unwrap();
    assert_eq!(members.len(), 1);
    assert!(members.contains_key(&fixture.id));
    for id in [&excluded.id, &idea.id, &future.id] {
        assert!(!members.contains_key(id));
    }
    assert_eq!(
        current["policy"]["scope"]["exclusions"],
        serde_json::json!([excluded.id])
    );
    assert!(
        fixture.store.effective_operating_policy().unwrap().tasks[&fixture.id]
            .refusals
            .contains_key(&corral_core::OperatingActionKind::Accept)
    );
}

#[test]
fn changed_task_scope_policy_or_repository_rejects_the_old_preview() {
    for change in ["task", "scope", "policy", "repository"] {
        // Given a project preview, before any grant.
        let fixture = Fixture::new();
        let preview = fixture.json(&[
            "mode",
            "autopilot",
            "--project",
            "app",
            "--dispatch",
            "--json",
        ]);
        // When a relevant input changes after preview.
        match change {
            "task" => {
                fixture
                    .store
                    .update(&fixture.id, |task| {
                        task.body.push_str("\nChanged");
                        Ok(())
                    })
                    .unwrap();
            }
            "scope" => {
                fixture
                    .store
                    .create("Added", Some("app"), Status::Backlog, "")
                    .unwrap();
            }
            "policy" => {
                fixture.ok(&["mode", "pause"]);
            }
            "repository" => {
                std::fs::rename(
                    fixture.root.path().join("app"),
                    fixture.root.path().join("old-app"),
                )
                .unwrap();
                std::fs::create_dir_all(fixture.root.path().join("app/.git")).unwrap();
            }
            _ => unreachable!(),
        }
        let before = std::fs::read(fixture.store.config_path()).unwrap();
        let output = fixture.run(&[
            "mode",
            "autopilot",
            "--project",
            "app",
            "--dispatch",
            "--confirm",
            preview["id"].as_str().unwrap(),
            "--json",
        ]);
        // Then old confirmation is refused without a policy write.
        assert!(!output.status.success(), "{change}: {output:?}");
        assert_eq!(std::fs::read(fixture.store.config_path()).unwrap(), before);
        assert!(fixture.store.operating_state().unwrap().approval.is_none());
    }
}

#[test]
fn pause_resume_assist_and_legacy_status_use_one_policy() {
    // Given a valid grant, with an intentionally disabled legacy review constraint.
    let fixture = Fixture::new();
    fixture
        .store
        .edit_config(|doc| doc.set_str("review.autopilot", "false"))
        .unwrap();
    fixture.grant();
    let approval = fixture.store.operating_state().unwrap().approval.unwrap();
    // When legacy off pauses, legacy on is inert, queue resume is limited, and owner resumes.
    let paused = fixture.json(&["autopilot", "off", "--json"]);
    assert_eq!(paused["mode"], "autopilot");
    assert_eq!(paused["paused"], true);
    fixture.ok(&["autopilot", "on"]);
    fixture.ok(&["queue", "resume"]);
    assert!(
        fixture.json(&["mode", "status", "--json"])["paused"]
            .as_bool()
            .unwrap()
    );
    fixture.ok(&["mode", "resume"]);
    let status = fixture.json(&["mode", "status", "--json"]);
    // Then every alias shares status, no legacy flag changes, and pause retained the grant.
    assert_eq!(status, fixture.json(&["autopilot", "status", "--json"]));
    assert_eq!(status["paused"], false);
    assert_eq!(
        fixture.store.operating_state().unwrap().approval.unwrap(),
        approval
    );
    assert!(
        !Store::open(fixture.root.path())
            .unwrap()
            .config
            .autopilot_enabled()
    );
    fixture.ok(&["mode", "assist"]);
    assert_eq!(
        fixture.store.effective_operating_policy().unwrap().mode,
        OperatingMode::Assist
    );
    assert!(!fixture.run(&["mode", "resume"]).status.success());
}

#[test]
fn generic_config_and_legacy_on_cannot_create_independent_automation() {
    // Given an Assist workspace.
    let fixture = Fixture::new();
    fixture
        .store
        .edit_config(|doc| {
            doc.unset("review")?;
            Ok(())
        })
        .unwrap();
    let config = std::fs::read(fixture.store.config_path()).unwrap();
    // When every generic policy edit and the old activation command is attempted.
    for (key, value) in [
        ("automation.mode", "autopilot"),
        ("automation", "{ mode = 'autopilot' }"),
        ("automation.paused", "false"),
        ("review.autopilot", "true"),
        ("review", "{ autopilot = true }"),
        (r#""automation".mode"#, "autopilot"),
        ("'automation' . paused", "false"),
        (r#" automation . "paused" "#, "false"),
        (r#""auto\u006dation"."paused""#, "false"),
        (r#""automation""#, "{ mode = 'autopilot' }"),
        (" 'automation' ", "{ paused = false }"),
        (r#""review".autopilot"#, "false"),
        (r#"review . "autopilot""#, "true"),
        ("'review' . 'autopilot'", "true"),
        (r#""rev\u0069ew"."auto\u0070ilot""#, "true"),
        (r#""review""#, "{ autopilot = true }"),
        (" 'review' ", "{ autopilot = true }"),
    ] {
        for layer in ["--workspace", "--user"] {
            assert!(
                !fixture
                    .run(&["config", "set", key, value, layer])
                    .status
                    .success(),
                "{key} ({layer})"
            );
        }
    }
    fixture.ok(&["autopilot", "on"]);
    // Then no settings, receipts or authority changed.
    assert_eq!(std::fs::read(fixture.store.config_path()).unwrap(), config);
    assert!(fixture.store.operating_state().unwrap().approval.is_none());
    assert_eq!(
        fixture.store.effective_operating_policy().unwrap().mode,
        OperatingMode::Assist
    );
}

#[test]
fn pm_and_agent_generic_key_aliases_cannot_resume_a_paused_grant() {
    // Given a valid grant paused by the owner.
    let fixture = Fixture::new();
    fixture.grant();
    assert!(
        fixture
            .store
            .effective_operating_policy()
            .unwrap()
            .allows(&fixture.id, corral_core::OperatingActionKind::Dispatch)
    );
    fixture.ok(&["mode", "pause"]);
    let config = std::fs::read(fixture.store.config_path()).unwrap();
    let approval = fixture.store.operating_state().unwrap().approval.unwrap();
    for (marker, value) in [("CORRAL_PM", "1"), ("AI_AGENT", "senpi")] {
        for key in [
            "automation.paused",
            r#""automation".paused"#,
            "'automation' . paused",
            r#" automation . "paused" "#,
            r#""auto\u006dation"."pau\u0073ed""#,
        ] {
            // When a non-owner tries to lift the pause through a TOML-equivalent path.
            let output = fixture
                .command()
                .env(marker, value)
                .args(["config", "set", key, "false"])
                .output()
                .unwrap();
            // Then no config or approval bytes change, and automatic work stays unauthorized.
            assert_eq!(output.status.code(), Some(1), "{marker}: {key}: {output:?}");
            assert_eq!(std::fs::read(fixture.store.config_path()).unwrap(), config);
            assert_eq!(
                fixture.store.operating_state().unwrap().approval.unwrap(),
                approval
            );
            let status = fixture.json(&["mode", "status", "--json"]);
            assert_eq!(status["mode"], "autopilot");
            assert_eq!(status["paused"], true);
            assert!(status["actions"].as_array().unwrap().is_empty());
            assert!(
                !fixture
                    .store
                    .effective_operating_policy()
                    .unwrap()
                    .allows(&fixture.id, corral_core::OperatingActionKind::Dispatch)
            );
        }
    }
}

#[test]
fn pm_and_agent_key_aliases_cannot_toggle_a_granted_legacy_constraint() {
    // Given a valid grant narrowed by an explicit legacy-disabled constraint.
    let fixture = Fixture::new();
    fixture
        .store
        .edit_config(|doc| doc.set_str("review.autopilot", "false"))
        .unwrap();
    fixture.grant();
    let config = std::fs::read(fixture.store.config_path()).unwrap();
    let approval = fixture.store.operating_state().unwrap().approval.unwrap();
    for (marker, value) in [("CORRAL_PM", "1"), ("AI_AGENT", "senpi")] {
        for key in [
            "review.autopilot",
            r#""review".autopilot"#,
            r#"review . "autopilot""#,
            "'review' . 'autopilot'",
            r#""rev\u0069ew"."auto\u0070ilot""#,
        ] {
            for setting in ["true", "false"] {
                // When a non-owner uses an equivalent key spelling to edit the legacy switch.
                let output = fixture
                    .command()
                    .env(marker, value)
                    .args(["config", "set", key, setting])
                    .output()
                    .unwrap();
                // Then the switch cannot change independently or restore automatic work.
                assert_eq!(output.status.code(), Some(1), "{marker}: {key}: {output:?}");
                assert_eq!(std::fs::read(fixture.store.config_path()).unwrap(), config);
                assert_eq!(
                    fixture.store.operating_state().unwrap().approval.unwrap(),
                    approval
                );
                let status = fixture.json(&["mode", "status", "--json"]);
                assert_eq!(status["paused"], false);
                assert_eq!(status["mode"], "autopilot");
                assert!(status["actions"].as_array().unwrap().is_empty());
                assert!(
                    !fixture
                        .store
                        .effective_operating_policy()
                        .unwrap()
                        .allows(&fixture.id, corral_core::OperatingActionKind::Dispatch)
                );
            }
        }
    }
}

#[test]
fn quoted_unrelated_config_keys_remain_editable() {
    // Given ordinary settings outside the operating control paths.
    let fixture = Fixture::new();
    // When the owner edits them through quoted, whitespace-separated TOML paths.
    fixture.ok(&["config", "set", r#""ui" . "mouse""#, "false"]);
    fixture.ok(&["config", "set", r#""review" . "max_rounds""#, "3"]);
    // Then the semantic fields are changed without manufacturing an operating grant.
    let config = Store::open(fixture.root.path()).unwrap().config;
    assert!(!config.ui.mouse);
    assert_eq!(config.review.max_rounds, Some(3));
    assert!(fixture.store.operating_state().unwrap().approval.is_none());
}

#[test]
fn agent_or_pm_cannot_confirm_or_resume_from_nonterminal_entry() {
    // Given the exact preview but agent/PM provenance.
    let fixture = Fixture::new();
    let preview = fixture.preview();
    // When either caller presents the exact digest.
    for (key, value) in [
        ("CORRAL_PM", "1"),
        ("AI_AGENT", "senpi"),
        ("PI_CODING_AGENT", "true"),
    ] {
        let output = fixture
            .command()
            .env(key, value)
            .args([
                "mode",
                "autopilot",
                "--task",
                &fixture.id,
                "--dispatch",
                "--confirm",
                preview["id"].as_str().unwrap(),
            ])
            .output()
            .unwrap();
        // Then a digest is not a caller-controlled owner override.
        assert!(!output.status.success());
        assert!(fixture.store.operating_state().unwrap().approval.is_none());
    }
}

#[test]
fn status_and_pm_workset_share_read_only_counts_and_blockers() {
    // Given a scoped task with a durable start hold.
    let fixture = Fixture::new();
    fixture
        .store
        .update(&fixture.id, |task| {
            task.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    fixture.grant();
    let events = std::fs::read(fixture.store.dir().join("events.jsonl")).unwrap();
    let config = std::fs::read(fixture.store.config_path()).unwrap();
    // When mode, legacy and PM readbacks are requested repeatedly.
    let mode = fixture.json(&["mode", "status", "--json"]);
    let pm = fixture.json(&["pm", "next", "--json"]);
    let brief = fixture.json(&["pm", "brief", "--json"]);
    // Then status and scoped counts agree, and none writes presentation or policy state.
    for field in [
        "status",
        "mode",
        "paused",
        "progress",
        "blockers",
        "decisions",
        "independent_work_continues",
    ] {
        assert_eq!(mode[field], pm["workset"][field], "{field}");
        assert_eq!(mode[field], brief["operating"][field], "{field}");
    }
    assert_eq!(mode, fixture.json(&["autopilot", "--json"]));
    assert_eq!(
        std::fs::read(fixture.store.dir().join("events.jsonl")).unwrap(),
        events
    );
    assert_eq!(std::fs::read(fixture.store.config_path()).unwrap(), config);
}

#[test]
fn pause_and_assist_preserve_running_identity_and_receipts() {
    // Given a persisted running identity.
    let fixture = Fixture::new();
    let run: corral_core::Run = serde_json::from_value(serde_json::json!({
        "id": "R1", "agent": "generic", "worktree": fixture.root.path().join("worktree"),
        "branch": "corral/fixture", "base": "main", "base_sha": "fixture",
        "started": "2026-10-19T00:00:00Z"
    }))
    .unwrap();
    fixture
        .store
        .update(&fixture.id, |task| {
            task.status = Status::Running;
            task.runs.push(run);
            Ok(())
        })
        .unwrap();
    fixture.grant();
    let before = fixture.store.get(&fixture.id).unwrap();
    // When the owner pauses and withdraws automatic authority.
    fixture.ok(&["mode", "pause"]);
    fixture.ok(&["mode", "assist"]);
    // Then no worker state or evidence is killed, removed, or rewritten.
    let after = fixture.store.get(&fixture.id).unwrap();
    assert_eq!(after.runs, before.runs);
    assert_eq!(after.status, Status::Running);
}

#[cfg(unix)]
fn terminal_confirmation(
    fixture: &Fixture,
    input: &str,
    after_preview: impl FnOnce(),
) -> (std::process::ExitStatus, String) {
    use std::io::{Read, Write};
    use std::sync::mpsc;
    use std::time::Duration;

    // Subscribe to the actual terminal prompt before starting the command.
    let (arrived, arrival) = mpsc::channel();
    let base = fixture.command();
    let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
    let command = format!(
        "{} --root {} mode autopilot --task {} --dispatch",
        quote(env!("CARGO_BIN_EXE_corral")),
        quote(fixture.root.path().to_str().unwrap()),
        fixture.id,
    );
    let mut child = Command::new("script")
        .env_clear()
        .envs(
            base.get_envs()
                .filter_map(|(key, value)| value.map(|value| (key, value))),
        )
        .current_dir(fixture.root.path())
        .args(["--quiet", "--return", "--command", &command, "/dev/null"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        let mut signal = Some(arrived);
        loop {
            let count = output.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
            if String::from_utf8_lossy(&bytes).contains("(Enter cancels): ")
                && let Some(signal) = signal.take()
            {
                signal.send(()).unwrap();
            }
        }
        String::from_utf8(bytes).unwrap()
    });
    if let Err(error) = arrival.recv_timeout(Duration::from_secs(20)) {
        child.kill().unwrap();
        child.wait().unwrap();
        panic!(
            "terminal confirmation never arrived: {error}; {}",
            reader.join().unwrap()
        );
    }
    after_preview();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(input.as_bytes()).unwrap();
    stdin.write_all(b"\n").unwrap();
    stdin.flush().unwrap();
    drop(stdin);
    let result = child.wait().unwrap();
    (result, reader.join().unwrap())
}

#[test]
#[cfg(unix)]
fn terminal_confirmation_can_cancel_without_a_grant() {
    // Given the actual terminal confirmation surface.
    let fixture = Fixture::new();
    let before = std::fs::read(fixture.store.config_path()).unwrap();
    // When the owner cancels with Enter.
    let (status, output) = terminal_confirmation(&fixture, "", || {});
    // Then cancellation is successful but leaves authority absent.
    assert!(status.success(), "{output}");
    assert_eq!(std::fs::read(fixture.store.config_path()).unwrap(), before);
    assert!(fixture.store.operating_state().unwrap().approval.is_none());
}

#[test]
#[cfg(unix)]
fn terminal_confirmation_grants_only_the_displayed_identity() {
    // Given an unchanged exact preview.
    let fixture = Fixture::new();
    let preview = fixture.preview();
    // When the owner types its full identity at the actual terminal prompt.
    let (status, output) = terminal_confirmation(&fixture, preview["id"].as_str().unwrap(), || {});
    // Then the persisted grant is that same preview, not a reconstructed broader selection.
    assert!(status.success(), "{output}");
    let approval = fixture.store.operating_state().unwrap().approval.unwrap();
    assert_eq!(approval.authorization, preview["id"].as_str().unwrap());
    assert_eq!(approval.digest, preview["grant_digest"].as_str().unwrap());
}

#[test]
#[cfg(unix)]
fn terminal_task_change_while_prompt_is_open_rejects_confirmation() {
    // Given a subscribed actual terminal prompt for an exact preview.
    let fixture = Fixture::new();
    let preview = fixture.preview();
    // When another context changes the task before the owner answers.
    let (status, _) = terminal_confirmation(&fixture, preview["id"].as_str().unwrap(), || {
        Store::open(fixture.root.path())
            .unwrap()
            .update(&fixture.id, |task| {
                task.body.push_str("\nChanged while confirming");
                Ok(())
            })
            .unwrap();
    });
    // Then the serialized final confirmation refuses the stale snapshot.
    assert!(!status.success());
    assert!(fixture.store.operating_state().unwrap().approval.is_none());
}
