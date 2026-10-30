//! scratch-worktrees from assistant shells and the owner's CLI (T-544 C8, C13, C10 promote).
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use jiff::{SignedDuration, Timestamp};

fn git(dir: &Path, args: &[&str], date: Option<&str>) -> String {
    let mut cmd = Command::new("git");
    if let Some(date) = date {
        cmd.env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date);
    }
    let out = cmd
        .args([
            "-c",
            "user.name=Scratch Test",
            "-c",
            "user.email=scratch@test",
        ])
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl Fixture {
    /// The owner's workspace is the git repository of project `corral`, with sessions A-1, A-2.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("ws")).unwrap();
        let root = dir.path().join("ws").canonicalize().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        git(&root, &["init", "-q", "-b", "main"], None);
        std::fs::write(root.join("README.md"), "base\n").unwrap();
        std::fs::write(root.join(".gitignore"), ".corral/\n.worktrees/\n").unwrap();
        git(&root, &["add", "."], None);
        git(&root, &["commit", "-q", "-m", "base"], None);
        let fx = Self {
            _dir: dir,
            root,
            home,
        };
        fx.ok(None, &["init"]);
        let config = fx.root.join(".corral/corral.toml");
        let mut text = std::fs::read_to_string(&config).unwrap();
        text.push_str("\n[projects.corral]\npath = '.'\n");
        std::fs::write(&config, text).unwrap();
        fx.ok(None, &["config", "set", "resources.min_disk_free", "'1B'"]);
        fx.ok(None, &["assistant", "create"]);
        fx.ok(None, &["assistant", "create"]);
        fx
    }

    fn run(&self, assistant: Option<&str>, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_corral"));
        for name in [
            "CORRAL_ASSISTANT",
            "CORRAL_PM",
            "AI_AGENT",
            "PI_CODING_AGENT",
            "HERDR_PANE_ID",
            "HERDR_WORKSPACE_ID",
            "HERDR_TAB_ID",
            "GIT_DIR",
            "GIT_INDEX_FILE",
        ] {
            cmd.env_remove(name);
        }
        if let Some(session) = assistant {
            cmd.env("CORRAL_ASSISTANT", session);
        }
        cmd.env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .env("HERDR_BIN_PATH", "/nonexistent/herdr")
            .arg("--root")
            .arg(&self.root)
            .args(args)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn ok(&self, assistant: Option<&str>, args: &[&str]) -> String {
        let out = self.run(assistant, args);
        assert!(
            out.status.success(),
            "{args:?}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn json(&self, assistant: Option<&str>, args: &[&str]) -> serde_json::Value {
        serde_json::from_str(&self.ok(assistant, args)).unwrap()
    }

    fn event(&self, kind: &str) -> serde_json::Value {
        std::fs::read_to_string(self.root.join(".corral/events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .rfind(|event| event["kind"] == kind)
            .unwrap_or_else(|| panic!("no {kind} event"))
    }

    fn branches(&self) -> String {
        git(&self.root, &["branch", "--list"], None)
    }

    /// Session records, artifacts and scratch checkouts: what a scratch command may change.
    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.insert(path.clone(), std::fs::read(&path).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        for dir in [".corral/assistants", ".corral/artifacts", ".worktrees"] {
            walk(&self.root.join(dir), &mut out);
        }
        out
    }
}

#[test]
fn assistant_shell_creates_promotes_and_discards_only_its_own_scratch() {
    // Given scratch spike-cache of A-1 with a commit, and scratch other of A-2.
    let fx = Fixture::new();
    let made = fx.json(
        Some("A-1"),
        &[
            "assistant",
            "scratch",
            "new",
            "-p",
            "corral",
            "--slug",
            "spike-cache",
            "--json",
        ],
    );
    let path = PathBuf::from(made["path"].as_str().unwrap());
    assert_eq!(path, fx.root.join(".worktrees/A-1-spike-cache"));
    assert_eq!(made["branch"], "corral/A-1/spike-cache");
    let created = fx.event("assistant_scratch_created");
    assert_eq!(created["via"], "assistant:A-1");
    fx.ok(
        Some("A-2"),
        &[
            "assistant",
            "scratch",
            "new",
            "-p",
            "corral",
            "--slug",
            "other",
        ],
    );
    std::fs::write(path.join("cache.rs"), "fn cache() {}\n").unwrap();
    git(&path, &["add", "cache.rs"], None);
    git(&path, &["commit", "-q", "-m", "cache spike"], None);
    // Then the owner's storage report shows its bytes.
    let storage = fx.json(None, &["stats", "--storage", "--json"]);
    let row = storage["scratch"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["slug"] == "spike-cache")
        .unwrap();
    assert!(row["bytes"].as_u64().unwrap() > 0, "{row}");
    assert!(
        fx.ok(None, &["stats", "--storage"])
            .contains("A-1/spike-cache")
    );
    // And an assistant lists only its own scratch.
    let own = fx.json(Some("A-1"), &["assistant", "scratch", "list", "--json"]);
    assert_eq!(own.as_array().unwrap().len(), 1);
    // When A-1 aims at A-2's scratch, then it refuses not_own_scratch and nothing changes.
    let before = (fx.snapshot(), fx.branches());
    let out = fx.run(
        Some("A-1"),
        &[
            "assistant",
            "scratch",
            "discard",
            "other",
            "--session",
            "A-2",
        ],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not_own_scratch"));
    assert_eq!((fx.snapshot(), fx.branches()), before);
    // When A-1 promotes its spike, then a Backlog task carries the diff; the scratch stays.
    let head = git(&path, &["rev-parse", "HEAD"], None);
    let promoted = fx.json(
        Some("A-1"),
        &[
            "assistant",
            "scratch",
            "promote",
            "spike-cache",
            "--title",
            "Cache layer",
            "--json",
        ],
    );
    let task = fx.json(
        None,
        &["show", promoted["task"].as_str().unwrap(), "--json"],
    );
    assert_eq!(task["status"], "backlog");
    assert_eq!(task["artifacts"][0]["kind"], "diff");
    assert_eq!(fx.event("assistant_scratch_promoted")["actor"], "assistant");
    assert_eq!(git(&path, &["rev-parse", "HEAD"], None), head);
    // When A-1 discards its own scratch (stdin closed: no confirmation can be read).
    let removed = fx.json(
        Some("A-1"),
        &["assistant", "scratch", "discard", "spike-cache", "--json"],
    );
    // Then its work was saved on A-1 first and the worktree and branch are gone.
    let artifact = removed["artifact"].as_str().unwrap();
    assert!(
        artifact.starts_with("artifacts/assistant/A-1/"),
        "{removed}"
    );
    let saved = fx.ok(
        None,
        &["artifact", "show", "A-1:A-1-spike-cache.diff", "--raw"],
    );
    assert!(saved.contains("+fn cache() {}"), "{saved}");
    let event = fx.event("assistant_scratch_discarded");
    assert_eq!(event["via"], "assistant:A-1");
    assert!(!path.exists() && !fx.branches().contains("corral/A-1/spike-cache"));
    assert!(fx.root.join(".worktrees/A-2-other").is_dir());
}

#[test]
fn owner_gc_collects_an_ended_sessions_idle_scratch_after_a_dry_run() {
    // Given A-1's scratch whose last commit and session end are ten days old.
    let fx = Fixture::new();
    fx.ok(
        Some("A-1"),
        &[
            "assistant",
            "scratch",
            "new",
            "-p",
            "corral",
            "--slug",
            "spike-cache",
        ],
    );
    let path = fx.root.join(".worktrees/A-1-spike-cache");
    let old = Timestamp::now()
        .checked_sub(SignedDuration::from_hours(10 * 24))
        .unwrap();
    std::fs::write(path.join("cache.rs"), "fn cache() {}\n").unwrap();
    git(&path, &["add", "cache.rs"], None);
    let date = format!("{} +0000", old.as_second());
    git(&path, &["commit", "-q", "-m", "cache spike"], Some(&date));
    fx.ok(None, &["assistant", "end", "A-1"]);
    corral_core::Store::open(&fx.root)
        .unwrap()
        .update_assistant("A-1", |s| {
            s.ended = Some(old);
            Ok(())
        })
        .unwrap();
    // GC needs an explicit mode and is never an assistant operation.
    assert!(
        !fx.run(None, &["assistant", "scratch", "gc"])
            .status
            .success()
    );
    let refused = fx.run(Some("A-2"), &["assistant", "scratch", "gc", "--yes"]);
    assert_eq!(refused.status.code(), Some(2));
    // When the owner previews, then it is listed and nothing changes.
    let before = (fx.snapshot(), fx.branches());
    let dry = fx.json(None, &["assistant", "scratch", "gc", "--dry-run", "--json"]);
    assert_eq!(dry["items"][0]["action"], "remove", "{dry}");
    assert_eq!(dry["items"][0]["preserve"], true);
    assert_eq!((fx.snapshot(), fx.branches()), before);
    // When the owner collects, then the diff is saved on A-1 and the worktree and branch go.
    let real = fx.json(None, &["assistant", "scratch", "gc", "--yes", "--json"]);
    assert_eq!(real["items"][0]["action"], "removed", "{real}");
    let saved = fx.ok(
        None,
        &["artifact", "show", "A-1:A-1-spike-cache.diff", "--raw"],
    );
    assert!(saved.contains("+fn cache() {}"));
    assert!(!path.exists() && !fx.branches().contains("corral/A-1/spike-cache"));
    assert_eq!(fx.event("assistant_scratch_collected")["actor"], "owner");
    // And doctor reports a scratch directory no session records.
    std::fs::create_dir_all(fx.root.join(".worktrees/A-7-lost")).unwrap();
    let doctor: serde_json::Value =
        serde_json::from_slice(&fx.run(None, &["doctor", "--json"]).stdout).unwrap();
    let check = doctor["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "assistant_scratch")
        .unwrap();
    assert_eq!(check["ok"], false);
    assert!(check["message"].as_str().unwrap().contains("A-7-lost"));
}
