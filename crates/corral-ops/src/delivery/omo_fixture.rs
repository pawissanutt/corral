//! Test-only OmO world: a fake `omo` executable replaying captured `--json` results, a fake
//! herdr reporting the session's process as the pane foreground, and a gateway transcript.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use corral_core::inbox::{DeliveryMode, Recipient};
use corral_core::{Run, Status, Store, Task};
use serde_json::{Value, json};

use super::adapters::omo::OmoAdapter;
use crate::Ctx;

pub(crate) const STARTED: &str = "2026-10-19T00:00:00Z";

const FAKE_OMO: &str = r#"#!/usr/bin/env python3
import json, os, sys
state = os.path.join(os.path.dirname(os.path.abspath(__file__)), "state")
args = sys.argv[1:]
if args[:2] == ["thread", "--help"]:
    print("usage: omo thread <list|send|read> [options] [--json]")
    print("  send      <target> <text> [--mode auto|steer|follow_up] [--idempotency-key <k>]")
    sys.exit(0)
if args[:2] == ["thread", "list"]:
    if os.path.exists(os.path.join(state, "list-fails")):
        fixtures = open(os.path.join(state, "fixtures")).read().strip()
        sys.stdout.write(open(os.path.join(fixtures, "list-host-unavailable.exit3.json")).read())
        sys.exit(3)
    sys.stdout.write(open(os.path.join(state, "list.json")).read())
    sys.exit(0)
if args[:2] == ["thread", "send"]:
    with open(os.path.join(state, "sends.jsonl"), "a") as log:
        log.write(json.dumps(args) + "\n")
    script = os.path.join(state, "script")
    steps = open(script).read().splitlines() if os.path.exists(script) else []
    step = steps[0] if steps else "0 send-idle.exit0.json 1"
    open(script, "w").write("\n".join(steps[1:]))
    code, fixture, write = step.split()
    fixtures = open(os.path.join(state, "fixtures")).read().strip()
    result = "not json" if fixture == "-" else open(os.path.join(fixtures, fixture)).read()
    if write == "1":
        text = args[args.index("--") + 2]
        entry = {"type": "custom_message", "customType": "session_control_delivery",
                 "content": "[OMO_GATEWAY v=1 source=external]\nRelayed by the gateway.\n" + json.dumps(text),
                 "display": True, "id": "entry", "timestamp": "2026-10-19T00:00:05Z",
                 "details": {"delivery_id": json.loads(result)["delivery_id"], "source": "session_control",
                             "deliverAs": "followUp", "sender": {"kind": "command_line", "user": "test"},
                             "display_text": text}}
        with open(open(os.path.join(state, "transcript")).read().strip(), "a") as session:
            session.write(json.dumps(entry) + "\n")
    sys.stdout.write(result)
    sys.exit(int(code))
sys.exit(2)
"#;

const FAKE_HERDR: &str = r#"#!/bin/sh
[ "$1" != --session ] || shift 2
case "$1 $2" in
"pane process-info") printf '%s\n' '{"result":{"process_info":{"shell_pid":100,"foreground_processes":[{"pid":4242,"argv":["omo"]}]}}}' ;;
"pane get") printf '%s\n' '{"result":{"pane":{"pane_id":"w:p","workspace_id":"w","agent":"omo","agent_status":"idle"}}}' ;;
*) printf '%s\n' "$*" >> "$(dirname "$0")/terminal-writes"
   printf '%s\n' '{"error":{"code":"unexpected","message":"inbox delivery must not touch the composer"}}'
   exit 1 ;;
esac
"#;

pub(crate) struct OmoWorld {
    pub temp: tempfile::TempDir,
    pub ctx: Ctx,
    pub task: Task,
    pub worktree: PathBuf,
    pub transcript: PathBuf,
    pub planner: bool,
}

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

