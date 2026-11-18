#![cfg(unix)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use corral_engine::{Actor, Applied, Check, CheckResult, Event, EventKind, Status, Task};
use serde::de::DeserializeOwned;

const BIN: &str = env!("CARGO_BIN_EXE_corral-next");

struct Workspace {
    _fixture: tempfile::TempDir,
    caller: PathBuf,
    root: PathBuf,
    repo: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let fixture = tempfile::Builder::new()
            .prefix("corral cli ")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .unwrap();
        let caller = fixture.path().to_owned();
        let root = caller.join("workspace");
        let repo = root.join("project");
        fs::create_dir_all(&repo).unwrap();
        let workspace = Self {
            _fixture: fixture,
            caller,
            root,
            repo,
        };
        workspace.git(&["init", "-b", "main"]);
        for (key, value) in [
            ("user.name", "Corral CLI Test"),
            ("user.email", "corral@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", "/dev/null"),
        ] {
            workspace.git(&["config", key, value]);
        }
        fs::write(workspace.repo.join(".gitignore"), "/.worktrees/\n").unwrap();
        workspace.git(&["add", ".gitignore"]);
        workspace.git(&["commit", "-m", "base"]);
        workspace.ok(&["init", "--first-id", "42"]);
        let config = workspace.root.join(".corral-next/corral.toml");
        assert!(
            fs::read_to_string(&config)
                .unwrap()
                .contains("# [projects.example]")
        );
        fs::write(
            config,
            r#"
[runner]
mode = "process"

[harnesses.test]
command = '''
set -eu
printf '%s\n' "$CORRAL_TASK" > landed.txt
git add landed.txt
git commit -m "fixture agent change"
{corral} done {task} --summary 'Fixture agent finished'
'''

[profiles.test]
harness = "test"

[projects.project]
path = "project"
profile = "test"
gate = ["test -f landed.txt"]
"#,
        )
        .unwrap();
        workspace
    }

    fn command(&self, program: &str, cwd: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(cwd)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
        for key in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "CORRAL_NEXT_BIN",
            "CORRAL_NEXT_ROOT",
            "CORRAL_ACTOR",
        ] {
            command.env_remove(key);
        }
        command
    }

    fn git_output(&self, args: &[&str]) -> Output {
        self.command("git", &self.repo).args(args).output().unwrap()
    }

    fn git(&self, args: &[&str]) -> String {
        success(self.git_output(args))
    }

    fn cli(&self, args: &[&str]) -> Output {
        self.command(BIN, &self.caller)
            .arg("--root")
            .arg(&self.root)
            .args(args)
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        success(self.cli(args))
    }

    fn json<T: DeserializeOwned>(&self, args: &[&str]) -> T {
        let args: Vec<_> = args.iter().copied().chain(["--json"]).collect();
        serde_json::from_str(&self.ok(&args)).unwrap()
    }

    fn new_task(&self, title: &str, extra: &[&str]) -> Applied {
        let args: Vec<_> = ["new", title, "-p", "project"]
            .into_iter()
            .chain(extra.iter().copied())
            .collect();
        self.json(&args)
    }

    fn wait_ready(&self, id: &str, after: u64) -> Event {
        let output = self.cli(&[
            "wait",
            "--for",
            "ready,error,blocked",
            "--task",
            id,
            "--after",
            &after.to_string(),
            "--timeout",
            "10s",
            "--json",
        ]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{} (worker log: {:?})",
            String::from_utf8_lossy(&output.stderr),
            fs::read_to_string(
                self.root
                    .join(format!(".corral-next/runs/{id}/R1/session.log"))
            )
        );
        let event: Event = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(event.kind, EventKind::Ready, "{event:?}");
        assert_eq!(event.task.as_deref(), Some(id));
        assert!(event.seq > after);
        event
    }
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn headless_agent_lands_and_a_refusal_command_is_runnable() {
    let ws = Workspace::new();
    let created = ws.new_task("land a file", &[]);
    let id = created.task.unwrap().id;
    assert_eq!(id, "T-042");
    let run = ws.json::<Applied>(&["dispatch", &id]).run.unwrap();
    ws.wait_ready(&id, created.seq);
    let landed: Applied = ws.json(&["approve", &id]);
    assert_eq!(landed.task.unwrap().status, Status::Done);
    assert_eq!(ws.git(&["show", "main:landed.txt"]).trim_end(), id);
    assert!(!run.worktree.exists());
    assert!(
        !ws.git_output(&[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", run.branch)
        ])
        .status
        .success()
    );

    let view: serde_json::Value = ws.json(&["show", &id]);
    let task: Task = serde_json::from_value(view["task"].clone()).unwrap();
    assert_eq!(task.status, Status::Done);
    let checks: Vec<Check> = serde_json::from_value(view["checks"].clone()).unwrap();
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].result, CheckResult::Pass);
    let log: Vec<Event> = ws.json(&["log", &id]);
    assert!(
        log.iter()
            .any(|event| event.kind == EventKind::Done && event.actor == Actor::Agent)
    );

    let prerequisite = ws.new_task("unmet dependency", &[]).task.unwrap().id;
    let dependent = ws.new_task("force a dependency", &["--depends-on", &prerequisite]);
    let dependent_id = dependent.task.unwrap().id;
    let refused = ws.cli(&["dispatch", &dependent_id]);
    assert_eq!(refused.status.code(), Some(2));
    let stderr = String::from_utf8(refused.stderr).unwrap();
    assert!(stderr.starts_with("refused: "));
    let next = stderr
        .lines()
        .find_map(|line| line.strip_prefix("next: "))
        .unwrap();
    assert!(next.ends_with(" --force"), "{next}");
    let argv = shlex::split(next).unwrap();
    assert_eq!(argv[0], BIN);
    success(
        ws.command(&argv[0], &ws.caller)
            .args(&argv[1..])
            .output()
            .unwrap(),
    );
    let ready = ws.wait_ready(&dependent_id, dependent.seq);
    assert_eq!(
        ws.cli(&[
            "wait",
            "--task",
            &dependent_id,
            "--after",
            &ready.seq.to_string(),
            "--timeout",
            "0s"
        ])
        .status
        .code(),
        Some(124)
    );
    ws.ok(&["approve", &dependent_id]);
}
