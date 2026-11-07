//! CLI boundaries only; policy permutations and crash journals are tested in corral-ops.
use std::path::Path;
use std::process::{Command, Output, Stdio};

use corral_core::{Status, Store};
use corral_ops::Ctx;
use serde_json::{Value, json};

const PLAN: &str = "---\ngoal: recovery CLI fixture\nnon_goals: []\ndecisions: []\naffected_paths: [a.rs]\ncontracts: []\ndependencies: []\nacceptance: [{scenario: check, expected: passes}]\nverification: [check]\nowner_decisions: []\n---\n";

struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
    id: String,
    other: String,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("app")).unwrap();
        let store = Store::init_with(dir.path(), |store| {
            std::fs::write(store.config_path(), "[projects.app]\npath = \"app\"\n[projects.app.review]\nreviewer = \"sol\"\n[pm]\nagent = \"sol\"\n[resources]\nmax_load_per_core = 1000000.0\nmin_mem_available = \"1B\"\nmin_disk_free = \"1B\"\n[harnesses.fixture]\ncommand = \"true {model_args}\"\nmodel_flag = \"--model\"\n[profiles.sol]\nharness = \"fixture\"\nmodel = \"author-model\"\nvendor = \"openai\"\n[profiles.alternate]\nharness = \"fixture\"\nmodel = \"reviewer-model\"\nvendor = \"openai\"\n")?;
            Ok(())
        }).unwrap();
        let task = store
            .create(
                "Recovery CLI",
                Some("app"),
                Status::Ready,
                "Bound specification",
            )
            .unwrap();
        store.update(&task.id, |task| {
            task.plan_runs.push(serde_json::from_value(json!({
                "id":"D1", "profile":"sol", "harness":"fixture", "model":"author-model", "vendor":"openai",
                "spec_revision":corral_ops::plan_spec_revision(task)?, "started":corral_core::clock::now(),
                "output_dir":"output", "rounds":0, "state":"planning"
            }))?);
            Ok(())
        }).unwrap();
        let source = dir.path().join("plan.md");
        std::fs::write(&source, PLAN).unwrap();
        corral_ops::plan_submit(&Ctx::from_store(store.clone()), &task.id, &source).unwrap();
        std::fs::write(
            dir.path().join("notes.md"),
            "No owner questions; accept this fixture plan only.",
        )
        .unwrap();
        let other = store
            .create("Unrelated", None, Status::Backlog, "Preserve me")
            .unwrap()
            .id;
        Self {
            dir,
            store,
            id: task.id,
            other,
        }
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        for name in [
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
            "XDG_RUNTIME_DIR",
        ] {
            let private = self.dir.path().join("private");
            std::fs::create_dir_all(&private).unwrap();
            command.env(name, &private);
        }
        command.env("HERDR_BIN_PATH", self.dir.path().join("no-herdr"));
        command.current_dir(self.dir.path()).stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_corral"))
            .args(args)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn args(&self) -> Vec<&str> {
        vec![
            "recover",
            &self.id,
            "--action",
            "accept-plan",
            "--plan",
            "P1",
            "--notes-file",
            "notes.md",
            "--waive",
            "independent-plan-review",
        ]
    }

    fn preview(&self) -> Value {
        let mut args = self.args();
        args.push("--json");
        serde_json::from_str(&self.ok(&args)).unwrap()
    }

    fn snapshot(&self) -> Value {
        json!({
            "task":self.store.get(&self.id).unwrap(),
            "other":self.store.get(&self.other).unwrap(),
            "events":self.store.events().unwrap(),
            "config":std::fs::read(self.store.config_path()).unwrap(),
            "plan":std::fs::read(self.dir.path().join("plan.md")).unwrap(),
        })
    }

    #[cfg(unix)]
    fn terminal(&self, response: &str, digest: &str) -> Output {
        let mut args = self.args();
        args.extend([
            "--apply",
            digest,
            "--reason",
            "Accept this exact fixture only",
            "--json",
        ]);
        self.command("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/recovery_cli/owner.py"))
            .args([response, &self.id, digest, env!("CARGO_BIN_EXE_corral")])
            .args(args)
            .output()
            .unwrap()
    }
}

