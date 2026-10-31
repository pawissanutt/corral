//! owner-authority (T-544 C3, confirm-class table): every owner-action kind is one real corral
//! command with flags that command takes, and an owner action's command child acts for the
//! owner via its assistant: attributed and carrying the owner message, past the owner guards,
//! exactly once, and only as the direct child of the reserving executor.
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use std::io::Write;

use corral_core::Store;
use corral_core::agent::{RequestReceipt, paths};
use corral_core::assistant::{NewOwnerInput, OwnerInputSource};
use corral_ops::agent::{BindTarget, open_generation, payload_sha256, tool_request_id};
use corral_ops::assistant::owner_action::kinds::{self, Class, KINDS, Target};
use serde_json::{Value, json};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl Fixture {
    /// A workspace with project `corral`, Backlog task T-001 and assistant session A-1.
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
        fx.ok(&["init"], &[]);
        let config = fx.root.join(".corral/corral.toml");
        let mut text = std::fs::read_to_string(&config).unwrap();
        text.push_str("\n[projects.corral]\npath = '.'\n");
        std::fs::write(&config, text).unwrap();
        fx.ok(&["new", "Existing task", "-p", "corral"], &[]);
        fx.ok(&["assistant", "create"], &[]);
        fx
    }

    /// `corral <args>` at the workspace root with a cleared environment plus `env`.
    fn command(&self, program: &str, args: &[&str], env: &[(&str, &str)]) -> Command {
        let mut cmd = Command::new(program);
        cmd.env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.home.join("bin").display()),
            )
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("XDG_CACHE_HOME", self.home.join("cache"))
            .env("HERDR_BIN_PATH", "/nonexistent/herdr")
            .envs(env.iter().copied())
            .args(args)
            .current_dir(&self.root)
            .stdin(Stdio::null());
        cmd
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let corral = env!("CARGO_BIN_EXE_corral");
        self.command(corral, args, env).output().unwrap()
    }

    fn ok(&self, args: &[&str], env: &[(&str, &str)]) -> String {
        let out = self.run(args, env);
        assert!(out.status.success(), "{args:?}: {}", text(&out));
        String::from_utf8(out.stdout).unwrap()
    }

    fn store(&self) -> Store {
        Store::open(&self.root).unwrap()
    }

    fn status(&self) -> String {
        let shown: Value =
            serde_json::from_str(&self.ok(&["show", "T-001", "--json"], &[])).unwrap();
        shown["status"].as_str().unwrap().to_owned()
    }

    /// The owner typed `text` into A-1's pane; returns its journal ID.
    fn journal(&self, text: &str) -> String {
        let input = NewOwnerInput {
            generation: 1,
            turn: None,
            source: OwnerInputSource::Pane,
            sha256: paths::sha256_hex(text.as_bytes()),
            text: text.into(),
            truncated: false,
            command: None,
            answers: None,
            request: "ev-1-1".into(),
        };
        let store = self.store();
        store
            .update_assistant_container("A-1", |container| {
                Ok(container.journal_owner_input("A-1", input).id)
            })
            .unwrap()
    }

    /// A reservation of `argv` citing `input`, held by this test process as its executor.
    fn reserve(&self, request: &str, argv: &[&str], input: &str) {
        let store = self.store();
        let executor = corral_ops::agent::effects::executor().unwrap();
        let receipt: RequestReceipt = serde_json::from_value(json!({
            "version": 1, "request_id": request, "op": "owner_action", "payload_sha256": "00",
            "identity": {"store": paths::store_id(&store), "project": "", "role": "assistant",
                "assistant": "A-1", "session_id": "s", "generation": 1},
            "state": "reserved", "executor": executor, "at": "2026-10-30T00:00:00Z",
            "result": {"kind": argv[0], "argv": argv, "input": input, "sha256": "ab",
                "confirmed": false, "outcome": "reserved"},
        }))
        .unwrap();
        store
            .update_assistant_container("A-1", |container| {
                container.state.receipts.push(receipt);
                Ok(())
            })
            .unwrap();
    }

    fn events(&self) -> Vec<Value> {
        std::fs::read_to_string(self.root.join(".corral/events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn every_owner_action_kind_is_a_real_command_with_its_flags_and_a_fixed_class() {
    let fx = Fixture::new();
    for spec in &KINDS {
        let choices: &[&str] = match spec.target {
            Target::Choice(choices) => choices,
            Target::Task | Target::Key | Target::Scratch => &[""],
        };
        let mut help = String::new();
        for choice in choices {
            let mut args: Vec<&str> = spec.words.to_vec();
            args.extend((!choice.is_empty()).then_some(*choice));
            args.push("--help");
            help.push_str(&fx.ok(&args, &[]));
        }
        let revision = spec.revision.then_some(("--expected-revision", true));
        for (flag, _) in spec.flags.iter().copied().chain(revision) {
            assert!(help.contains(flag), "{} lacks {flag}:\n{help}", spec.kind);
        }
    }
    let class = |kind: &str, target: &str, args: &[&str]| {
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        kinds::resolve(kind, Some(target), &args, None).unwrap()
    };
    for (kind, target, args, expected, command) in [
        (
            "move",
            "T-003",
            &["dropped"][..],
            Class::Confirm,
            "corral move T-003 dropped",
        ),
        (
            "move",
            "T-003",
            &["ready", "--force"],
            Class::Confirm,
            "corral move T-003 ready --force",
        ),
        (
            "discard",
            "T-003",
            &[],
            Class::Confirm,
            "corral discard T-003",
        ),
        (
            "discard",
            "T-003",
            &["--keep-branch"],
            Class::Confirm,
            "corral discard T-003 --keep-branch",
        ),
        (
            "config_set",
            "limits.running",
            &["3"],
            Class::Confirm,
            "corral config set limits.running 3",
        ),
        (
            "mode",
            "autopilot",
            &["--confirm", "pv-1"],
            Class::Confirm,
            "corral mode autopilot --confirm pv-1",
        ),
        (
            "approve",
            "T-005",
            &[],
            Class::Ordinary,
            "corral approve T-005",
        ),
        (
            "dispatch",
            "T-009",
            &[],
            Class::Ordinary,
            "corral dispatch T-009",
        ),
        (
            "queue",
            "T-009",
            &[],
            Class::Ordinary,
            "corral queue add T-009",
        ),
        (
            "move",
            "T-003",
            &["review"],
            Class::Ordinary,
            "corral move T-003 review",
        ),
        (
            "verdict",
            "T-005",
            &["approve", "--notes", "ok"],
            Class::Ordinary,
            "corral verdict T-005 approve --notes ok",
        ),
        (
            "review_run",
            "T-005",
            &[],
            Class::Ordinary,
            "corral review-run T-005",
        ),
        (
            "plan_accept",
            "T-007",
            &["--attempt", "pa-1"],
            Class::Ordinary,
            "corral plan accept T-007 --attempt pa-1",
        ),
        (
            "finish",
            "cache-fix",
            &["--title", "Fix the cache", "--review", "none"],
            Class::Ordinary,
            "corral assistant scratch finish cache-fix --title 'Fix the cache' --review none",
        ),
    ] {
        let action = class(kind, target, args);
        assert_eq!(
            (action.class, action.command.as_str()),
            (expected, command),
            "{kind} {args:?}"
        );
    }
}

#[test]
fn an_owner_action_child_acts_for_the_owner_via_its_assistant_exactly_once() {
    // Given A-1 journaled the owner's instruction, and its shell may not move tasks itself.
    let fx = Fixture::new();
    let message = "move T-001 to ready please";
    let input = fx.journal(message);
    let shell = fx.run(&["move", "T-001", "ready"], &[("CORRAL_ASSISTANT", "A-1")]);
    assert_eq!(shell.status.code(), Some(2), "{}", text(&shell));
    assert!(text(&shell).contains("assistant_refused"));
    // When the reserving executor (this test) runs the reserved command as its direct child.
    fx.reserve("rq-move", &["move", "T-001", "ready"], &input);
    let env = [
        ("CORRAL_ASSISTANT", "A-1"),
        ("CORRAL_OWNER_ACTION", "A-1:rq-move"),
    ];
    fx.ok(&["move", "T-001", "ready"], &env);
    // Then the task moved, and its events name the owner via A-1 and carry the owner message.
    assert_eq!(fx.status(), "ready");
    let events: Vec<Value> = fx
        .events()
        .into_iter()
        .filter(|event| event["detail"]["owner_authority"].is_object())
        .collect();
    assert!(!events.is_empty());
    for event in &events {
        assert_eq!(
            (event["actor"].as_str(), event["via"].as_str()),
            (Some("owner"), Some("assistant:A-1"))
        );
        let authority = &event["detail"]["owner_authority"];
        assert_eq!(authority["owner_message"], message);
        assert_eq!(authority["input"], input.as_str());
        assert_eq!(authority["session"], "A-1");
        assert_eq!(authority["generation"], 1);
        assert_eq!(authority["request"], "rq-move");
    }
    // And the same reservation never runs a second command.
    let again = fx.run(&["move", "T-001", "ready"], &env);
    assert!(
        text(&again).contains("owner_action_unclaimed"),
        "{}",
        text(&again)
    );
    // A process the command started in turn (here through a shell) is not the claimed child:
    // it is the assistant's shell again and is refused.
    fx.reserve("rq-back", &["move", "T-001", "backlog"], &input);
    let corral = env!("CARGO_BIN_EXE_corral");
    let nested = fx
        .command(
            "sh",
            &["-c", &format!("'{corral}' move T-001 backlog; exit $?")],
            &[
                ("CORRAL_ASSISTANT", "A-1"),
                ("CORRAL_OWNER_ACTION", "A-1:rq-back"),
            ],
        )
        .output()
        .unwrap();
    assert_eq!(nested.status.code(), Some(2), "{}", text(&nested));
    assert_eq!(fx.status(), "ready");
    // And an owner guard that refuses every assistant shell accepts a claimed owner action.
    fx.reserve("rq-mode", &["mode", "assist"], &input);
    let refused = fx.run(&["mode", "assist"], &[("CORRAL_ASSISTANT", "A-1")]);
    assert_eq!(refused.status.code(), Some(2), "{}", text(&refused));
    fx.ok(
        &["mode", "assist"],
        &[
            ("CORRAL_ASSISTANT", "A-1"),
            ("CORRAL_OWNER_ACTION", "A-1:rq-mode"),
        ],
    );
}

/// A-1's host as corral sees it: generation 1 bound by hello, and its `corral agent call`s.
struct Host<'a> {
    fx: &'a Fixture,
    identity: Value,
    seq: u64,
}

impl<'a> Host<'a> {
    fn bind(fx: &'a Fixture) -> Self {
        let store = fx.store();
        let target = BindTarget::Assistant {
            session: "A-1".into(),
        };
        let grant = open_generation(&store, &target, "p1").unwrap();
        let identity = json!({"store": paths::store_id(&store), "project": "", "role": "assistant",
            "assistant": "A-1", "session_id": "s", "generation": grant.generation});
        let mut host = Self {
            fx,
            identity,
            seq: 0,
        };
        let me = corral_ops::agent::effects::executor().unwrap();
        let sessions = paths::container_sessions_dir(&store, &grant.container);
        let hello = json!({"ctl_cap_sha256": paths::sha256_hex("c".repeat(64).as_bytes()),
            "session_file": sessions.join("s.jsonl"), "pid": me.pid, "start_time": me.start_time,
            "pane": "p1"});
        let bound = host.call("hello", "ev-1-0", hello, Some(&grant.bind_nonce));
        assert_eq!(bound["state"], "committed", "{bound}");
        host
    }

    fn call(&mut self, op: &str, request: &str, payload: Value, nonce: Option<&str>) -> Value {
        let mut auth = json!({"host_cap": "h".repeat(64)});
        if let Some(nonce) = nonce {
            auth["bind_nonce"] = json!(nonce);
        }
        let envelope = json!({"version": 1, "op": op, "request_id": request,
            "payload_sha256": payload_sha256(&payload), "identity": self.identity, "auth": auth,
            "payload": payload});
        let corral = env!("CARGO_BIN_EXE_corral");
        let mut child = self
            .fx
            .command(
                corral,
                &["agent", "call", "--op", op],
                &[("CORRAL_ASSISTANT", "A-1")],
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(envelope.to_string().as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{op}: {}", text(&out));
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// The owner typed `text` (or answered a confirmation of `command`) in the pane.
    fn journal(&mut self, text: &str, answer: Option<(&str, &str)>) -> String {
        self.seq += 1;
        let source = if answer.is_some() {
            "confirmation"
        } else {
            "pane"
        };
        let mut payload = json!({"kind": "owner_input", "source": source, "text": text,
            "sha256": paths::sha256_hex(text.as_bytes())});
        if let Some((command, call)) = answer {
            payload["command"] = json!(command);
            payload["answers"] = json!(tool_request_id("s", call));
        }
        let request = format!("ev-1-{}", self.seq);
        let receipt = self.call("event", &request, payload, None);
        receipt["result"]["input"].as_str().unwrap().to_owned()
    }

    /// `corral_owner_action` for the model's tool call `call`.
    fn act(&mut self, call: &str, payload: Value) -> Value {
        let request = tool_request_id("s", call);
        self.call("owner_action", &request, payload, None)
    }
}

#[test]
fn owner_actions_run_through_the_real_agent_call_and_child_once() {
    // Given A-1 bound by hello, and the owner's "do it" after it proposed moving T-001 to Ready.
    let fx = Fixture::new();
    let mut host = Host::bind(&fx);
    let input = host.journal("do it", None);
    assert_eq!(input, "A-1/g1/i1");
    // When the model cites a PM outcome notice, nothing runs; citing "do it" moves the task once.
    let wake = host.act(
        "rq-wake",
        json!({"kind": "move", "target": "T-001", "args": ["ready"],
        "input": "pmi:PMI-1:2"}),
    );
    assert_eq!(wake["result"]["code"], "not_owner_input", "{wake}");
    assert_eq!(fx.status(), "backlog");
    let payload = json!({"kind": "move", "target": "T-001", "args": ["ready"], "input": input});
    let moved = host.act("rq-move", payload.clone());
    assert_eq!(
        (moved["state"].as_str(), moved["result"]["outcome"].as_str()),
        (Some("committed"), Some("executed")),
        "{moved}"
    );
    assert_eq!(fx.status(), "ready");
    let carried: Vec<Value> = fx
        .events()
        .into_iter()
        .filter(|event| {
            event["detail"]["owner_authority"]["request"] == tool_request_id("s", "rq-move")
        })
        .collect();
    assert!(!carried.is_empty());
    for event in &carried {
        assert_eq!(
            (event["actor"].as_str(), event["via"].as_str()),
            (Some("owner"), Some("assistant:A-1"))
        );
        assert_eq!(event["detail"]["owner_authority"]["owner_message"], "do it");
    }
    // And a replay returns the first receipt without a second effect.
    let replay = host.act("rq-move", payload);
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["committed_at"], moved["committed_at"]);
    let after: Vec<Value> = fx
        .events()
        .into_iter()
        .filter(|event| {
            event["detail"]["owner_authority"]["request"] == tool_request_id("s", "rq-move")
        })
        .collect();
    assert_eq!(after.len(), carried.len());
    // When the owner says to set limits.running to 3, the host must first show the command.
    let set = host.journal("set limits.running to 3", None);
    let config =
        json!({"kind": "config_set", "target": "limits.running", "args": ["3"], "input": set});
    let asked = host.act("rq-no", config.clone());
    assert_eq!(asked["result"]["code"], "confirmation_required");
    let command = asked["result"]["command"].as_str().unwrap().to_owned();
    assert_eq!(command, "corral config set limits.running 3");
    let toml = || std::fs::read_to_string(fx.root.join(".corral/corral.toml")).unwrap();
    let before = toml();
    // N for this call, then the identical call: a declined receipt, corral.toml unchanged, and
    // a replay returns that receipt.
    host.journal("n", Some((&command, "rq-no")));
    let declined = host.act("rq-no", config.clone());
    assert_eq!(declined["result"]["outcome"], "declined", "{declined}");
    let replayed = host.act("rq-no", config.clone());
    assert_eq!(
        (&replayed["replayed"], &replayed["result"]["outcome"]),
        (&json!(true), &json!("declined"))
    );
    assert_eq!(toml(), before);
    // y for a new call sets it once; replaying that identical call changes nothing more.
    let asked = host.act("rq-yes", config.clone());
    assert_eq!(asked["result"]["code"], "confirmation_required");
    host.journal("y", Some((&command, "rq-yes")));
    let done = host.act("rq-yes", config.clone());
    assert_eq!(done["result"]["outcome"], "executed", "{done}");
    assert_eq!(fx.ok(&["config", "get", "limits.running"], &[]).trim(), "3");
    let after = toml();
    let again = host.act("rq-yes", config);
    assert_eq!(
        (&again["replayed"], &again["committed_at"]),
        (&json!(true), &done["committed_at"])
    );
    assert_eq!(toml(), after);
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", text(&out));
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[test]
fn c9_c10_finish_and_approve_run_on_the_owner_s_words_and_never_from_the_shell() {
    // Given the workspace is project corral's repository, and A-1 co-worked in scratch cache-fix.
    let fx = Fixture::new();
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.name", "corral test"],
        &["config", "user.email", "corral@test.invalid"],
    ] {
        git(&fx.root, args);
    }
    std::fs::write(fx.root.join(".gitignore"), ".corral/\n.worktrees/\n").unwrap();
    git(&fx.root, &["add", ".gitignore"]);
    git(&fx.root, &["commit", "-q", "-m", "base"]);
    let shell = [("CORRAL_ASSISTANT", "A-1")];
    fx.ok(
        &[
            "assistant",
            "scratch",
            "new",
            "-p",
            "corral",
            "--slug",
            "cache-fix",
        ],
        &shell,
    );
    let scratch = fx.root.join(".worktrees/A-1-cache-fix");
    std::fs::write(scratch.join("cache.txt"), "fixed\n").unwrap();
    git(&scratch, &["add", "cache.txt"]);
    git(&scratch, &["commit", "-q", "-m", "Fix the cache"]);
    let tasks = || fx.store().list().unwrap();
    // When the assistant's shell offers the scratch for landing, it is refused and nothing changes.
    let offered = fx.run(
        &[
            "assistant",
            "scratch",
            "finish",
            "cache-fix",
            "--title",
            "Fix the cache",
        ],
        &shell,
    );
    assert_eq!(offered.status.code(), Some(2), "{}", text(&offered));
    assert!(
        text(&offered)
            .contains("assistant_refused: corral assistant scratch finish needs the owner"),
        "{}",
        text(&offered)
    );
    assert_eq!(tasks().len(), 1);
    // When the owner only says "ship something like this", a guessed finish names no scratch or title.
    let mut host = Host::bind(&fx);
    let vague = host.journal("ship something like this", None);
    let guess = host.act("rq-guess", json!({"kind": "finish", "input": vague}));
    assert_eq!(guess["result"]["code"], "bad_request", "{guess}");
    let untitled = host.act(
        "rq-untitled",
        json!({"kind": "finish", "target": "cache-fix", "input": vague}),
    );
    assert_eq!(untitled["result"]["outcome"], "refused", "{untitled}");
    assert_eq!(tasks().len(), 1);
    // When the owner says to finish it as a fix for the cache with no review, it finishes at once.
    let words = "finish this as a fix for the cache, no review";
    let input = host.journal(words, None);
    let finish = json!({"kind": "finish", "target": "cache-fix",
        "args": ["--title", "Fix the cache", "--review", "none"], "input": input});
    let finished = host.act("rq-finish", finish);
    assert_eq!(
        (
            finished["state"].as_str(),
            finished["result"]["outcome"].as_str()
        ),
        (Some("committed"), Some("executed")),
        "{finished}"
    );
    // Then T-002 is a Review task of origin assistant, label cowork, session A-1, run R1 on the scratch.
    let shown: Value = serde_json::from_str(&fx.ok(&["show", "T-002", "--json"], &[])).unwrap();
    assert_eq!(shown["status"], "review");
    assert_eq!(shown["origin"], "assistant");
    assert_eq!(shown["labels"], json!(["cowork"]));
    assert_eq!(
        shown["cowork"],
        json!({"session": "A-1", "scratch": "cache-fix", "run": "R1", "review": "none"})
    );
    assert_eq!(shown["runs"][0]["branch"], "corral/A-1/cache-fix");
    let task = fx.store().get("T-002").unwrap();
    assert!(task.body.contains(words), "{}", task.body);
    let finished_event = fx
        .events()
        .into_iter()
        .find(|event| event["kind"] == "cowork_finished")
        .unwrap();
    assert_eq!(
        (
            finished_event["actor"].as_str(),
            finished_event["via"].as_str()
        ),
        (Some("owner"), Some("assistant:A-1"))
    );
    assert_eq!(
        finished_event["detail"]["owner_authority"]["owner_message"],
        words
    );
    // When the owner later says "approve T-002", it lands through the normal approve path.
    let approve = host.journal("approve T-002", None);
    let landed = host.act(
        "rq-approve",
        json!({"kind": "approve", "target": "T-002", "input": approve}),
    );
    assert_eq!(landed["result"]["outcome"], "executed", "{landed}");
    let head = git(&fx.root, &["rev-parse", "main"]);
    assert_eq!(
        git(&fx.root, &["log", "-1", "--format=%s", "main"]),
        "Fix the cache"
    );
    let task = fx.store().get("T-002").unwrap();
    assert_eq!(task.status, corral_core::Status::Done);
    // And A-1's record lists the landed task and commits.
    let session = fx.store().get_assistant("A-1").unwrap();
    assert_eq!(session.landed.len(), 1);
    assert_eq!(
        (session.landed[0].task.as_str(), &session.landed[0].commits),
        ("T-002", &vec![head])
    );
    assert!(session.scratch.is_empty());
}
