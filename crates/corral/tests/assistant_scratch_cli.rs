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
        self.command(assistant, args).output().unwrap()
    }

    fn command(&self, assistant: Option<&str>, args: &[&str]) -> Command {
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
            .stdin(Stdio::null());
        cmd
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

    /// Fetches HEAD from a saved bundle read back through `artifact show --raw`, then shows a file.
    fn fetched(&self, artifact: &str, file: &str) -> String {
        let out = self.run(None, &["artifact", "show", artifact, "--raw"]);
        assert!(out.status.success(), "{out:?}");
        let bundle = self.home.join("fetched.bundle");
        std::fs::write(&bundle, &out.stdout).unwrap();
        let source = bundle.to_str().unwrap();
        git(
            &self.root,
            &["fetch", "-q", source, "+HEAD:refs/heads/fetched"],
            None,
        );
        git(&self.root, &["show", &format!("fetched:{file}")], None)
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
    let artifact = removed["artifacts"][0].as_str().unwrap();
    assert!(
        artifact.starts_with("artifacts/assistant/A-1/"),
        "{removed}"
    );
    let manifest = fx.ok(
        None,
        &[
            "artifact",
            "show",
            "A-1:A-1-spike-cache.restore.txt",
            "--raw",
        ],
    );
    assert!(
        manifest.contains("commits      in A-1-spike-cache.commits.bundle"),
        "{manifest}"
    );
    assert_eq!(
        fx.fetched("A-1:A-1-spike-cache.commits.bundle", "cache.rs"),
        "fn cache() {}"
    );
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
    let manifest = fx.ok(
        None,
        &[
            "artifact",
            "show",
            "A-1:A-1-spike-cache.restore.txt",
            "--raw",
        ],
    );
    assert!(
        manifest.contains("commits      in A-1-spike-cache.commits.bundle"),
        "{manifest}"
    );
    assert_eq!(
        fx.fetched("A-1:A-1-spike-cache.commits.bundle", "cache.rs"),
        "fn cache() {}"
    );
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

#[test]
fn owner_gc_keeps_a_scratch_its_own_calling_shell_still_uses() {
    // Given an ended session's idle scratch, and a shell whose cwd is inside it.
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
            "resident",
        ],
    );
    let path = fx.root.join(".worktrees/A-1-resident");
    fx.ok(None, &["config", "set", "assistant.scratch_idle_days", "0"]);
    fx.ok(None, &["assistant", "end", "A-1"]);
    // When that shell runs owner GC from a subshell that has left the scratch, so only an
    // ancestor of the GC process remains there.
    let script = r#"(cd "$WS" && exec "$CORRAL" --root "$WS" assistant scratch gc --yes --json)"#;
    let mut cmd = Command::new("sh");
    for name in [
        "CORRAL_ASSISTANT",
        "CORRAL_PM",
        "AI_AGENT",
        "PI_CODING_AGENT",
    ] {
        cmd.env_remove(name);
    }
    let out = cmd
        .args(["-c", script])
        .env("WS", &fx.root)
        .env("CORRAL", env!("CARGO_BIN_EXE_corral"))
        .env("HOME", &fx.home)
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .current_dir(&path)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    // Then GC keeps it because a live process (the calling shell) uses it.
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["items"][0]["action"], "skip", "{report}");
    let reason = report["items"][0]["reason"].as_str().unwrap();
    assert!(reason.starts_with("live process cwd"), "{report}");
    assert!(path.is_dir() && fx.branches().contains("corral/A-1/resident"));
    // And once no process uses it, the same GC removes it.
    let later = fx.json(None, &["assistant", "scratch", "gc", "--yes", "--json"]);
    assert_eq!(later["items"][0]["action"], "removed", "{later}");
    assert!(!path.exists());
}

#[cfg(target_os = "linux")]
mod race {
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc::Receiver;
    use std::time::{Duration, Instant};

    use super::Fixture;

    /// Kills a spawned process group, wrappers included, if the test ends early.
    struct Group(Child);

    impl Drop for Group {
        fn drop(&mut self) {
            // Already-exited groups make kill fail; nothing is left to clean up then.
            let _ = Command::new("kill")
                .args(["-KILL", "--", &format!("-{}", self.0.id())])
                .stderr(Stdio::null())
                .status();
            let _ = self.0.wait();
        }
    }

    /// A child's stderr, read on a thread and awaited with bounded deadlines, without polling.
    struct Stream {
        rx: Receiver<Vec<u8>>,
        seen: Vec<u8>,
    }

