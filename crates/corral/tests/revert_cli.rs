use std::path::Path;
use std::process::{Command, Output, Stdio};

use corral_core::{Status, Store};
use serde_json::json;

fn isolated(command: &mut Command) -> &mut Command {
    for (key, _) in std::env::vars_os() {
        let key_text = key.to_string_lossy();
        if key_text.starts_with("CORRAL_")
            || key_text.starts_with("HERDR_")
            || key_text.starts_with("GIT_")
        {
            command.env_remove(key);
        }
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::null())
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = isolated(&mut Command::new("git"))
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

struct Fixture {
    dir: tempfile::TempDir,
    repo: std::path::PathBuf,
    store: Store,
    task: String,
    original: String,
}

impl Fixture {
    fn new(landed: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.name", "Corral fixture"]);
        git(&repo, &["config", "user.email", "fixture@example.invalid"]);
        std::fs::write(repo.join("task.txt"), "before\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "initial"]);
        let base = git(&repo, &["rev-parse", "HEAD"]);
        std::fs::write(repo.join("task.txt"), "landed\n").unwrap();
        git(&repo, &["commit", "-qam", "task change"]);
        let original = git(&repo, &["rev-parse", "HEAD"]);
        let store = Store::init(dir.path()).unwrap();
        let task = store
            .create(
                "revert fixture",
                Some("app"),
                if landed { Status::Done } else { Status::Review },
                "",
            )
            .unwrap();
        store
            .update(&task.id, |task| {
                task.runs.push(serde_json::from_value(json!({
                    "id":"R1","agent":"worker","worktree":repo.join(".worktrees/T-001"),
                    "branch":"corral/T-001","base":"main","base_sha":base,
                    "started":"2026-01-01T00:00:00Z","ended":"2026-01-01T00:05:00Z",
                    "outcome":"done"
                }))?);
                Ok(())
            })
            .unwrap();
        if landed {
            store
                .log_event(
                    Some(&task.id),
                    "approved",
                    json!({
                        "run":"R1","strategy":"fast_forward","sha":original,"previous_sha":base,
                        "base":"main","project":"app"
                    }),
                )
                .unwrap();
        }
        Self {
            dir,
            repo,
            store,
            task: task.id,
            original,
        }
    }

    fn revert(&self) -> Output {
        isolated(&mut Command::new(env!("CARGO_BIN_EXE_corral")))
            .env("CORRAL_ROOT", self.dir.path())
            .env("HERDR_BIN_PATH", self.dir.path().join("no-herdr"))
            .args(["revert", &self.task])
            .output()
            .unwrap()
    }
}

#[test]
fn revert_cli_appends_history_reopens_task_and_records_sha() {
    // Given a landed task followed by an unrelated change on base.
    let fx = Fixture::new(true);
    std::fs::write(fx.repo.join("peer.txt"), "keep\n").unwrap();
    git(&fx.repo, &["add", "."]);
    git(&fx.repo, &["commit", "-qm", "unrelated later work"]);
    let before = git(&fx.repo, &["rev-parse", "HEAD"]);
    // When invoking the real CLI with no remote configured.
    let result = fx.revert();
    // Then an appended revert preserves later work and links its SHA from the event.
    assert!(result.status.success(), "{result:?}");
    let head = git(&fx.repo, &["rev-parse", "HEAD"]);
    assert_ne!(head, before);
    assert_eq!(git(&fx.repo, &["rev-parse", "HEAD^"]), before);
    assert_eq!(
        std::fs::read_to_string(fx.repo.join("task.txt")).unwrap(),
        "before\n"
    );
    assert_eq!(
        std::fs::read_to_string(fx.repo.join("peer.txt")).unwrap(),
        "keep\n"
    );
    let task = fx.store.get(&fx.task).unwrap();
    assert_eq!(task.status, Status::Backlog);
    assert!(task.dispatch_hold);
    let event = fx
        .store
        .events()
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "reverted")
        .unwrap();
    assert_eq!(event.detail["sha"], head);
    assert_eq!(event.detail["original_shas"], json!([fx.original]));
    assert!(git(&fx.repo, &["status", "--porcelain"]).is_empty());
}

#[test]
fn revert_cli_conflict_reports_later_commit_and_changes_nothing() {
    // Given a later commit that modifies the landed task's line.
    let fx = Fixture::new(true);
    std::fs::write(fx.repo.join("task.txt"), "later conflicting work\n").unwrap();
    git(&fx.repo, &["commit", "-qam", "conflicting later work"]);
    let before = git(&fx.repo, &["rev-parse", "HEAD"]);
    // When reverting through the CLI.
    let result = fx.revert();
    // Then the conflicting later SHA is reported and base/task remain unchanged.
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains(&before));
    assert_eq!(git(&fx.repo, &["rev-parse", "HEAD"]), before);
    assert!(git(&fx.repo, &["status", "--porcelain"]).is_empty());
    assert_eq!(fx.store.get(&fx.task).unwrap().status, Status::Done);
    assert!(
        !fx.store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.kind == "reverted")
    );
}

#[test]
fn revert_cli_refuses_an_unlanded_task() {
    // Given an unlanded run with no approval event.
    let fx = Fixture::new(false);
    let before = git(&fx.repo, &["rev-parse", "HEAD"]);
    // When reverting through the CLI.
    let result = fx.revert();
    // Then both base and task retain their existing state.
    assert!(!result.status.success());
    assert_eq!(git(&fx.repo, &["rev-parse", "HEAD"]), before);
    assert_eq!(fx.store.get(&fx.task).unwrap().status, Status::Review);
}
