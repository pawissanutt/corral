//! Test fixture shared by the recovery tests (T-252 recovery): a git worktree, a fake herdr that
//! logs every call, a pi-native worker run bound to a real stand-in host process, a scripted
//! bridge socket in the generation's runtime directory, and the gate executor child process of
//! op-receipts, which the tests release or SIGKILL at its failpoints.
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use corral_core::agent::{ContainerRef, RequestBinding, RequestOrigin, paths};
use corral_core::inbox::Recipient;
use corral_core::{Event, Status, Store, Task};
use corral_herdr::Herdr;
use serde_json::{Value, json};

use crate::Ctx;
use crate::agent::admission::BindTarget;
use crate::agent::effects::process_start;
use crate::agent::launch::{
    BOOTSTRAP_FILE, SOCKET_FILE, ensure_private_dir, generation_dir, runtime_root,
};
use crate::agent::test_support as ts;

/// The session ID of tools/pi-probe/fixtures/session-dangling.jsonl.
pub(crate) const SESSION: &str = "01a167ce-69e8-72a2-ae3c-efc763d16640";
pub(crate) const CAP: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const DEADLINE: Duration = Duration::from_secs(120);

const FAKE: &str = r#"#!/usr/bin/env bash
cd "$(dirname "$0")" || exit 1
printf '%s\n' "$*" >> calls.log
if [ "${1:-}" = "--session" ]; then shift 2; fi
ok() { printf '{"id":"fake","result":%s}\n' "$1"; }
case "$1 $2" in
  "workspace create"|"tab create")
    n=1
    while [ -f "created-$n" ]; do n=$((n+1)); done
    touch "created-$n" "w1:p$n"
    ok "{\"workspace\":{\"workspace_id\":\"w1\"},\"root_pane\":{\"workspace_id\":\"w1\",\"tab_id\":\"w1:t$n\",\"pane_id\":\"w1:p$n\"}}" ;;
  "workspace list") ok '{"workspaces":[]}' ;;
  "pane list")
    list=""
    for p in w*:p*; do [ -f "$p" ] && list="$list${list:+,}{\"pane_id\":\"$p\"}"; done
    ok "{\"panes\":[$list]}" ;;
  "pane get")
    if [ -f "$3" ]; then ok "{\"pane\":{\"pane_id\":\"$3\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\"}}"; else ok '{}'; fi ;;
  "pane read") ok '{"text":"pane"}' ;;
  "pane process-info") ok '{"process_info":{"shell_pid":10,"foreground_processes":[]}}' ;;
  *) ok '{}' ;;
esac
"#;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = crate::test_support::command("git", dir)
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().into()
}

pub(crate) fn ctx_at(root: &Path) -> Ctx {
    Ctx {
        resource_probe: crate::resources::FixedResourceProbe::arc(),
        store: Store::discover(root).unwrap(),
        herdr: Herdr::with_bin(root.join("herdr"), None),
    }
}

pub(crate) struct Fx {
    pub(crate) tmp: tempfile::TempDir,
    pub(crate) ctx: Ctx,
    pub(crate) id: String,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(runtime_root().join(paths::store_id(&self.ctx.store)));
    }
}

pub(crate) fn run_json(id: &str, app: &Path, pane: &str, herdr: &Herdr) -> Value {
    json!({
        "id": id, "agent": "pi", "harness": "pi-native", "model": "gpt-fixture",
        "worktree": app, "branch": "main", "base": "main", "base_sha": git(app, &["rev-parse", "HEAD"]),
        "started": "2026-01-01T00:00:00Z", "delivery": "inbox",
        "herdr": {"workspace_id": "w0", "pane_id": pane, "socket": herdr.socket_path().unwrap()},
    })
}

