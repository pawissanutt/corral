//! One isolated owner journey; service-level anchor/concurrency/cap variants live in ops.
use std::{
    path::Path,
    process::{Command, Output, Stdio},
};

use corral_core::{
    Run, Status, Store,
    backend::{TaskNum, TaskParts},
};
use corral_ops::annotations::{self, AnnotationsQuery};
use serde_json::{Value, json};

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "core.fsmonitor")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_1", "false")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}

struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
    base: String,
    head: String,
    worktree: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-b", "main"]);
        std::fs::write(dir.path().join("code.txt"), "old\n").unwrap();
        git(dir.path(), &["add", "code.txt"]);
        git(dir.path(), &["commit", "-m", "base"]);
        let base = git(dir.path(), &["rev-parse", "HEAD"]);
        git(
            dir.path(),
            &["worktree", "add", "-b", "corral/T-001", ".worktrees/T-001"],
        );
        let worktree = dir.path().join(".worktrees/T-001");
        std::fs::write(worktree.join("code.txt"), "one\ntwo\n").unwrap();
        git(&worktree, &["commit", "-am", "candidate"]);
        let head = git(&worktree, &["rev-parse", "HEAD"]);
        let store = Store::init(dir.path()).unwrap();
        store
            .create("Annotated", None, Status::Review, "Spec")
            .unwrap();
        store
            .update("1", |task| {
                let run: Run = serde_json::from_value(json!({
                    "id":"R1", "worktree":worktree, "branch":"corral/T-001", "base":"main",
                    "base_sha":base, "agent":"fixture", "started":"2026-01-01T00:00:00Z"
                }))
                .unwrap();
                task.runs.push(run);
                Ok(())
            })
            .unwrap();
        Self {
            dir,
            store,
            base,
            head,
            worktree,
        }
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_corral"));
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy();
            if name.starts_with("CORRAL_")
                || name.starts_with("HERDR_")
                || name == "AI_AGENT"
                || name == "PI_CODING_AGENT"
            {
                cmd.env_remove(key);
            }
        }
        cmd.current_dir(self.dir.path())
            .arg("--root")
            .arg(self.dir.path())
            .arg("annotation")
            .args(args)
            .arg("--json")
            .env("HERDR_BIN_PATH", self.dir.path().join("no-herdr"))
            .stdin(Stdio::null());
        cmd
    }
    fn ok(&self, args: &[&str]) -> Value {
        let out = self.command(args).output().unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.stdout.len() <= 32 * 1024 + 1);
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn open(&self) -> Vec<&str> {
        vec![
            "open",
            "1",
            "--run",
            "R1",
            "--base",
            &self.base,
            "--head",
            &self.head,
            "--path",
            "code.txt",
            "--lines",
            "1-2",
            "--text",
            "Check the bound",
        ]
    }
    fn snapshot(&self) -> Value {
        let view = self.store.view().unwrap();
        json!({"seq":view.seq(),"task":view.task(TaskNum(1), &TaskParts::All).unwrap(),
            "events":self.store.events().unwrap()})
    }
    fn refused(&self, command: &mut Command, code: &str) -> Value {
        let before = self.snapshot();
        let out: Output = command.output().unwrap();
        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
        let value: Value = serde_json::from_slice(&out.stderr)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
        assert_eq!(value["error"]["code"], code, "{value}");
        assert!(value["error"]["message"].is_string());
        assert!(value["error"].get("detail").is_some());
        assert_eq!(before, self.snapshot(), "refusal must write nothing");
        value
    }
}