impl OmoWorld {
    pub fn new(status: Status) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let worktree = temp.path().join("app");
        std::fs::create_dir(&worktree).unwrap();
        let agent = temp.path().join("agent");
        let socket = agent.join("rpc/tui/t.sock");
        let registry = agent.join("rpc-host-daemon/endpoint");
        std::fs::create_dir_all(registry.join("generations/instance")).unwrap();
        for (path, value) in [
            (
                registry.join("endpoint.json"),
                json!({"endpoint_kind":"tui","socket":socket}),
            ),
            (registry.join("host.pid"), json!({"instance_id":"instance"})),
            (
                registry.join("generations/instance/host.pid"),
                json!({"pid":4242,"instance_id":"instance","socket":socket}),
            ),
        ] {
            std::fs::write(path, value.to_string()).unwrap();
        }
        let transcript = temp.path().join("session.jsonl");
        std::fs::write(
            &transcript,
            format!(
                "{}\n",
                json!({"type":"session","id":"S1","cwd":worktree,"timestamp":STARTED})
            ),
        )
        .unwrap();
        let bin = temp.path().join("bin");
        let state = bin.join("state");
        std::fs::create_dir_all(&state).unwrap();
        executable(&bin.join("omo"), FAKE_OMO);
        executable(&temp.path().join("herdr"), FAKE_HERDR);
        std::fs::write(
            state.join("fixtures"),
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/delivery/adapters/omo_fixtures"
            ),
        )
        .unwrap();
        std::fs::write(state.join("transcript"), transcript.display().to_string()).unwrap();
        let mut store = Store::init(temp.path()).unwrap();
        store.config.sync.omo_agent_dir = Some(agent);
        let ctx = Ctx {
            store,
            herdr: corral_herdr::Herdr::with_bin(
                temp.path().join("herdr"),
                Some("private-inbox-fixture".into()),
            ),
            resource_probe: crate::resources::FixedResourceProbe::arc(),
        };
        let task = ctx.store.create("Inbox run", None, status, "").unwrap();
        let mut run: Run = serde_json::from_value(json!({
            "id":"R1", "agent":"omo", "harness":"omo", "worktree":worktree,
            "branch":"corral/T-fixture", "base":"main", "base_sha":"0", "started":STARTED,
            "herdr":{"workspace_id":"w","pane_id":"w:p","socket":ctx.herdr.socket_path().unwrap()}
        }))
        .unwrap();
        run.delivery = DeliveryMode::Inbox;
        let task = ctx
            .store
            .update(&task.id, |task| {
                task.runs.push(run);
                Ok(())
            })
            .unwrap();
        let world = Self {
            temp,
            ctx,
            task,
            worktree,
            transcript,
            planner: false,
        };
        world.set_list(&[world.row("S1", "2026-10-19T00:00:01Z")]);
        world
    }

    /// The same world addressed to a live inbox planner in the project directory instead.
    pub fn planner(status: Status) -> Self {
        let mut world = Self::new(status);
        let project: corral_core::Project =
            serde_json::from_value(json!({"path": world.worktree})).unwrap();
        world
            .ctx
            .store
            .config
            .projects
            .insert("app".into(), project);
        world.ctx.store.save_config().unwrap();
        let socket = world.ctx.herdr.socket_path().unwrap();
        world.task = world
            .ctx
            .store
            .update(&world.task.id, |task| {
                task.project = Some("app".into());
                task.runs[0].outcome = Some(corral_core::RunOutcome::Done);
                let mut run: corral_core::PlanRun = serde_json::from_value(json!({
                    "id":"D1", "profile":"omo", "harness":"omo",
                    "spec_revision":crate::plan_spec_revision(task)?,
                    "started":STARTED, "state":"planning", "output_dir":"planner", "rounds":0,
                    "herdr":{"workspace_id":"w","pane_id":"w:p","socket":socket}
                }))?;
                run.delivery = DeliveryMode::Inbox;
                task.plan_runs.push(run);
                Ok(())
            })
            .unwrap();
        world.planner = true;
        world
    }

    pub fn recipient(&self) -> Recipient {
        if self.planner {
            return Recipient::PlanRun {
                task: self.task.id.clone(),
                plan_run: "D1".into(),
            };
        }
        Recipient::Run {
            task: self.task.id.clone(),
            run: "R1".into(),
        }
    }

    pub fn pending(&self) -> Option<String> {
        let task = self.ctx.store.get(&self.task.id).unwrap();
        if self.planner {
            task.plan_runs[0].pending_reply.clone()
        } else {
            task.runs[0].pending_reply.clone()
        }
    }

    /// The gateway writing envelope `number`'s typed entry later (a late arrival).
    pub fn deliver_entry(&self, number: u64, delivery_id: &str) {
        let inbox = corral_core::inbox::Inbox::new(&self.ctx.store, self.recipient()).unwrap();
        let text = inbox.payload(&inbox.envelope(number).unwrap()).unwrap();
        let entry = json!({"type":"custom_message","customType":"session_control_delivery",
            "content":format!("[OMO_GATEWAY v=1]\nRelayed.\n{}", json!(text)),
            "details":{"delivery_id":delivery_id,"source":"session_control","display_text":text}});
        let mut transcript = std::fs::read_to_string(&self.transcript).unwrap();
        transcript.push_str(&format!("{entry}\n"));
        std::fs::write(&self.transcript, transcript).unwrap();
    }

    pub fn adapter(&self) -> OmoAdapter {
        OmoAdapter::with_program(self.temp.path().join("bin/omo"))
    }

    fn state(&self) -> PathBuf {
        self.temp.path().join("bin/state")
    }

    /// A captured `thread list` row rebound to this world's worktree, socket and transcript.
    pub fn row(&self, session: &str, created_at: &str) -> Value {
        let mut row: Value =
            serde_json::from_str::<Value>(include_str!("adapters/omo_fixtures/thread-list.json"))
                .unwrap()[0]
                .clone();
        let socket = self.temp.path().join("agent/rpc/tui/t.sock");
        for (key, value) in [
            ("sessionId", json!(session)),
            ("durableSessionId", json!(session)),
            ("thread_id", json!(session)),
            ("sessionPath", json!(self.transcript)),
            ("session_file", json!(self.transcript)),
            ("cwd", json!(self.worktree)),
            ("socket", json!(socket)),
            ("created_at", json!(created_at)),
        ] {
            row[key] = value;
        }
        row
    }

    /// `thread list` answers with the captured exit-3 `host_unavailable` result.
    pub fn list_fails(&self) {
        std::fs::write(self.state().join("list-fails"), "").unwrap();
    }

    pub fn set_list(&self, rows: &[Value]) {
        std::fs::write(self.state().join("list.json"), json!(rows).to_string()).unwrap();
    }

    /// `<exit> <fixture file|-> <write transcript entry 0|1>` per send.
    pub fn script(&self, steps: &[&str]) {
        std::fs::write(self.state().join("script"), steps.join("\n")).unwrap();
    }

    pub fn sends(&self) -> Vec<Vec<String>> {
        std::fs::read_to_string(self.state().join("sends.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    pub fn entries(&self) -> usize {
        std::fs::read_to_string(&self.transcript)
            .unwrap()
            .lines()
            .filter(|l| l.contains("session_control_delivery"))
            .count()
    }

    pub fn terminal_writes(&self) -> String {
        std::fs::read_to_string(self.temp.path().join("terminal-writes")).unwrap_or_default()
    }

    pub fn events(&self, kind: &str) -> Vec<Value> {
        self.ctx
            .store
            .events_shared()
            .unwrap()
            .iter()
            .filter(|e| e.kind == kind)
            .map(|e| e.detail.clone())
            .collect()
    }
}
