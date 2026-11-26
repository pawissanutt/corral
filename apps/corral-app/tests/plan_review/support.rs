//! The scripted corrald behind the plan review tests, and the app connected to it: the daemon
//! answers what the app reads from the host's World, hands every `session.card.answer` to the
//! test, and answers each `plan_status` read from the World as it stands when it replies, or
//! holds it for the test to release or refuse.

use std::collections::BTreeSet;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use corral_protocol::frame::{Frame, FrameKind, read_frame, write_frame};
use corral_protocol::rpc::Request;
use corral_protocol::session_host::{SessionAction, SessionActionRequest, SessionCardAnswer};
use corral_protocol::session_state::SessionState;
use corral_protocol::{self as wire};
use nsh_core::{
    Points, Size,
    platform::{Idiom, Os},
};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{Headless, MemoryStorage};
use serde_json::{Value, json};

use crate::world::{COMPACT, DEVICE, DUPLICATE, MODEL, SESSION, STORE, World, card, now};

/// Every test here points the process's `CORRAL_SOCKET` and `XDG_STATE_HOME` at its own daemon.
static ENV: Mutex<()> = Mutex::new(());

type Outcome = Result<Value, (&'static str, String)>;

enum In {
    Connected(UnixStream),
    Request(u64, Request),
    Reply(Value, Outcome),
    /// Answers a held `plan_status` read for the card from the World as it is now.
    Status(Value, String),
    Change(Box<dyn FnOnce(&mut World) + Send>),
    /// Changes the World without announcing it: only a status read can tell.
    Quiet(Box<dyn FnOnce(&mut World) + Send>),
    /// Announces a live state derived from the World's, leaving the World as it is.
    Stale(Box<dyn FnOnce(&mut SessionState) + Send>),
    HoldProbes(bool),
    Stop,
}

/// One `session.card.answer` the app sent.
pub struct Answer {
    rpc: Value,
    pub answer: SessionCardAnswer,
}

/// One `plan_status` read the app sent while the daemon held them.
pub struct Probe {
    rpc: Value,
    pub card: String,
}

/// The scripted corrald: its own thread answers the app; the test holds every card answer.
pub struct Daemon {
    to: Sender<In>,
    answers: Receiver<Answer>,
    probes: Receiver<Probe>,
    /// The `plan_status` reads answered as they came.
    probed: Arc<AtomicUsize>,
    root: PathBuf,
    stopping: Arc<AtomicBool>,
    listener: Option<JoinHandle<()>>,
    worker: Option<JoinHandle<()>>,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        let _ = self.to.send(In::Stop);
        let _ = UnixStream::connect(self.root.join("daemon.sock"));
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn send(stream: &mut UnixStream, value: &Value) {
    let frame = Frame {
        kind: FrameKind::Control,
        stream: 0,
        payload: serde_json::to_vec(value).unwrap(),
    };
    // The app may have dropped the connection; it reconnects and is served again.
    let _ = write_frame(stream, &frame);
}

fn notify(stream: &mut UnixStream, method: &str, params: Value) {
    send(
        stream,
        &json!({"jsonrpc": "2.0", "method": method, "params": params}),
    );
}

fn respond(stream: &mut UnixStream, id: &Value, outcome: Outcome) {
    let value = match outcome {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
        }
    };
    send(stream, &value);
}

fn empty_rows(topic: &str, rows: Vec<Value>) -> Value {
    json!({"topic": topic, "epoch": 1, "rows": rows, "left": [],
        "cursor": {"rev": 1, "after": null}, "more": false})
}

fn health() -> Value {
    json!({"daemon": "healthy", "runner_reachable": true, "last_sync_at": null,
        "last_sync_error": null, "pid": 1, "version": "harness"})
}

fn device() -> Value {
    json!({"id": DEVICE, "name": "Harness", "platform": "native", "app_version": "0.1.0",
        "enrolled_at": "2026-11-01T00:00:00Z", "revoked_at": null})
}

fn build_row() -> Value {
    json!({"id": SESSION, "kind": "build", "title": "Plan harness build", "state": "waiting",
        "live_in": "daemon", "profile": null, "focus": null, "pending": 0, "turn": 1,
        "cost_usd": null, "cost_basis": null, "wait": null, "rev": 1, "model": MODEL})
}

fn hello() -> Value {
    json!({
        "proto": {"major": wire::PROTO_MAJOR, "minor": wire::PROTO_MINOR},
        "caps": [
            corral_protocol::device::DEVICE_V1,
            wire::app::AGENTS_V1,
            wire::app::BUILD_V1,
            wire::app::SESSION_V1,
            wire::app::SESSION_STATE_V1,
        ],
        "daemon": {"version": "harness", "store": STORE, "epoch": 1},
        "health": health(),
    })
}

/// The daemon's side of the conversation, owned by its worker thread.
struct Served {
    world: World,
    answers: Sender<Answer>,
    probes: Sender<Probe>,
    probed: Arc<AtomicUsize>,
    hold_probes: bool,
    open: BTreeSet<String>,
}

impl Served {
    fn request(&mut self, stream: &mut UnixStream, request: Request) {
        let id = request.id.clone();
        match request.method.as_str() {
            wire::HELLO => respond(stream, &id, Ok(hello())),
            corral_protocol::device::DEVICE_ATTACH => respond(stream, &id, Ok(device())),
            corral_protocol::device::DEVICE_PAGE => respond(
                stream,
                &id,
                Ok(json!({"devices": [device()], "next": null})),
            ),
            wire::app::APP_SESSION => respond(
                stream,
                &id,
                Ok(
                    json!({"device": device(), "seen": {"through": 0, "at": null},
                    "server_time": now(), "local": true}),
                ),
            ),
            wire::SUB_OPEN => {
                respond(stream, &id, Ok(json!({})));
                let topic = request.params["topic"].as_str().unwrap_or_default();
                let page = match topic {
                    "health" => empty_rows("health", vec![health()]),
                    "assistant" => empty_rows("assistant", vec![build_row()]),
                    "session" => {
                        let scope = request.params["scope"].as_str().unwrap_or_default();
                        self.open.insert(scope.to_owned());
                        self.world.session_page(scope)
                    }
                    other => empty_rows(other, Vec::new()),
                };
                notify(stream, wire::SUB_DELTA, page);
            }
            wire::SUB_CLOSE => {
                if let Some(scope) = request.params["scope"].as_str() {
                    self.open.remove(scope);
                }
                respond(stream, &id, Ok(json!({})));
            }
            wire::SUB_ACK | wire::PING => respond(stream, &id, Ok(json!({}))),
            wire::app::DETAIL_GET => {
                let outcome = serde_json::from_value(request.params)
                    .map_err(|error| ("invalid_params", error.to_string()))
                    .and_then(|get| self.world.detail(get));
                respond(stream, &id, outcome);
            }
            corral_protocol::session_host::SESSION_CARD_ANSWER => {
                let answer = serde_json::from_value(request.params).unwrap();
                let _ = self.answers.send(Answer { rpc: id, answer });
            }
            corral_protocol::session_host::SESSION_ACTION => {
                match serde_json::from_value::<SessionActionRequest>(request.params) {
                    Ok(SessionActionRequest {
                        id: session,
                        action: SessionAction::PlanStatus { card },
                    }) if session == SESSION => {
                        if self.hold_probes {
                            let _ = self.probes.send(Probe { rpc: id, card });
                        } else {
                            self.probed.fetch_add(1, Ordering::SeqCst);
                            respond(stream, &id, Ok(self.world.status(&card)));
                        }
                    }
                    _ => respond(
                        stream,
                        &id,
                        Err(("unavailable", "not served by the plan harness".into())),
                    ),
                }
            }
            _ => respond(
                stream,
                &id,
                Err(("unavailable", "not served by the plan harness".into())),
            ),
        }
    }

    /// The test's reply to a card answer, as corrald relays the host's.
    fn reply(&mut self, mut outcome: Outcome) -> Outcome {
        if outcome
            .as_ref()
            .err()
            .is_some_and(|(_, reason)| reason == DUPLICATE)
        {
            if let Some(plan) = self.world.state.plan_mode.as_ref() {
                outcome = Ok(
                    json!({"status": "waiting", "approval_epoch": plan.approval_epoch,
                    "approval_host": plan.approval_host}),
                );
            }
        }
        if outcome
            .as_ref()
            .ok()
            .and_then(|value| value["status"].as_str())
            == Some("preparing")
        {
            // The host took the answer: the status it reports from now on is this approval's.
            self.world.preparing(Some("compact"));
            if let (Ok(value), Some(plan)) = (&mut outcome, self.world.state.plan_mode.as_ref()) {
                value["approval_epoch"] = json!(plan.approval_epoch);
                value["approval_host"] = json!(plan.approval_host);
            }
        }
        outcome
    }

    fn announce(&mut self, stream: Option<&mut UnixStream>, state: Option<SessionState>) {
        if let Some(stream) = stream
            && self.open.contains(SESSION)
        {
            let page = match state {
                Some(state) => self.world.page_of(SESSION, &state),
                None => self.world.session_page(SESSION),
            };
            notify(stream, wire::SUB_DELTA, page);
        }
    }
}

fn serve(inbox: Receiver<In>, mut served: Served) {
    let mut stream: Option<UnixStream> = None;
    let mut connection = 0u64;
    for message in inbox {
        match message {
            In::Connected(accepted) => {
                connection += 1;
                served.open.clear();
                stream = Some(accepted);
            }
            In::Request(from, request) => {
                if let Some(stream) = stream.as_mut().filter(|_| from == connection) {
                    served.request(stream, request);
                }
            }
            In::Reply(id, outcome) => {
                let outcome = served.reply(outcome);
                if let Some(stream) = stream.as_mut() {
                    respond(stream, &id, outcome);
                }
            }
            In::Status(id, card) => {
                if let Some(stream) = stream.as_mut() {
                    respond(stream, &id, Ok(served.world.status(&card)));
                }
            }
            In::Change(change) => {
                change(&mut served.world);
                served.announce(stream.as_mut(), None);
            }
            In::Quiet(change) => change(&mut served.world),
            In::Stale(change) => {
                let mut state = served.world.state.clone();
                change(&mut state);
                served.announce(stream.as_mut(), Some(state));
            }
            In::HoldProbes(hold) => served.hold_probes = hold,
            In::Stop => break,
        }
    }
}

/// The next message on `inbox`, while the app runs.
fn next<T>(inbox: &Receiver<T>, app: &mut Headless, what: &str) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match inbox.try_recv() {
            Ok(message) => return message,
            Err(TryRecvError::Disconnected) => panic!("the daemon stopped"),
            Err(TryRecvError::Empty) => {}
        }
        assert!(Instant::now() < deadline, "the app sent no {what}");
        app.next_change(Duration::from_millis(50)).unwrap();
    }
}