/// A Running pi-native task in project `corral`; `gate` configures a gate command, and
/// `drain_secs` bounds resume's wait for reservation executors.
pub(crate) fn fixture(gate: bool, drain_secs: u64) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");
    std::fs::create_dir(&app).unwrap();
    git(&app, &["init", "-qb", "main"]);
    for (key, value) in [("user.name", "t"), ("user.email", "t@example.invalid")] {
        git(&app, &["config", key, value]);
    }
    std::fs::write(app.join("tracked"), "initial\n").unwrap();
    git(&app, &["add", "."]);
    git(&app, &["commit", "-qm", "initial"]);
    let bin = tmp.path().join("herdr");
    std::fs::write(&bin, FAKE).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(tmp.path().join("w0:p1"), "").unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.limits.review = 4;
    store.config.limits.count_reviews = false;
    store.config.limits.count_gates = false;
    store.config.dispatch.paused = true;
    store.config.agent.drain_secs = drain_secs;
    let assets = tmp.path().join("assets");
    std::fs::create_dir(&assets).unwrap();
    std::fs::write(assets.join("package.json"), "{\"name\":\"corral-pi\"}\n").unwrap();
    store.config.agent.assets_dir = Some(assets);
    if gate {
        store.config.review.gate = Some(vec!["true".into()]);
    }
    let project = serde_json::from_value(json!({"path": "app"})).unwrap();
    store.config.projects.insert("corral".into(), project);
    store.save_config().unwrap();
    let ctx = ctx_at(tmp.path());
    let task = ctx
        .store
        .create("Pi", Some("corral"), Status::Running, "spec")
        .unwrap();
    let herdr = Herdr::with_bin(bin, None);
    let run: corral_core::Run =
        serde_json::from_value(run_json("R1", &app, "w0:p1", &herdr)).unwrap();
    ctx.store
        .update(&task.id, |t| {
            t.runs.push(run);
            Ok(())
        })
        .unwrap();
    Fx {
        tmp,
        ctx,
        id: task.id,
    }
}

impl Fx {
    pub(crate) fn root(&self) -> &Path {
        self.tmp.path()
    }

    pub(crate) fn app(&self) -> PathBuf {
        self.root().join("app")
    }

    pub(crate) fn task(&self) -> Task {
        self.ctx.store.get(&self.id).unwrap()
    }

    pub(crate) fn worker(&self) -> ContainerRef {
        ContainerRef::Worker {
            task: self.id.clone(),
            run: "R1".into(),
        }
    }

    pub(crate) fn worker_target(&self) -> BindTarget {
        BindTarget::Worker {
            task: self.id.clone(),
            run: "R1".into(),
        }
    }

    /// Lines of the fake herdr's call log that start with `prefix`.
    pub(crate) fn calls(&self, prefix: &str) -> Vec<String> {
        let log = std::fs::read_to_string(self.root().join("calls.log")).unwrap_or_default();
        log.lines()
            .filter(|l| l.starts_with(prefix))
            .map(String::from)
            .collect()
    }

    pub(crate) fn events(&self, kind: &str) -> Vec<Event> {
        let events = self.ctx.store.events().unwrap();
        events.into_iter().filter(|e| e.kind == kind).collect()
    }

    /// Opens a generation of `target` and binds it with hello from `host`, whose real pid and
    /// start time hello records; the session file is `<sessions>/<SESSION>.jsonl`.
    pub(crate) fn bind(&self, target: &BindTarget, pane: &str, host: &HostProc) -> ts::Host {
        let fx = ts::Fx {
            _tmp: tempfile::tempdir().unwrap(),
            ctx: self.ctx.clone(),
        };
        let bound = ts::bind(&fx, target, pane, SESSION);
        let request = crate::agent::event_request_id(bound.identity.generation, 0);
        let payload = json!({
            "ctl_cap_sha256": paths::sha256_hex(bound.ctl_cap.as_bytes()),
            "session_file": session_file(&bound), "pid": host.pid(),
            "start_time": host.start_time(), "pane": pane,
        });
        let input = ts::envelope(&bound, "hello", &request, payload, true);
        ts::committed(&crate::agent::call(&self.ctx, "hello", &input).unwrap());
        bound
    }

