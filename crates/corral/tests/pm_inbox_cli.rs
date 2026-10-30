//! pm-inbox (T-544 C7): the PM inbox through the CLI, as an assistant shell, the owner and an
//! OmO PM sweep (`CORRAL_PM=1`, plain `corral pm next`) use it. No herdr exists here, so
//! nothing can be typed into any pane.
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

const A1: &[(&str, &str)] = &[("CORRAL_ASSISTANT", "A-1")];
const A2: &[(&str, &str)] = &[("CORRAL_ASSISTANT", "A-2")];
const PM: &[(&str, &str)] = &[("CORRAL_PM", "1")];
const OWNER: &[(&str, &str)] = &[];

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl Fixture {
    /// A workspace with project `corral`, task T-001 and open sessions A-1 and A-2 (no host).
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
        fx.ok(OWNER, &["init"]);
        let config = fx.root.join(".corral/corral.toml");
        let mut text = std::fs::read_to_string(&config).unwrap();
        text.push_str("\n[projects.corral]\npath = '.'\n");
        std::fs::write(&config, text).unwrap();
        fx.ok(OWNER, &["new", "Existing task", "-p", "corral"]);
        fx.ok(OWNER, &["assistant", "create"]);
        fx.ok(OWNER, &["assistant", "create"]);
        fx
    }

    fn run(&self, env: &[(&str, &str)], args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_corral"));
        cmd.env_clear()
            .env("PATH", self.home.join("bin"))
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .env("XDG_RUNTIME_DIR", self.home.join("run"))
            .env("HERDR_BIN_PATH", "/nonexistent/herdr");
        for (key, value) in env {
            cmd.env(key, value);
        }
        cmd.arg("--root")
            .arg(&self.root)
            .args(args)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn ok(&self, env: &[(&str, &str)], args: &[&str]) -> String {
        let out = self.run(env, args);
        assert!(
            out.status.success(),
            "{args:?}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn json(&self, env: &[(&str, &str)], args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(env, args)).unwrap()
    }

    /// Exit code and stderr of a refused command, which must leave the PM inbox unchanged.
    fn refused(&self, env: &[(&str, &str)], args: &[&str]) -> (Option<i32>, String) {
        let before = self.inbox_files();
        let out = self.run(env, args);
        assert!(!out.status.success(), "{args:?} succeeded");
        assert_eq!(self.inbox_files(), before, "{args:?} changed the PM inbox");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn inbox_files(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files: Vec<_> = std::fs::read_dir(self.root.join(".corral/pm-inbox"))
            .map(|entries| {
                entries
                    .map(|e| e.unwrap().path())
                    .map(|p| (p.clone(), std::fs::read(p).unwrap()))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        files
    }

    fn event(&self, kind: &str) -> Value {
        std::fs::read_to_string(self.root.join(".corral/events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .rfind(|e| e["kind"] == kind)
            .unwrap_or_else(|| panic!("no {kind} event"))
    }
}

#[test]
fn c7_a_routed_refusal_is_swept_claimed_resolved_and_notified_once() {
    // Given A-1's refused owner command, routed to the PM as an action request.
    let fx = Fixture::new();
    let (code, stderr) = fx.refused(A1, &["approve", "T-001"]);
    assert_eq!(code, Some(2));
    assert!(stderr.contains("corral pm inbox post"), "{stderr}");
    let posted = fx.json(
        A1,
        &[
            "pm",
            "inbox",
            "post",
            "--kind",
            "action_request",
            "--subject",
            "Approve T-001",
            "--ref",
            "T-001",
            "--action",
            "approve T-001",
            "--quote",
            "T-001 looks good, please land it",
            "--json",
        ],
    );
    assert_eq!(
        (posted["id"].as_str(), posted["revision"].as_u64()),
        (Some("PMI-1"), Some(0))
    );
    assert_eq!(posted["from"], "assistant:A-1");
    assert_eq!(posted["requested_action"]["kind"], "approve");
    let event = fx.event("pm_inbox_posted");
    assert_eq!(
        (event["actor"].as_str(), event["via"].as_str()),
        (Some("assistant"), Some("assistant:A-1"))
    );
    // When the OmO PM sweeps with `corral pm next`.
    let text = fx.ok(PM, &["pm", "next"]);
    let next = fx.json(PM, &["pm", "next", "--json"]);
    // Then the card lists PMI-1 with its source session and kind, and claiming is next.
    assert!(
        text.contains("PMI-1 from assistant:A-1 (action_request): Approve T-001"),
        "{text}"
    );
    assert_eq!(next["kind"], "inbox");
    assert!(
        next["command"]
            .as_str()
            .unwrap()
            .contains("pm inbox claim 'PMI-1' --expected-revision 0"),
        "{next}"
    );
    assert!(
        next["evidence"]
            .to_string()
            .contains("requested: corral approve T-001")
    );
    assert_eq!(next["workset"]["inbox"].as_array().unwrap().len(), 1);
    // When it claims at revision 0, resolves done with refs at revision 1, then claims at 0 again.
    let claimed = fx.json(
        PM,
        &[
            "pm",
            "inbox",
            "claim",
            "PMI-1",
            "--expected-revision",
            "0",
            "--json",
        ],
    );
    let resolved = fx.json(
        PM,
        &[
            "pm",
            "inbox",
            "resolve",
            "PMI-1",
            "--expected-revision",
            "1",
            "--outcome",
            "done",
            "--text",
            "Asked the owner; T-001 landed.",
            "--ref",
            "T-001",
            "--json",
        ],
    );
    let (code, stderr) = fx.refused(
        PM,
        &["pm", "inbox", "claim", "PMI-1", "--expected-revision", "0"],
    );
    // Then claim and resolve carry PM attribution at revisions 1 and 2 with an outcome.
    assert_eq!(
        (claimed["revision"].as_u64(), claimed["state"].as_str()),
        (Some(1), Some("claimed"))
    );
    assert_eq!(
        (resolved["revision"].as_u64(), resolved["state"].as_str()),
        (Some(2), Some("resolved"))
    );
    assert_eq!(resolved["outcome"]["resolution"], "done");
    assert_eq!(resolved["outcome"]["refs"][0], "T-001");
    for (kind, revision) in [("pm_inbox_claimed", 1), ("pm_inbox_resolved", 2)] {
        let event = fx.event(kind);
        assert_eq!(event["actor"], "pm", "{event}");
        assert_eq!(event["detail"]["revision"], revision, "{event}");
    }
    // And the stale claim refused stale_revision.
    assert_eq!(code, Some(1));
    assert!(
        stderr.contains("stale_revision: PMI-1 is at revision 2, not 0"),
        "{stderr}"
    );
    // And A-1's native agent inbox holds exactly one notice, pmi:PMI-1:2, queued for its host.
    assert_eq!(resolved["notice"]["key"], "pmi:PMI-1:2");
    assert_eq!(resolved["notice"]["state"], "posted");
    let inbox = fx.root.join(".corral/inbox/assistant_A-1");
    let messages: Vec<_> = std::fs::read_dir(&inbox)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "md"))
        .collect();
    assert_eq!(messages.len(), 1, "{messages:?}");
    let message = std::fs::read_to_string(&messages[0]).unwrap();
    assert!(message.contains("pmi:PMI-1:2") && message.contains("pm_inbox_outcome"));
    // And the sweep no longer shows a card for the resolved item.
    assert!(
        fx.json(PM, &["pm", "next", "--json"])["workset"]["inbox"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn the_author_or_owner_withdraws_and_other_callers_are_refused() {
    // Given a question from A-1.
    let fx = Fixture::new();
    fx.ok(
        A1,
        &[
            "pm",
            "inbox",
            "post",
            "--kind",
            "question",
            "--subject",
            "Which cache?",
        ],
    );
    // When another session, the PM and A-2's shell try to move it.
    let withdraw = [
        "pm",
        "inbox",
        "withdraw",
        "PMI-1",
        "--expected-revision",
        "0",
    ];
    let (code, stderr) = fx.refused(A2, &withdraw);
    assert_eq!(code, Some(1));
    assert!(stderr.contains("not_author"), "{stderr}");
    let (_, stderr) = fx.refused(PM, &withdraw);
    assert!(stderr.contains("not_author"), "{stderr}");
    let (code, stderr) = fx.refused(
        A2,
        &["pm", "inbox", "claim", "PMI-1", "--expected-revision", "0"],
    );
    assert_eq!(code, Some(2));
    assert!(
        stderr.contains("assistant_refused: corral pm inbox claim"),
        "{stderr}"
    );
    // When the author withdraws it.
    let withdrawn = fx.json(
        A1,
        &[
            "pm",
            "inbox",
            "withdraw",
            "PMI-1",
            "--expected-revision",
            "0",
            "--reason",
            "answered",
            "--json",
        ],
    );
    // Then it is withdrawn by A-1 at revision 1, attributed to the assistant, and leaves the sweep.
    assert_eq!(
        (withdrawn["revision"].as_u64(), withdrawn["state"].as_str()),
        (Some(1), Some("withdrawn"))
    );
    assert_eq!(withdrawn["withdrawn"]["by"], "assistant:A-1");
    let event = fx.event("pm_inbox_withdrawn");
    assert_eq!(
        (event["actor"].as_str(), event["via"].as_str()),
        (Some("assistant"), Some("assistant:A-1"))
    );
    assert!(
        fx.json(OWNER, &["pm", "inbox", "list", "--json"])
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fx.json(OWNER, &["pm", "inbox", "list", "--all", "--json"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // When the owner posts and withdraws its own item from a plain shell.
    fx.ok(
        OWNER,
        &[
            "pm",
            "inbox",
            "post",
            "--kind",
            "handoff",
            "--subject",
            "Tidy labels",
        ],
    );
    let event = fx.event("pm_inbox_posted");
    assert_eq!(
        (event["actor"].as_str(), event["detail"]["from"].as_str()),
        (Some("owner"), Some("owner"))
    );
    fx.ok(
        OWNER,
        &[
            "pm",
            "inbox",
            "withdraw",
            "PMI-2",
            "--expected-revision",
            "0",
        ],
    );
    // Then the shell attribution is the owner, not the PM.
    assert_eq!(fx.event("pm_inbox_withdrawn")["actor"], "owner");
}