impl Daemon {
    fn start(root: PathBuf) -> Self {
        let listener = UnixListener::bind(root.join("daemon.sock")).unwrap();
        let (to, inbox) = channel();
        let (answered, answers) = channel();
        let (probing, probes) = channel();
        let probed = Arc::new(AtomicUsize::new(0));
        let accepting = to.clone();
        let stopping = Arc::new(AtomicBool::new(false));
        let listener_stopping = stopping.clone();
        let listener = std::thread::spawn(move || {
            let mut readers = Vec::new();
            for (connection, accepted) in listener.incoming().enumerate() {
                let Ok(accepted) = accepted else { break };
                if listener_stopping.load(Ordering::SeqCst) {
                    break;
                }
                let mut reader = accepted.try_clone().unwrap();
                if accepting.send(In::Connected(accepted)).is_err() {
                    return;
                }
                let requests = accepting.clone();
                let connection = connection as u64 + 1;
                readers.push(std::thread::spawn(move || {
                    while let Ok(Some(frame)) = read_frame(&mut reader) {
                        let Ok(request) = serde_json::from_slice::<Request>(&frame.payload) else {
                            continue;
                        };
                        if requests.send(In::Request(connection, request)).is_err() {
                            return;
                        }
                    }
                }));
            }
            for reader in readers {
                let _ = reader.join();
            }
        });
        let served = Served {
            world: World::new(),
            answers: answered,
            probes: probing,
            probed: probed.clone(),
            hold_probes: false,
            open: BTreeSet::new(),
        };
        let worker = std::thread::spawn(move || serve(inbox, served));
        Daemon {
            to,
            answers,
            probes,
            probed,
            root,
            stopping,
            listener: Some(listener),
            worker: Some(worker),
        }
    }