#[test]
fn previews_history_and_explicit_target_checks_never_mutate() {
    let f = Fixture::new();
    // An observed open would emit a config change: preview must not consume it.
    let mut config = f.store.config.clone();
    config
        .projects
        .get_mut("app")
        .unwrap()
        .review
        .require_cross_family = Some(true);
    std::fs::write(
        f.store.config_path(),
        toml::to_string_pretty(&config).unwrap(),
    )
    .unwrap();
    let before = f.snapshot();
    let human = f.ok(&f.args());
    assert!(human.contains("independent-plan-review"));
    let preview = f.preview();
    assert_eq!(preview["request"]["action"]["action"], "accept-plan");
    assert_eq!(preview["checks"][0]["disposition"], "waived");
    assert_eq!(preview["request"]["target"]["identity"]["plan"], "P1");
    assert!(
        preview["remaining_blockers"].as_array().unwrap().is_empty(),
        "{preview}"
    );
    assert_eq!(
        f.ok(&["recover", &f.id, "--history", "--json"]).trim(),
        "[]"
    );
    let mut stale_target = f.args();
    let position = stale_target.iter().position(|a| *a == "P1").unwrap();
    stale_target[position] = "P0";
    assert!(!f.run(&stale_target).status.success());
    assert_eq!(before, f.snapshot());
}

#[test]
fn redirected_or_managed_authorization_and_stale_digest_refuse_without_effects() {
    let f = Fixture::new();
    let preview = f.preview();
    let digest = preview["digest"].as_str().unwrap();
    let before = f.snapshot();
    for (reason, supplied, managed) in [
        ("specific reason", digest, false),
        (" ", digest, false),
        ("specific reason", "stale-digest", false),
        ("specific reason", digest, true),
    ] {
        let mut args = f.args();
        args.extend(["--apply", supplied, "--reason", reason]);
        let mut command = f.command(env!("CARGO_BIN_EXE_corral"));
        command.args(args);
        if managed {
            command.env("CORRAL_AGENT_ROLE", "worker");
        }
        let output = command.output().unwrap();
        assert!(!output.status.success(), "{output:?}");
        if managed {
            assert!(String::from_utf8_lossy(&output.stderr).contains("managed context"));
        }
        assert_eq!(before, f.snapshot());
    }
}

#[cfg(unix)]
#[test]
fn local_terminal_cancel_then_exact_confirmation_records_one_owner_waiver() {
    let f = Fixture::new();
    let preview = f.preview();
    let digest = preview["digest"].as_str().unwrap();
    let before = f.snapshot();
    let cancelled = f.terminal("cancel", digest);
    assert!(!cancelled.status.success(), "{cancelled:?}");
    assert_eq!(before, f.snapshot());
    let accepted = f.terminal("confirm", digest);
    assert!(
        accepted.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&accepted.stdout),
        String::from_utf8_lossy(&accepted.stderr)
    );
    let capture = String::from_utf8(accepted.stdout).unwrap();
    assert!(capture.contains(digest));
    assert!(capture.contains("independent-plan-review"));
    assert!(capture.contains("Accept this exact fixture only"));
    println!("Private fixture terminal capture:\n{capture}");
    let history: Value =
        serde_json::from_str(&f.ok(&["recover", &f.id, "--history", "--json"])).unwrap();
    assert_eq!(history.as_array().unwrap().len(), 1);
    let record = &history[0];
    assert_eq!(
        record["outcomes"].as_array().unwrap().last().unwrap()["state"],
        "applied"
    );
    assert_eq!(
        record["authorization"]["attestation"]["kind"],
        "local_owner_attestation"
    );
    assert_eq!(record["authorization"]["attestation"]["interaction"], "cli");
    assert_eq!(
        record["authorization"]["reason"],
        "Accept this exact fixture only"
    );
    assert!(record["links"].as_array().unwrap().iter().any(
        |link| link["acceptance"]["acceptance_mode"] == "owner_recovery"
            && link["acceptance"]["policy_satisfied"] == false
    ));
    let task = f.store.get(&f.id).unwrap();
    assert!(task.dispatch_hold);
    assert_eq!(
        before["task"]["plans"][0]["artifact"],
        json!(task.plans[0].artifact)
    );
    assert!(!task.plans[0].reviews.iter().any(|r| r.reviewer != "owner"));
    assert_eq!(
        before["other"],
        serde_json::to_value(f.store.get(&f.other).unwrap()).unwrap()
    );
    assert_eq!(
        before["config"],
        json!(std::fs::read(f.store.config_path()).unwrap())
    );
    assert_eq!(
        before["plan"],
        json!(std::fs::read(f.dir.path().join("plan.md")).unwrap())
    );
    // The original digest cannot authorize a new operation after acceptance changed the target.
    let replay = f.terminal("confirm", digest);
    assert!(!replay.status.success());
    let history: Value =
        serde_json::from_str(&f.ok(&["recover", &f.id, "--history", "--json"])).unwrap();
    assert_eq!(history.as_array().unwrap().len(), 1);
    let shown: Value = serde_json::from_str(&f.ok(&["plan", "show", &f.id, "--json"])).unwrap();
    assert_eq!(shown["recoveries"], history);
}
