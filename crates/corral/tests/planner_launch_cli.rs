//! T-556: execute submitted planner commands through the real binary and an isolated engine.
#![cfg(target_os = "linux")]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use corral_core::agent::{ContainerRef, container_state};
use corral_core::{Status, Store};
use serde_json::{Value, json};

const MODEL: &str = "claude-sonnet-4-5";
const PLAN: &str = "---\ngoal: CLI fixture\nnon_goals: [No implementation]\ndecisions: [Keep existing contracts]\naffected_paths: [protected.txt]\ncontracts: []\ndependencies: []\nacceptance:\n  - scenario: Read source\n    expected: Source remains unchanged\nverification: [cargo test]\nowner_decisions: []\n---\n";

struct Fixture {
    tmp: tempfile::TempDir,
    store: Store,
    task: String,
}

fn write(path: &Path, bytes: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let app = root.join("app");
        std::fs::create_dir(&app).unwrap();
        write(&app.join("protected.txt"), "original\n");
        for dir in [
            "home", "config", "data", "state", "cache", "runtime", "engine",
        ] {
            std::fs::create_dir(root.join(dir)).unwrap();
        }
        std::fs::set_permissions(root.join("runtime"), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let engine = root.join("engine/herdr");
        write(&engine, include_str!("planner_launch/herdr.sh"));
        std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o700)).unwrap();
        let assets = root.join("assets");
        for (name, body) in [
            ("package.json", "{\"name\":\"planner-launch-fixture\"}"),
            ("bun.lock", "{}"),
            ("fixture.json", "{\"fixture\":true}"),
            (
                "compat.json",
                "{\"version\":1,\"verdict\":\"GO\",\"bun\":{\"range\":\"1.4.2\"},\"providers\":[\"anthropic\"]}",
            ),
            ("src/host/main.ts", "// fixture uses the compiled host\n"),
            ("src/roles/planner/index.ts", "export default {};\n"),
            ("src/roles/plan_reviewer/index.ts", "export default {};\n"),
            ("src/delivery/index.ts", "export default {};\n"),
            ("src/auth/index.ts", "export default {};\n"),
        ] {
            write(&assets.join(name), body);
        }
        let mut store = Store::init(root).unwrap();
        store.config.agent.assets_dir = Some(assets);
        store.config.dispatch.paused = false;
        store.config.limits.review = 4;
        store.config.limits.count_reviews = false;
        // Admission must not depend on other agents' current machine pressure.
        store.config.resources.max_load_per_core = 1_000_000.0;
        store.config.resources.min_mem_available = "1B".into();
        store.config.resources.min_disk_free = "1B".into();
        store.config.projects.insert(
            "app".into(),
            serde_json::from_value(json!({"path":"app"})).unwrap(),
        );
        store.config.profiles.insert(
            "native-planner".into(),
            serde_json::from_value(json!({
                "harness":"pi-native", "model":MODEL, "vendor":"anthropic", "effort":"high",
            }))
            .unwrap(),
        );
        store.save_config().unwrap();
        let task = store
            .create(
                "Planner launch",
                Some("app"),
                Status::Ready,
                "Read source only",
            )
            .unwrap()
            .id;
        Self { tmp, store, task }
    }

    fn root(&self) -> &Path {
        self.tmp.path()
    }

    fn isolated(&self, binary: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(binary);
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.root().join("home"))
            .env("XDG_CONFIG_HOME", self.root().join("config"))
            .env("XDG_DATA_HOME", self.root().join("data"))
            .env("XDG_STATE_HOME", self.root().join("state"))
            .env("XDG_CACHE_HOME", self.root().join("cache"))
            .env("XDG_RUNTIME_DIR", self.root().join("runtime"))
            .env("CORRAL_ROOT", self.root())
            .env("HERDR_BIN_PATH", self.root().join("engine/herdr"))
            .env("HERDR_SOCKET_PATH", self.root().join("engine/socket"))
            .current_dir(self.root())
            .stdin(Stdio::null());
        command
    }

    fn sandbox_usable(&self) -> bool {
        let app = self.root().join("app");
        let output = self
            .isolated("bwrap")
            .args([
                "--bind",
                "/",
                "/",
                "--dev",
                "/dev",
                "--proc",
                "/proc",
                "--ro-bind",
            ])
            .arg(&app)
            .arg(&app)
            .arg("--bind")
            .arg(self.store.dir())
            .arg(self.store.dir())
            .args(["--", "/bin/true"])
            .output();
        match output {
            Ok(out) if out.status.success() => true,
            other => {
                eprintln!(
                    "skipping planner launch regression: bubblewrap is not usable: {other:?}"
                );
                false
            }
        }
    }

    fn cli(&self, args: &[&str]) -> Output {
        self.isolated(env!("CARGO_BIN_EXE_corral"))
            .arg("--root")
            .arg(self.root())
            .args(args)
            .output()
            .unwrap()
    }

    fn success(&self, args: &[&str]) {
        let out = self.cli(args);
        let pane = std::fs::read_to_string(self.root().join("engine/output")).unwrap_or_default();
        assert!(
            out.status.success(),
            "{args:?}: {out:?}\nExecuted pane output:\n{pane}"
        );
    }

    fn compile_host(&self) {
        // Compile with the same toolchain and serde_json artifact as this integration test.
        // No provider runtime, network, or dependency installation is involved.
        let deps = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_owned();
        let serde = std::fs::read_dir(&deps)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                let name = path.file_name().unwrap().to_string_lossy();
                name.starts_with("libserde_json-") && name.ends_with(".rlib")
            })
            .expect("Cargo built serde_json for this integration test");
        let out = Command::new("rustc")
            .args(["--edition=2024", "--crate-name", "planner_launch_host"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/planner_launch/host.rs"))
            .arg("--extern")
            .arg(format!("serde_json={}", serde.display()))
            .arg("-L")
            .arg(format!("dependency={}", deps.display()))
            .arg("-o")
            .arg(self.root().join("assets/corral-pi-host"))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    }

    fn execute_submitted(&self) {
        // Complete the fake pane's handoff after submission releases its board
        // lock. Execute the unmodified command, including the actual sandbox.
        let command = std::fs::read_to_string(self.root().join("engine/launch-command")).unwrap();
        let out = self
            .isolated("/bin/sh")
            .args(["-c", &command])
            .output()
            .unwrap();
        let capture = [out.stdout.as_slice(), out.stderr.as_slice()].concat();
        write(&self.root().join("engine/output"), &capture);
        assert!(out.status.success(), "{out:?}");
    }

    fn proof(&self, role: &str) -> Value {
        let path = self
            .store
            .state_dir()
            .join(format!("planner-launch-{role}.json"));
        let bytes = std::fs::read(&path).unwrap_or_else(|error| {
            panic!(
                "host did not complete hello at {}: {error}\npane output: {}",
                path.display(),
                std::fs::read_to_string(self.root().join("engine/output")).unwrap_or_default()
            )
        });
        serde_json::from_slice(&bytes).unwrap()
    }

    fn assert_bound(
        &self,
        proof: &Value,
        state: &Value,
        role: &str,
        run: &str,
        review: Option<&str>,
        prompt: PathBuf,
    ) {
        let launch = &proof["launch"];
        assert_eq!(launch["role"], role);
        assert_eq!(launch["task"], self.task);
        assert_eq!(launch["run"], run);
        assert_eq!(launch["review"].as_str(), review);
        assert!(launch["callback"].is_null());
        assert_eq!(launch["provider"], "anthropic");
        assert_eq!(launch["model"], MODEL);
        assert_eq!(launch["effort"], "high");
        assert_eq!(launch["prompt_file"], prompt.to_str().unwrap());
        assert_eq!(launch["cwd"], self.root().join("app").to_str().unwrap());
        assert_eq!(proof["hello"]["state"], "committed");
        assert_eq!(proof["hello"]["identity"]["role"], role);
        assert_eq!(proof["hello"]["identity"]["run"], run);
        assert_eq!(proof["hello"]["identity"]["review"].as_str(), review);
        assert_eq!(proof["nonce_reuse"]["result"]["code"], "nonce_consumed");
        assert_eq!(state["binding"]["role"], role);
        assert_eq!(state["binding"]["health"], "healthy");
        assert_eq!(state["binding"]["generation"], 1);
        assert_eq!(state["generations"][0]["bound"]["request_id"], "ev-1-0");
        assert_eq!(
            state["generations"][0]["bound"]["session_id"],
            format!("fixture-{role}")
        );
        assert_eq!(state["generations"][0]["pane"], launch["pane"]);
        assert_eq!(
            std::fs::read_to_string(self.root().join("app/protected.txt")).unwrap(),
            "original\n"
        );
        assert!(
            !Path::new(launch["runtime_dir"].as_str().unwrap())
                .join("bootstrap.json")
                .exists(),
            "the executed launcher must consume its one-shot bootstrap"
        );
    }
}