    /// The next card answer the app sends, while the app runs.
    pub fn answer(&self, app: &mut Headless) -> Answer {
        next(&self.answers, app, "session.card.answer")
    }

    /// Runs the app for `time` and fails if it answers the card meanwhile.
    pub fn no_answer(&self, app: &mut Headless, time: Duration, why: &str) {
        let deadline = Instant::now() + time;
        while Instant::now() < deadline {
            if let Ok(answer) = self.answers.try_recv() {
                panic!("{why}: the app sent {:?}", answer.answer);
            }
            app.next_change(Duration::from_millis(50)).unwrap();
        }
    }

    pub fn reply(&self, answer: &Answer, status: &str) {
        self.to
            .send(In::Reply(
                answer.rpc.clone(),
                Ok(json!({ "status": status })),
            ))
            .unwrap();
    }

    pub fn refuse(&self, answer: &Answer, reason: &str) {
        self.to
            .send(In::Reply(
                answer.rpc.clone(),
                Err(("refused", reason.to_owned())),
            ))
            .unwrap();
    }

    /// Changes the host and announces it as the live session's state and cards.
    pub fn change(&self, change: impl FnOnce(&mut World) + Send + 'static) {
        self.to.send(In::Change(Box::new(change))).unwrap();
    }

    /// Changes the host without a live update: only a status read can see it.
    pub fn quietly(&self, change: impl FnOnce(&mut World) + Send + 'static) {
        self.to.send(In::Quiet(Box::new(change))).unwrap();
    }