    /// Binds the generation resume opened after `previous` with hello from `host`, on the same
    /// Pi session, as the relaunched Bun host would: its nonce comes from resume's bootstrap.
    pub(crate) fn hello_next(
        &self,
        previous: &ts::Host,
        to: &Recipient,
        host: &HostProc,
    ) -> ts::Host {
        let n = previous.identity.generation + 1;
        let dir = generation_dir(&self.ctx.store, to, n).unwrap();
        let bootstrap: Value =
            serde_json::from_slice(&std::fs::read(dir.join(BOOTSTRAP_FILE)).unwrap()).unwrap();
        let mut next = previous.clone();
        next.identity.generation = n;
        next.host_cap = ts::secret(&format!("{SESSION}-host"), n);
        next.ctl_cap = ts::secret(&format!("{SESSION}-ctl"), n);
        next.nonce = bootstrap["bind_nonce"].as_str().unwrap().into();
        let payload = json!({
            "ctl_cap_sha256": paths::sha256_hex(next.ctl_cap.as_bytes()),
            "session_file": session_file(previous), "pid": host.pid(),
            "start_time": host.start_time(), "pane": previous.pane,
        });
        let request = crate::agent::event_request_id(n, 0);
        let input = ts::envelope(&next, "hello", &request, payload, true);
        ts::committed(&crate::agent::call(&self.ctx, "hello", &input).unwrap());
        next
    }

    /// Records the pending review of R1 that review-run would, its reviewer in `pane` since
    /// `at`, so the review watchdog and admission see an open review.
    pub(crate) fn pending_review(&self, pane: &str, at: jiff::Timestamp) {
        let sha = git(&self.app(), &["rev-parse", "HEAD"]);
        let record: corral_core::ReviewRecord = serde_json::from_value(json!({
            "verdict": null, "process_lost": false, "reviewer": "pi", "notes": null,
            "evidence_digest": null, "identity": null,
            "sha": sha, "at": at, "pane": pane, "route": null,
        }))
        .unwrap();
        self.ctx
            .store
            .update(&self.id, |t| {
                let child = t.runs[0].review_runs.last_mut().unwrap();
                child.review = Some(record.clone());
                t.runs[0].review = Some(record);
                Ok(())
            })
            .unwrap();
    }

    /// A request of the bound worker, as the gate executor child rebuilds it.
    pub(crate) fn origin(&self, host: &ts::Host, call: &str, op: &str) -> RequestOrigin {
        RequestOrigin {
            request_id: crate::agent::tool_request_id(SESSION, call),
            op: op.into(),
            payload_sha256: paths::sha256_hex(call.as_bytes()),
            identity: host.identity.clone(),
            binding: RequestBinding {
                container: self.worker(),
                generation: host.identity.generation,
                callback: None,
                settles_turn: None,
            },
        }
    }
}

pub(crate) fn session_file(host: &ts::Host) -> PathBuf {
    host.sessions.join(format!("{SESSION}.jsonl"))
}

/// Writes the bound generation's Pi session file, as the host's session manager would.
pub(crate) fn write_session(host: &ts::Host, text: &str) {
    std::fs::create_dir_all(&host.sessions).unwrap();
    std::fs::write(session_file(host), text).unwrap();
}

/// A Pi session after `since`: one assistant message calling each `(id, name)`, then a tool
/// result for each id in `results`.
pub(crate) fn session(calls: &[(&str, &str)], results: &[&str]) -> String {
    let at = jiff::Timestamp::now().to_string();
    let header = json!({"type": "session", "version": 3, "id": SESSION, "timestamp": at});
    let content: Vec<Value> = calls
        .iter()
        .map(|(id, name)| json!({"type": "toolCall", "id": id, "name": name, "arguments": {}}))
        .collect();
    let assistant = json!({"type": "message", "id": "a1", "timestamp": at,
        "message": {"role": "assistant", "content": content, "stopReason": "toolUse"}});
    let mut lines = vec![header.to_string(), assistant.to_string()];
    for id in results {
        lines.push(
            json!({"type": "message", "id": format!("r-{id}"), "timestamp": at,
                "message": {"role": "toolResult", "toolCallId": id, "content": []}})
            .to_string(),
        );
    }
    lines.join("\n") + "\n"
}