#[test]
fn native_planner_executes_bootstrap_and_binds_the_stored_design_run() {
    // Given explicit fixture assets and a private board/project with no live herdr.
    let fx = Fixture::new();
    if !fx.sandbox_usable() {
        return;
    }
    fx.compile_host();
    // When the actual CLI submits, then its exact sandboxed pane command executes.
    fx.success(&[
        "dispatch",
        &fx.task,
        "--plan",
        "--profile",
        "native-planner",
    ]);
    fx.execute_submitted();
    // Then the host's real hello is persisted on D1, not an implementation run.
    let task = fx.store.get(&fx.task).unwrap();
    assert!(task.runs.is_empty());
    let run = &task.plan_runs[0];
    let proof = fx.proof("planner");
    let encoded = serde_json::to_value(run).unwrap();
    let container = ContainerRef::Planner {
        task: fx.task.clone(),
        run: "D1".into(),
    };
    assert_eq!(proof["hello"]["result"]["container"], container.key());
    assert_eq!(
        serde_json::to_value(container_state(&task, &container).unwrap()).unwrap(),
        encoded["agent_host"]
    );
    fx.assert_bound(
        &proof,
        &encoded["agent_host"],
        "planner",
        "D1",
        None,
        run.output_dir.join("prompt.md"),
    );
    assert_eq!(proof["read_only"], true);
    let command = std::fs::read_to_string(fx.root().join("engine/launch-command")).unwrap();
    assert!(command.contains("bwrap --bind / /"));
    assert!(command.contains(" agent launch --bootstrap "));
}