    /// Delivers a live state the host no longer holds, as a frame queued before it moved on.
    pub fn stale(&self, change: impl FnOnce(&mut SessionState) + Send + 'static) {
        self.to.send(In::Stale(Box::new(change))).unwrap();
    }

    /// While held, each `plan_status` read waits for `status` or `refuse_status`; otherwise each
    /// is answered from the World at once.
    pub fn hold_probes(&self, hold: bool) {
        self.to.send(In::HoldProbes(hold)).unwrap();
    }

    /// The next held `plan_status` read the app sends, while the app runs.
    pub fn probe(&self, app: &mut Headless) -> Probe {
        next(&self.probes, app, "plan_status read")
    }

    /// Answers a held read from the host as it is when the answer goes out.
    pub fn status(&self, probe: &Probe) {
        self.to
            .send(In::Status(probe.rpc.clone(), probe.card.clone()))
            .unwrap();
    }

    pub fn refuse_status(&self, probe: &Probe, reason: &str) {
        self.to
            .send(In::Reply(
                probe.rpc.clone(),
                Err(("unavailable", reason.to_owned())),
            ))
            .unwrap();
    }

    /// How many `plan_status` reads were answered as they came.
    pub fn probed(&self) -> usize {
        self.probed.load(Ordering::SeqCst)
    }

    /// Runs the app until a read is answered as it came after `since` were.
    pub fn probed_since(&self, app: &mut Headless, since: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.probed() <= since {
            assert!(Instant::now() < deadline, "the app read no plan status");
            app.next_change(Duration::from_millis(50)).unwrap();
        }
    }
}

/// A desktop wide enough for the session page and the side pane beside it.
fn desktop() -> EngineConfig {
    let mut config = EngineConfig::new(Root::of::<nsh_app_corral::App>());
    config.viewport = Size::new(Points(1600.0), Points(1000.0));
    config.environment.os = Os::Linux;
    config.environment.idiom = Idiom::Desktop;
    config.storage = Some(MemoryStorage::new([(
        "corral.destination".to_owned(),
        "sessions".to_owned(),
    )]));
    config
}

/// The app on Sessions, enrolled with the scripted daemon, with the build's page open.
pub fn connected() -> (MutexGuard<'static, ()>, Daemon, Headless) {
    let guard = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let root = std::env::temp_dir().join(format!(
        "corral-plan-{}-{}",
        std::process::id(),
        jiff::Timestamp::now().as_nanosecond()
    ));
    let state = root.join("state");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(state.join("corral-app"))
        .unwrap();
    let devices = json!({"version": 1, "devices": {STORE: DEVICE}});
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(state.join("corral-app/devices.json"))
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(devices.to_string().as_bytes())
        })
        .unwrap();
    let daemon = Daemon::start(root.clone());
    // SAFETY: ENV serializes the tests that set these, and the app reads them only as it starts.
    unsafe {
        std::env::set_var("CORRAL_SOCKET", root.join("daemon.sock"));
        std::env::set_var("XDG_STATE_HOME", &state);
    }
    let mut app = Headless::start(desktop()).unwrap();
    let row = format!("id:agent-{SESSION}");
    app.wait_for(&row).unwrap();
    app.press(&row).unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    while !shows(&app, "id:page-back") && Instant::now() < deadline {
        app.next_change(Duration::from_millis(100)).unwrap();
    }
    if !shows(&app, "id:page-back") {
        app.press(&row).unwrap();
    }
    app.wait_for("id:page-back").unwrap();
    (guard, daemon, app)
}