/// bun-reproof's dangling-call session, its entries re-stamped now (after the generation opened).
pub(crate) fn dangling_fixture() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/pi-probe/fixtures/session-dangling.jsonl");
    let text = std::fs::read_to_string(path).unwrap();
    let at = jiff::Timestamp::now().to_string();
    let lines = text.lines().map(|line| {
        let mut entry: Value = serde_json::from_str(line).unwrap();
        entry["timestamp"] = json!(at);
        entry.to_string()
    });
    lines.collect::<Vec<_>>().join("\n") + "\n"
}

/// A real stand-in host: bash blocked on stdin, its argv shaped like the Bun host's or not.
pub(crate) struct HostProc {
    child: Child,
}

impl HostProc {
    pub(crate) fn spawn(like_host: bool) -> HostProc {
        let entry = if like_host {
            "/fixture/src/host/main.ts"
        } else {
            "/fixture/other.ts"
        };
        let child = Command::new("bash")
            .args(["-c", "read -r _", entry, "--launch-fd", "3"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        HostProc { child }
    }

    /// A stand-in host caught inside a tool call: in `dir` it runs `script` (the edit it
    /// writes, or the command whose child keeps running), then blocks until it is killed.
    pub(crate) fn spawn_busy(dir: &Path, script: &str) -> HostProc {
        let script = format!("{script}\necho READY\nread -r _");
        let mut child = Command::new("bash")
            .args([
                "-c",
                &script,
                "/fixture/src/host/main.ts",
                "--launch-fd",
                "3",
            ])
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, ready) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line == "READY" {
                    let _ = tx.send(());
                }
            }
        });
        ready
            .recv_timeout(DEADLINE)
            .expect("the busy host never got READY");
        HostProc { child }
    }

    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn start_time(&self) -> String {
        process_start(self.pid()).unwrap().unwrap()
    }

    /// SIGKILLs and reaps it, so no live process has its pid and start time.
    pub(crate) fn kill(&mut self) {
        let _ = self.child.kill();
        self.child.wait().unwrap();
    }

    /// Reaps it after something else signalled it; true when it exited.
    pub(crate) fn reaped(&mut self) -> bool {
        self.child.try_wait().unwrap().is_some()
    }
}

impl Drop for HostProc {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            self.kill();
        }
    }
}

/// A scripted bridge on generation `n`'s socket: every frame is recorded, then answered by
/// `reply(frame)`; null closes the connection without replying. It serves any number of connections.
pub(crate) struct Bridge {
    pub(crate) frames: mpsc::Receiver<Value>,
    pub(crate) dir: PathBuf,
}

pub(crate) fn bridge(
    store: &Store,
    recipient: &Recipient,
    n: u32,
    cap: Option<&str>,
    reply: impl Fn(&Value) -> Value + Send + Sync + 'static,
) -> Bridge {
    let reply = std::sync::Arc::new(reply);
    let dir = generation_dir(store, recipient, n).unwrap();
    ensure_private_dir(&runtime_root(), &dir).unwrap();
    if let Some(cap) = cap {
        crate::agent::launch::write_ctl_cap(&dir, cap).unwrap();
    }
    let listener = UnixListener::bind(dir.join(SOCKET_FILE)).unwrap();
    let (tx, frames) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else {
                return;
            };
            let (tx, reply) = (tx.clone(), reply.clone());
            std::thread::spawn(move || {
                let mut writer = stream.try_clone().unwrap();
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let frame: Value = serde_json::from_str(&line).unwrap();
                    let mut answer = reply(&frame);
                    if answer.is_null() {
                        return;
                    }
                    answer["id"] = frame["id"].clone();
                    answer["generation"] = frame["generation"].clone();
                    answer["version"] = json!(1);
                    let _ = tx.send(frame);
                    if writeln!(writer, "{answer}").is_err() {
                        return;
                    }
                }
            });
        }
    });
    Bridge { frames, dir }
}

impl Bridge {
    pub(crate) fn commands(&self) -> Vec<Value> {
        self.frames.try_iter().collect()
    }
}

