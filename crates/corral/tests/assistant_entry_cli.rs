//! assistant-entry-guard (T-544 C2): every command an assistant shell can name has one class.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Read,
    File,
    Refused,
}
use Class::{File, Read, Refused};

const ROUTE: &str = "needs the owner; use corral_owner_action or corral pm inbox post";

/// Commands that clap hides from `--help`, so the help walk cannot list them.
const HIDDEN: [&str; 1] = ["runtime-probe"];

/// One row per command path, plus one per argument that changes the class.
const ROWS: &[(&str, &[&str], Class)] = &[
    ("", &[], Refused),
    ("assistant create", &[], Refused),
    ("assistant list", &[], Read),
    ("assistant show", &["A-1"], Read),
    ("assistant end", &["A-1"], Refused),
    ("assistant summary", &["A-1", "--file", "summary.md"], File),
    (
        "assistant summary",
        &["A-2", "--file", "summary.md"],
        Refused,
    ),
    ("assistant scratch new", &["-p", "corral"], File),
    ("assistant scratch list", &[], Read),
    ("assistant scratch discard", &["spike"], File),
    (
        "assistant scratch promote",
        &["spike", "--title", "Spike"],
        File,
    ),
    ("assistant scratch gc", &["--dry-run"], Refused),
    ("assistant scratch gc", &["--yes"], Refused),
    ("mode status", &[], Read),
    ("mode assist", &[], Refused),
    ("mode autopilot", &[], Refused),
    ("mode pause", &[], Refused),
    ("mode resume", &[], Refused),
    ("mode turbo", &[], Refused),
    ("inbox", &[], Read),
    ("inbox show", &["pm#1"], Read),
    ("inbox flush", &["pm"], Refused),
    ("inbox retry", &["pm#1"], Refused),
    ("inbox inspect-retry", &["pm#1"], Refused),
    ("inbox index-owner", &["pm"], Refused),
    ("inbox recover", &["pm", "--discard"], Refused),
    (
        "inbox mark",
        &["pm#1", "--observed", "--reason", "seen"],
        Refused,
    ),
    ("attention", &[], Read),
    ("attention list", &[], Read),
    ("attention attend", &["T-001", "--note", "n"], Refused),
    ("attention park", &["T-001", "--reason", "r"], Refused),
    (
        "attention unpark",
        &["T-001", "--expected-revision", "1"],
        Refused,
    ),
    ("attention stats", &[], Read),
    ("agent call", &["--op", "hello"], File),
    ("agent launch", &["--bootstrap", "boot.json"], Refused),
    ("agent install", &[], Refused),
    ("agent login", &[], Refused),
    ("agent doctor", &[], Read),
    ("agent state", &["T-001"], Read),
    ("agent cancel", &["T-001"], Refused),
    ("agent resume", &["T-001"], Refused),
    ("agent rewind", &["T-001", "--list"], Read),
    ("agent rewind", &["T-001"], Refused),
    ("pi", &[], Refused),
    ("question list", &[], Read),
    ("question show", &["q-1"], Read),
    (
        "question answer",
        &["q-1", "--rev", "1", "--text", "x"],
        Refused,
    ),
    ("question escalate", &["q-1", "--rev", "1"], Refused),
    (
        "question cancel",
        &["q-1", "--rev", "1", "--reason", "r"],
        Refused,
    ),
    (
        "question deadline",
        &[
            "q-1",
            "--rev",
            "1",
            "--at",
            "2026-11-01T00:00:00Z",
            "--instruction",
            "i",
        ],
        Refused,
    ),
    ("automation preview", &[], Read),
    ("automation confirm", &["--digest", "d"], Refused),
    ("automation status", &[], Read),
    (
        "selection catalog",
        &["T-001", "--role", "implementation"],
        Read,
    ),
    ("selection save", &["--file", "s.json"], Refused),
    ("setup procedure", &[], Read),
    ("setup template", &[], Read),
    ("setup bootstrap", &["--harness", "pi"], Refused),
    ("setup status", &[], Read),
    ("setup catalog", &[], Read),
    ("setup discover", &[], Read),
    ("setup begin", &["--expected-revision", "0"], Refused),
    (
        "setup answers",
        &["--file", "a.json", "--expected-revision", "0"],
        Refused,
    ),
    ("setup validate", &["--file", "v.json"], Read),
    (
        "setup propose",
        &["--file", "p.json", "--expected-revision", "0"],
        Refused,
    ),
    (
        "setup approve",
        &["--file", "p.json", "--expected-revision", "0"],
        Refused,
    ),
    ("setup diff", &[], Read),
    ("setup apply", &[], Refused),
    ("setup cancel", &["--expected-revision", "0"], Refused),
    ("digest", &[], Refused),
    ("revert", &["T-001"], Refused),
    ("review-sprint", &[], Refused),
    ("idea", &["T-001", "drop", "--reason", "r"], Refused),
    ("lessons", &[], Read),
    ("lessons distill", &[], Refused),
    ("lessons apply", &["--file", "l.json"], Refused),
    ("lessons pin", &["L-1"], Refused),
    ("lessons delete", &["L-1"], Refused),
    ("lessons due", &[], Read),
    ("plan progress", &["T-001"], Read),
    ("plan verify", &["T-001"], Refused),
    ("plan checkpoint", &["T-001"], Refused),
    ("plan evidence", &["T-001", "--file", "e.json"], Refused),
    (
        "plan aggregate-review",
        &["T-001", "--attempt", "a"],
        Refused,
    ),
    (
        "plan aggregate-verdict",
        &[
            "T-001",
            "approve",
            "--attempt",
            "a",
            "--review",
            "r",
            "--notes-file",
            "n.md",
        ],
        Refused,
    ),
    ("plan accept", &["T-001", "--attempt", "a"], Refused),
    ("plan cancel", &["T-001"], Refused),
    ("plan submit", &["T-001", "plan.md"], Refused),
    ("plan show", &["T-001"], Read),
    ("plan review", &["T-001"], Refused),
    (
        "plan verdict",
        &["T-001", "approve", "--plan", "P1", "--notes-file", "n.md"],
        Refused,
    ),
    (
        "plan member-gate",
        &[
            "T-001",
            "--file",
            "g.json",
            "--expected-revision",
            "0",
            "--reason",
            "r",
        ],
        Refused,
    ),
    ("plan add-member", &["T-001", "T-002"], Refused),
    (
        "plan move-member",
        &["T-001", "T-002", "--to", "T-003"],
        Refused,
    ),
    ("plan policy", &["T-001", "required"], Refused),
    ("build-identity", &[], Read),
    ("hooks list", &[], Read),
    ("hooks reconcile", &[], Refused),
    ("hooks retry", &["app", "abc123"], Refused),
    ("run wait", &["T-001", "--file", "w.json"], Refused),
    ("run decision", &["T-001", "--file", "d.json"], Refused),
    (
        "run resume",
        &["T-001", "--file", "r.json", "--wait", "w-1"],
        Refused,
    ),
    ("run progress", &["T-001"], Read),
    ("run progress", &["T-001", "--file", "p.json"], Refused),
    ("run checkpoint", &["T-001"], Read),
    ("run checkpoint", &["T-001", "--file", "c.json"], Refused),
    ("queue add", &["T-001"], Refused),
    ("queue remove", &["T-001"], Refused),
    ("queue pause", &[], Refused),
    ("queue resume", &[], Refused),
    ("queue admit", &["app"], Refused),
    ("queue list", &[], Read),
    ("wait", &["--timeout", "1s"], Read),
    ("up", &[], Refused),
    ("doctor", &[], Read),
    ("doctor", &["--fix"], Refused),
    ("init", &[], Refused),
    ("pricing check", &[], Read),
    ("import", &["plan.md"], Refused),
    ("stats", &[], Read),
    ("new", &["Filed", "--idea"], File),
    ("new", &["Ready", "--ready"], Refused),
    ("new", &["Claimed", "--origin", "pm"], Refused),
    ("list", &[], Read),
    ("search", &["needle"], Read),
    ("log", &["T-001"], Read),
    ("show", &["T-001"], Read),
    ("comment", &["T-001", "note"], File),
    ("comment", &["T-001", "note", "--send"], Refused),
    ("comment", &["T-001", "note", "--author", "pm"], Refused),
    ("comment", &["T-001", "owner-ok: drop not needed"], Refused),
    ("move", &["T-001", "ready"], Refused),
    ("edit", &["T-001"], Refused),
    ("route", &["T-001"], Read),
    ("route", &["T-001", "--set-reviewer", "none"], Refused),
    ("route", &["T-001", "--clear-reviewer"], Refused),
    ("dispatch", &["T-001"], Refused),
    ("dispatch", &["T-001", "--explain"], Read),
    ("reply", &["T-001", "hi"], Refused),
    ("unstick", &["T-001"], Refused),
    ("done", &["T-001", "--summary", "s"], Refused),
    ("done", &["T-001", "--check"], Read),
    ("criteria register", &["T-001", "--file", "c.json"], Refused),
    ("criteria list", &["T-001"], Read),
    ("evidence record", &["T-001", "--file", "e.json"], Refused),
    ("evidence list", &["T-001"], Read),
    ("gate", &["T-001"], Refused),
    ("review-run", &["T-001"], Refused),
    ("verdict", &["T-001", "approve", "--notes", "n"], Refused),
    (
        "dashboard set",
        &[
            "T-001",
            "--interval-secs",
            "60",
            "--next-due",
            "2026-11-01T00:00:00Z",
        ],
        Refused,
    ),
    ("dashboard show", &["T-001"], Read),
    ("dashboard pause", &["T-001"], Refused),
    ("dashboard remove", &["T-001"], Refused),
    ("artifact set-tier", &["T-001:R1:x.md", "keep"], Refused),
    ("artifact gc", &["--dry-run"], Refused),
    ("artifact gc", &["--yes"], Refused),
    ("artifact add", &["T-001", "report.md"], Refused),
    ("artifact add", &["--session", "A-1", "report.md"], File),
    ("artifact add", &["--session", "A-2", "report.md"], Refused),
    ("artifact list", &[], Read),
    ("artifact serve", &["--stop"], Read),
    ("artifact show", &["T-001:R1:x.md"], Read),
    ("pin", &["T-001"], Refused),
    ("unpin", &["T-001"], Refused),
    ("pins", &[], Read),
    ("open", &["T-404"], Read),
    ("review", &["T-001"], Read),
    ("approve", &["T-001"], Refused),
    ("changes", &["T-001", "fix"], Refused),
    ("retry", &["T-001"], Refused),
    ("discard", &["T-001"], Refused),
    ("jump", &["T-001"], Refused),
    ("sync", &[], Refused),
    ("autopilot", &[], Read),
    ("autopilot", &["on"], Refused),
    ("hook", &[], Refused),
    (
        "bind-session",
        &["--socket", "/nonexistent/s", "--launch-record", "r.json"],
        Refused,
    ),
    ("pm", &[], Refused),
    ("pm bench", &["--events", "e.jsonl"], Read),
    ("pm next", &[], Read),
    ("pm brief", &[], Read),
    ("pm act", &["a-1"], Refused),
    ("pm present", &["d-1", "--delivery", "pane"], Refused),
    ("pm resolve", &["d-1", "--answer", "yes"], Refused),
    ("focus-board", &[], Refused),
    ("board", &[], Refused),
    ("capture", &[], Refused),
    ("artifacts", &[], Refused),
    ("settings", &[], Refused),
    ("harness list", &[], Read),
    ("plugin install", &["--print"], Refused),
    ("skill", &[], Read),
    ("skill install", &[], Refused),
    ("config diff", &[], Read),
    ("config history", &[], Read),
    ("config get", &["limits.running"], Read),
    ("config set", &["limits.running", "7"], Refused),
    ("config validate", &[], Read),
    ("config path", &[], Read),
    ("runtime-probe", &[], Read),
];

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl Fixture {
    /// A workspace with project `corral`, task T-001 and sessions A-1 and A-2, made by the owner.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("ws");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(home.join("bin")).unwrap();
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
        fx.ok(None, &["new", "Existing task", "-p", "corral"]);
        fx.ok(None, &["assistant", "create"]);
        fx.ok(None, &["assistant", "create"]);
        std::fs::write(fx.root.join("summary.md"), "# Notes\n").unwrap();
        std::fs::write(fx.root.join("report.md"), "# Report\n").unwrap();
        fx
    }

    /// Runs with a cleared environment: only the assistant marker, a private HOME/XDG and an
    /// empty PATH, so no inherited identity or installed tool can affect the class.
    fn run(&self, assistant: Option<&str>, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_corral"));
        cmd.env_clear()
            .env("PATH", self.home.join("bin"))
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .env("HERDR_BIN_PATH", "/nonexistent/herdr");
        if let Some(session) = assistant {
            cmd.env("CORRAL_ASSISTANT", session);
        }
        cmd.arg("--root")
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

    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    let bytes = std::fs::read(&path).unwrap();
                    out.insert(path, bytes);
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.root, &mut out);
        out
    }

    fn events(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.root.join(".corral/events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn assert_refused(&self, path: &str, extra: &[&str]) {
        let mut args: Vec<&str> = path.split_whitespace().collect();
        args.extend_from_slice(extra);
        let named = if path.is_empty() { "up" } else { path };
        let before = self.snapshot();
        let out = self.run(Some("A-1"), &args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(
            stderr.contains(&format!("assistant_refused: corral {named} {ROUTE}")),
            "{args:?}: {stderr}"
        );
        assert!(self.snapshot() == before, "{args:?} changed the workspace");
    }
}

/// Every command path in `corral --help`, recursively; a parent whose subcommand is optional
/// is itself invocable. `ext` runs external executables before any `Cmd` is parsed.
fn help_paths(fx: &Fixture) -> BTreeSet<String> {
    fn walk(fx: &Fixture, words: &mut Vec<String>, out: &mut BTreeSet<String>) {
        let mut args: Vec<&str> = words.iter().map(String::as_str).collect();
        args.push("--help");
        let help = fx.ok(None, &args);
        let mut subcommands = Vec::new();
        let mut listing = false;
        for line in help.lines() {
            if line.starts_with("Commands:") {
                listing = true;
            } else if listing && !line.starts_with(' ') {
                listing = false;
            } else if listing && line[2..].starts_with(|c: char| c.is_ascii_lowercase()) {
                let name = line.split_whitespace().next().unwrap();
                if name != "help" && !(words.is_empty() && name == "ext") {
                    subcommands.push(name.to_owned());
                }
            }
        }
        let usage = help.lines().find(|l| l.starts_with("Usage:")).unwrap();
        if subcommands.is_empty() || !usage.contains("<COMMAND>") {
            out.insert(words.join(" "));
        }
        for name in subcommands {
            words.push(name);
            walk(fx, words, out);
            words.pop();
        }
    }
    let mut out = BTreeSet::new();
    walk(fx, &mut Vec::new(), &mut out);
    out
}

#[test]
fn every_command_variant_has_an_explicit_class() {
    // Given the owner's workspace and the complete command tree the binary reports.
    let fx = Fixture::new();
    let mut expected = help_paths(&fx);
    expected.extend(HIDDEN.map(str::to_owned));
    let covered: BTreeSet<String> = ROWS.iter().map(|(path, ..)| (*path).to_owned()).collect();
    assert_eq!(
        covered.symmetric_difference(&expected).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "every command path needs a row, and every row a real command"
    );
    // When assistant A-1 (with no AI_AGENT or PI_CODING_AGENT) runs each row.
    for (path, extra, class) in ROWS {
        // Then Refused rows exit 2 with the route and change nothing; the others run.
        if *class == Refused {
            fx.assert_refused(path, extra);
            continue;
        }
        let mut args: Vec<&str> = path.split_whitespace().collect();
        args.extend_from_slice(extra);
        let out = fx.run(Some("A-1"), &args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.code() != Some(2) && !stderr.contains("assistant_refused"),
            "{args:?} is {class:?}: {stderr}"
        );
    }
}

#[test]
fn c2_reads_and_filing_are_attributed_and_owner_commands_change_nothing() {
    // Given task T-001 and assistant A-1, both created by the owner.
    let fx = Fixture::new();
    // When A-1 reads the board.
    let shown: serde_json::Value =
        serde_json::from_str(&fx.ok(Some("A-1"), &["show", "T-001", "--json"])).unwrap();
    let stats: serde_json::Value =
        serde_json::from_str(&fx.ok(Some("A-1"), &["stats", "--json"])).unwrap();
    // Then the reads succeed as for the owner.
    assert_eq!(shown["id"], "T-001");
    assert!(stats.is_array());
    // When A-1 files an idea, a comment and a report on its own session.
    let idea = fx.ok(Some("A-1"), &["new", "Fix X", "-p", "corral", "--idea"]);
    let idea = idea.trim();
    fx.ok(Some("A-1"), &["comment", "T-001", "Seen while reading"]);
    fx.ok(
        Some("A-1"),
        &[
            "artifact",
            "add",
            "--session",
            "A-1",
            "report.md",
            "--kind",
            "report",
        ],
    );
    // Then all three are recorded with actor assistant via assistant:A-1.
    let events = fx.events();
    for (kind, target) in [
        ("created", idea),
        ("commented", "T-001"),
        ("artifact", "A-1"),
    ] {
        let event = events
            .iter()
            .rfind(|e| {
                e["kind"] == kind && (e["task"] == target || e["detail"]["session"] == target)
            })
            .unwrap_or_else(|| panic!("no {kind} event for {target}"));
        assert_eq!(event["actor"], "assistant", "{event}");
        assert_eq!(event["via"], "assistant:A-1", "{event}");
    }
    let filed: serde_json::Value =
        serde_json::from_str(&fx.ok(None, &["show", idea, "--json"])).unwrap();
    assert_eq!(filed["status"], "idea");
    // When A-1 tries owner actions from its shell, then each refuses and changes nothing.
    for (path, extra) in [
        ("approve", &["T-001"][..]),
        ("move", &["T-001", "ready"]),
        ("config set", &["limits.running", "7"]),
    ] {
        fx.assert_refused(path, extra);
    }
}