#[test]
fn native_plan_reviewer_executes_bootstrap_and_binds_the_stored_review() {
    // Given an owner-submitted plan, separate from any implementation run or callback.
    let fx = Fixture::new();
    if !fx.sandbox_usable() {
        return;
    }
    fx.compile_host();
    let plan = fx.root().join("plan.md");
    write(&plan, PLAN);
    fx.success(&["plan", "submit", &fx.task, plan.to_str().unwrap()]);
    // When an independent native reviewer is launched through the actual CLI.
    fx.success(&["plan", "review", &fx.task, "--reviewer", "native-planner"]);
    fx.execute_submitted();
    // Then P1/PR1's hello is stored on that review, with no implementation callback.
    let task = fx.store.get(&fx.task).unwrap();
    let review = &task.plans[0].reviews[0];
    let encoded = serde_json::to_value(review).unwrap();
    let container = ContainerRef::PlanReviewer {
        task: fx.task.clone(),
        plan: "P1".into(),
        review: "PR1".into(),
    };
    let proof = fx.proof("plan_reviewer");
    assert_eq!(proof["hello"]["result"]["container"], container.key());
    assert_eq!(
        serde_json::to_value(container_state(&task, &container).unwrap()).unwrap(),
        encoded["agent_host"]
    );
    fx.assert_bound(
        &proof,
        &encoded["agent_host"],
        "plan_reviewer",
        "P1",
        Some("PR1"),
        fx.store
            .prompts_dir()
            .join(format!("{}-P1-PR1-plan-review.md", fx.task)),
    );
    assert!(task.runs.is_empty());
}

#[test]
fn omo_planner_executes_its_rendered_command_inside_the_read_only_wrapper() {
    // Given an OmO profile whose inert command reports its actual arguments.
    let mut fx = Fixture::new();
    if !fx.sandbox_usable() {
        return;
    }
    fx.store.config.harnesses.insert(
        "omo".into(),
        serde_json::from_value(json!({
            "command": ": {model_args}; if printf changed > '{worktree}/protected.txt'; then exit 9; fi; printf '%s\\n' '{task}' '{prompt_file}' > '{prompt_file}.executed'",
            "flags": "omo",
        }))
        .unwrap(),
    );
    fx.store.config.profiles.insert(
        "omo-planner".into(),
        serde_json::from_value(json!({"harness":"omo", "model":"anthropic/fixture"})).unwrap(),
    );
    fx.store.save_config().unwrap();
    // When the real CLI dispatches through the same executing fake pane.
    fx.success(&["dispatch", &fx.task, "--plan", "--profile", "omo-planner"]);
    fx.execute_submitted();
    // Then the existing template ran, with the generated prompt and source write denied.
    let task = fx.store.get(&fx.task).unwrap();
    let prompt = task.plan_runs[0].output_dir.join("prompt.md");
    assert_eq!(
        std::fs::read_to_string(prompt.with_extension("md.executed")).unwrap(),
        format!("{}\n{}\n", fx.task, prompt.display())
    );
    assert_eq!(
        std::fs::read_to_string(fx.root().join("app/protected.txt")).unwrap(),
        "original\n"
    );
    assert!(serde_json::to_value(&task.plan_runs[0]).unwrap()["agent_host"].is_null());
    let command = std::fs::read_to_string(fx.root().join("engine/launch-command")).unwrap();
    assert!(command.contains("bwrap --bind / /"));
    assert!(!command.contains(" agent launch "));
}

#[test]
fn former_bare_agent_launch_is_rejected_without_a_bootstrap() {
    // Given the actual CLI and a private environment, without altering production code.
    let fx = Fixture::new();
    // When the former planner template's bare launch reaches Clap.
    let output = fx.cli(&["agent", "launch"]);
    // Then it fails at argument parsing instead of starting a host or touching herdr.
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(!fx.root().join("engine/launch-command").exists());
    eprintln!("former bare `corral agent launch` control: rejected with exit 2");
}