/// A healthy host of any generation with no new events; it admits every input.
pub(crate) fn quiet(frame: &Value) -> Value {
    let generation = frame["generation"].clone();
    match frame["command"].as_str() {
        Some("get_state") => json!({"ok": true, "result": {
            "generation": generation, "session": {"bound_id": SESSION}}}),
        Some("events_since") => {
            let after = frame["args"]["seq"].clone();
            json!({"ok": true, "result": {"generation": generation, "events": [],
                "last": after, "next": after, "more": false}})
        }
        Some("hello") => json!({"ok": true, "result": {
            "protocol": 1, "generation": generation, "bound": true, "session_id": SESSION}}),
        Some("submit_input") => json!({"ok": true, "result": {
            "request_id": frame["id"], "idempotency_key": frame["args"]["idempotency_key"],
            "disposition": "started", "turn": 1, "replayed": false}}),
        _ => json!({"ok": false, "error": {"code": "unknown_command", "reason": "fixture"}}),
    }
}

/// A healthy generation-1 host: its state names the fixture session, and its events are 1..=3.
pub(crate) fn healthy(frame: &Value) -> Value {
    match frame["command"].as_str() {
        Some("get_state") => json!({"ok": true, "result": {
            "generation": frame["generation"], "session": {"bound_id": SESSION}}}),
        Some("events_since") => {
            let after = frame["args"]["seq"].as_u64().unwrap();
            let events: Vec<Value> = (after + 1..=3)
                .map(|seq| json!({"seq": seq, "generation": 1, "kind": "queue"}))
                .collect();
            let next = events.last().map_or(after, |e| e["seq"].as_u64().unwrap());
            json!({"ok": true, "result": {"generation": 1, "events": events, "last": 3,
                "next": next, "more": false}})
        }
        Some("cancel") => json!({"ok": true, "result": {"cancelled": true, "turn": 2}}),
        _ => json!({"ok": false, "error": {"code": "unknown_command", "reason": "fixture"}}),
    }
}

/// The op-receipts gate executor child: `effects::gate` for `origin` in a fresh process armed
/// at comma-separated failpoints, announced on stdout and blocking until released.
pub(crate) struct Exec {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl Exec {
    pub(crate) fn spawn(fx: &Fx, origin: &RequestOrigin, points: &str) -> Exec {
        let spec = json!({"root": fx.root(), "identity": origin.identity,
            "request": origin.request_id, "payload": origin.payload_sha256});
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "agent::effects_tests::gate::gate_executor_child",
                "--ignored",
                "--nocapture",
                "--test-threads",
                "1",
            ])
            .env("CORRAL_T300_EXECUTOR", spec.to_string())
            .env("CORRAL_AGENT_FAILPOINT", points)
            .env_remove("CORRAL_GATE_ATTEMPT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Exec { child, lines }
    }

    pub(crate) fn reached(&self, point: &str) -> &Self {
        let deadline = Instant::now() + DEADLINE;
        let marker = format!("FAILPOINT {point}");
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) if line.contains(&marker) => return self,
                Ok(_) => {}
                Err(error) => panic!("the executor never reached {point}: {error}"),
            }
        }
    }

    pub(crate) fn release(&mut self) -> &mut Self {
        let stdin = self.child.stdin.as_mut().unwrap();
        stdin.write_all(b"go\n").unwrap();
        self
    }

    pub(crate) fn wait(mut self) -> bool {
        self.child.wait().unwrap().success()
    }

    pub(crate) fn kill(mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

impl Drop for Exec {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// The reviewer of a Review task: R1 done, its review child `R1-review1` in pane `w0:p2`.
pub(crate) fn reviewing(fx: &Fx) -> BindTarget {
    let herdr = Herdr::with_bin(fx.root().join("herdr"), None);
    let child: corral_core::Run =
        serde_json::from_value(run_json("R1-review1", &fx.app(), "w0:p2", &herdr)).unwrap();
    std::fs::write(fx.root().join("w0:p2"), "").unwrap();
    fx.ctx
        .store
        .update(&fx.id, |t| {
            t.status = Status::Review;
            t.runs[0].outcome = Some(corral_core::RunOutcome::Done);
            t.runs[0].review_runs.push(child);
            Ok(())
        })
        .unwrap();
    BindTarget::Reviewer {
        task: fx.id.clone(),
        run: "R1".into(),
        review: "R1-review1".into(),
        callback: "g1:c1".into(),
    }
}