    impl Stream {
        fn of(child: &mut Child) -> Self {
            use std::io::Read;
            let mut stderr = child.stderr.take().unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut chunk = [0; 4096];
                while let Ok(n) = stderr.read(&mut chunk) {
                    if n == 0 || tx.send(chunk[..n].to_vec()).is_err() {
                        break;
                    }
                }
            });
            Self {
                rx,
                seen: Vec::new(),
            }
        }

        fn text(&self) -> String {
            String::from_utf8_lossy(&self.seen).into_owned()
        }

        fn until(&mut self, reached: impl Fn(&str) -> bool) -> String {
            let deadline = Instant::now() + Duration::from_secs(30);
            while !reached(&self.text()) {
                let remaining = deadline
                    .checked_duration_since(Instant::now())
                    .unwrap_or_else(|| panic!("never matched: {}", self.text()));
                self.seen.extend(self.rx.recv_timeout(remaining).unwrap());
            }
            self.text()
        }

        /// Everything until the writer closes stderr (the process and its children exited).
        fn rest(mut self) -> String {
            let deadline = Instant::now() + Duration::from_secs(60);
            loop {
                let remaining = deadline
                    .checked_duration_since(Instant::now())
                    .unwrap_or_else(|| panic!("never exited: {}", self.text()));
                match self.rx.recv_timeout(remaining) {
                    Ok(chunk) => self.seen.extend(chunk),
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return self.text(),
                    Err(error) => panic!("{error}: {}", self.text()),
                }
            }
        }
    }

    fn spawn(mut cmd: Command) -> (Group, Stream) {
        let mut child = cmd
            .process_group(0)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stream = Stream::of(&mut child);
        (Group(child), stream)
    }

    #[test]
    fn concurrent_creators_never_exceed_the_last_scratch_slot() {
        // Given one free slot, and a git wrapper that parks each corral process's first branch
        // probe for slug `race` until the test releases it.
        let fx = Fixture::new();
        fx.ok(None, &["config", "set", "assistant.scratch_max", "1"]);
        let barrier = fx.home.join("barrier");
        std::fs::create_dir_all(barrier.join("bin")).unwrap();
        let release = barrier.join("release");
        assert!(
            Command::new("mkfifo")
                .arg(&release)
                .status()
                .unwrap()
                .success()
        );
        let found = Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap();
        let real = String::from_utf8(found.stdout).unwrap().trim().to_owned();
        let wrapper = barrier.join("bin/git");
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nfor last in \"$@\"; do :; done\ncase \"$last\" in\n  refs/heads/corral/A-*/race)\n    if mkdir \"{dir}/held.$PPID\" 2>/dev/null; then\n      echo \"barrier-arrived $PPID\" >&2\n      read _ < \"{dir}/release\"\n    fi ;;\nesac\nexec \"{real}\" \"$@\"\n",
                dir = barrier.display()
            ),
        )
        .unwrap();
        assert!(
            Command::new("chmod")
                .arg("+x")
                .arg(&wrapper)
                .status()
                .unwrap()
                .success()
        );
        let path = format!(
            "{}:{}",
            barrier.join("bin").display(),
            std::env::var("PATH").unwrap()
        );
        let create = |session: &str| {
            let mut cmd = fx.command(
                Some(session),
                &[
                    "assistant",
                    "scratch",
                    "new",
                    "-p",
                    "corral",
                    "--slug",
                    "race",
                ],
            );
            cmd.env("PATH", &path);
            cmd
        };
        // When A-1 is parked inside its creation, holding the last slot,
        let (mut first, mut first_err) = spawn(create("A-1"));
        first_err.until(|text| text.contains("barrier-arrived"));
        // and A-2 starts its own creation under strace,
        let plain = create("A-2");
        let mut traced = Command::new("strace");
        traced
            .args(["-yy", "-e", "trace=flock", "--"])
            .arg(plain.get_program())
            .args(plain.get_args())
            .current_dir(&fx.root)
            .stdin(Stdio::null());
        for (key, value) in plain.get_envs() {
            match value {
                Some(value) => traced.env(key, value),
                None => traced.env_remove(key),
            };
        }
        let (mut second, mut second_err) = spawn(traced);
        // then A-2 blocks on the workspace creation lock instead of reaching its branch probe.
        let seen = second_err.until(|text| {
            text.contains("scratch-create.lock>, LOCK_EX") || text.contains("barrier-arrived")
        });
        assert!(
            !seen.contains("barrier-arrived"),
            "A-2 passed the cap check: {seen}"
        );
        // When A-1 is released, it completes, and A-2 then refuses the full workspace.
        std::fs::write(&release, "go\n").unwrap();
        let first_text = first_err.rest();
        assert!(first.0.wait().unwrap().success(), "{first_text}");
        let second_text = second_err.rest();
        assert!(!second.0.wait().unwrap().success(), "{second_text}");
        assert!(second_text.contains("scratch_cap"), "{second_text}");
        let listed = fx.json(None, &["assistant", "scratch", "list", "--json"]);
        assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
        assert_eq!(listed[0]["session"], "A-1");
        assert!(!fx.root.join(".worktrees/A-2-race").exists());
    }
}