/// A held review still draws the option but it must not have a press target.
pub fn assert_muted(app: &Headless, selector: &str) {
    assert!(shows(app, selector), "the option must remain visible");
    assert_eq!(app.screen().press_target(selector).unwrap(), None);
}

/// Runs the app until the option can be pressed again.
pub fn wait_armed(app: &mut Headless, selector: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.screen().press_target(selector).ok().flatten().is_none() {
        assert!(
            Instant::now() < deadline,
            "{selector} never took a press again"
        );
        app.next_change(Duration::from_millis(50)).unwrap();
    }
}

pub fn shows(app: &Headless, selector: &str) -> bool {
    app.screen().matches(selector) == Ok(true)
}

/// Where the review is answered: the session's page, or its tab in the side pane.
pub struct Entry {
    within: &'static str,
}

impl Entry {
    pub const PAGE: Entry = Entry { within: "" };
    pub const PANE: Entry = Entry {
        within: "id:dock-pane >> ",
    };

    /// The entry the scenario answers from: the page, or the pane opened beside it.
    pub fn of(pane: bool, app: &mut Headless) -> &'static Entry {
        if !pane {
            return &Entry::PAGE;
        }
        app.press("id:btn-dock-side").unwrap();
        app.wait_for(&Entry::PANE.at("id:dock-transcript")).unwrap();
        &Entry::PANE
    }

    pub fn at(&self, selector: &str) -> String {
        format!("{}{selector}", self.within)
    }

    pub fn option(&self, n: u32, option: usize) -> String {
        self.at(&format!("id:opt-{}-{option}", card(n)))
    }

    /// Opens plan `n`'s review and waits until the whole plan is read, which Execute needs.
    pub fn open(&self, app: &mut Headless, n: u32) {
        let line = self.at(&format!("id:dock-{}", card(n)));
        app.wait_for(&line).unwrap();
        app.press(&line).unwrap();
        app.wait_for(&self.at("id:plan-review")).unwrap();
        app.wait_for(&self.at("text:The whole plan was read."))
            .unwrap();
        app.wait_for(&self.option(n, COMPACT)).unwrap();
    }

    /// The review of plan `n` has closed where it was answered.
    pub fn closed(&self, app: &mut Headless, n: u32) {
        let gone = Duration::from_secs(5);
        app.wait_for_gone(&self.at(&format!("id:dock-{}", card(n))), gone)
            .unwrap();
        app.wait_for_gone(&self.at("id:plan-review"), gone).unwrap();
        app.wait_for_gone(&self.at("id:plan-preparing"), gone)
            .unwrap();
    }

    /// Dismisses whatever an earlier scenario left in the head, so a banner read next is new.
    pub fn clear_banners(&self, app: &mut Headless) {
        let dismiss = self.at("id:btn-dismiss-notice");
        for _ in 0..8 {
            if !shows(app, &dismiss) {
                return;
            }
            app.press(&dismiss).unwrap();
            app.next_change(Duration::from_millis(100)).unwrap();
        }
    }

    pub fn banner(&self, words: &str) -> String {
        self.at(&format!("text:/{words}/"))
    }
}

pub fn chose(answer: &Answer, option: &str, n: u32) {
    assert_eq!(answer.answer.id, SESSION);
    assert_eq!(answer.answer.card, card(n));
    let options: Vec<&str> = answer
        .answer
        .answers
        .iter()
        .flat_map(|given| given.options.iter().map(String::as_str))
        .collect();
    assert_eq!(options, [option]);
    assert_eq!(answer.answer.expected_model.as_deref(), Some(MODEL));
}