#[test]
fn owner_annotation_cli_journey_and_authority_boundary() {
    let f = Fixture::new();
    let opened = f.ok(&f.open());
    assert_eq!(
        opened,
        json!({"task":"T-001","thread":"N1","entry":1,"revision":1,
        "state":"open","replayed":false})
    );
    let before = f.snapshot();
    let mut replay = f.ok(&f.open());
    assert_eq!(replay["replayed"], true);
    replay["replayed"] = json!(false);
    assert_eq!(replay, opened);
    assert_eq!(before, f.snapshot());

    // Authorization precedes replay; all four mutation adapters refuse without writes.
    for (marker, value) in [
        ("AI_AGENT", "1"),
        ("PI_CODING_AGENT", "1"),
        ("CORRAL_PM", "1"),
        ("CORRAL_ASSISTANT", "A-1"),
    ] {
        for args in [
            f.open(),
            vec!["reply", "1", "N1", "--revision", "1", "--text", "reply"],
            vec!["resolve", "1", "N1", "--revision", "1"],
            vec!["reopen", "1", "N1", "--revision", "1"],
        ] {
            f.refused(f.command(&args).env(marker, value), "not_owner");
        }
        // Generic agent and PM shells retain reads. Assistant reads additionally require its
        // ordinary authenticated session; classification is covered by the cheap unit table.
        if marker != "CORRAL_ASSISTANT" {
            for args in [vec!["list", "1"], vec!["show", "1", "N1"]] {
                let out = f.command(&args).env(marker, value).output().unwrap();
                assert!(
                    out.status.success(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            assert_eq!(before, f.snapshot());
        }
    }
    for args in [
        f.open(),
        vec!["reply", "1", "N1", "--revision", "1", "--text", "reply"],
        vec!["resolve", "1", "N1", "--revision", "1"],
        vec!["reopen", "1", "N1", "--revision", "1"],
    ] {
        f.refused(f.command(&args).current_dir(&f.worktree), "not_owner");
    }
    for args in [vec!["list", "1"], vec!["show", "1", "N1"]] {
        assert!(
            f.command(&args)
                .current_dir(&f.worktree)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    assert_eq!(before, f.snapshot());
    // The required observed run is forwarded, never inferred from matching base/head.
    let mut stale = f.open();
    stale[3] = "R0";
    let refusal = f.refused(&mut f.command(&stale), "stale_candidate");
    assert_eq!(refusal["error"]["detail"]["run"], "R1");
    let mut invalid = f.open();
    invalid[11] = "0-2";
    f.refused(&mut f.command(&invalid), "invalid_range");
    f.refused(&mut f.command(&["reply", "1", "N1"]), "bad_request");

    let list = f.ok(&["list", "1"]);
    assert_eq!(
        list,
        serde_json::to_value(
            annotations::list(&f.store, "1", &AnnotationsQuery::default()).unwrap()
        )
        .unwrap()
    );
    assert_eq!(list["version"], 1);
    assert_eq!(list["candidate"]["candidate"]["run"], "R1");
    assert_eq!(list["items"][0]["anchor_status"]["status"], "current");
    assert!(list["items"][0].get("entries").is_none());
    let show = f.ok(&["show", "1", "N1"]);
    assert_eq!(
        show["entries"][0]["author"],
        json!({"kind":"owner","version":1,"surface":{"kind":"cli"}})
    );
    let key = show["entries"][0]["request"]["key"].as_str().unwrap();
    assert!(key.starts_with("cli-") && key.len() == 36);
    let mut conflict = f.open();
    *conflict.last_mut().unwrap() = "Changed text";
    conflict.extend(["--request-key", key]);
    f.refused(&mut f.command(&conflict), "request_key_conflict");
    assert_eq!(before, f.snapshot(), "reads must write nothing");

    let note_file = f.dir.path().join("note.txt");
    std::fs::write(&note_file, "file reply\n").unwrap();
    let reply = f.ok(&[
        "reply",
        "1",
        "N1",
        "--revision",
        "1",
        "--text-file",
        note_file.to_str().unwrap(),
    ]);
    assert_eq!(reply["revision"], 2);
    assert_eq!(
        f.ok(&["resolve", "1", "N1", "--revision", "2", "--text", "fixed"])["state"],
        "resolved"
    );
    let resolved = f.ok(&[
        "list", "1", "--state", "resolved", "--anchor", "current", "--limit", "1",
    ]);
    assert_eq!(resolved["items"][0]["revision"], 3);
    let conflict = f.refused(
        &mut f.command(&["reply", "1", "N1", "--revision", "2", "--text", "racing"]),
        "thread_revision_conflict",
    );
    assert_eq!(
        conflict["error"]["detail"],
        json!({"revision":3,"state":"resolved"})
    );
    let reopened = f.ok(&[
        "reopen",
        "1",
        "N1",
        "--revision",
        "3",
        "--request-key",
        "reopen-note",
    ]);
    assert_eq!(reopened["revision"], 4);
    assert_eq!(reopened["state"], "open");
    let before = f.snapshot();
    let mut page = f.ok(&["show", "1", "N1", "--limit", "1"]);
    let mut entries = vec![];
    loop {
        entries.extend(page["entries"].as_array().unwrap().clone());
        let Some(cursor) = page["next_cursor"].as_str() else {
            break;
        };
        page = f.ok(&["show", "1", "N1", "--limit", "1", "--cursor", cursor]);
    }
    assert_eq!(
        entries
            .iter()
            .map(|e| e["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["opened", "reply", "resolved", "reopened"]
    );
    let expected = annotations::show(&f.store, "1", "N1", &AnnotationsQuery::default()).unwrap();
    assert_eq!(
        f.ok(&["show", "1", "N1"]),
        serde_json::to_value(expected).unwrap()
    );
    assert_eq!(before, f.snapshot());
    let events: Vec<_> = f
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.kind.starts_with("annotation_"))
        .map(|e| e.kind)
        .collect();
    assert_eq!(
        events,
        [
            "annotation_opened",
            "annotation_replied",
            "annotation_resolved",
            "annotation_reopened"
        ]
    );
}
