//! In-memory coordination: no store or socket I/O while the state mutex is held.
use std::sync::atomic::AtomicUsize;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use corral_ops::boardview::Projection;

use corral_protocol::{
    DaemonState, Health, LoopFacts, NudgeAck, PmState, RuntimeHealth, RuntimeState,
};

pub(crate) struct Shared {
    pub state: Mutex<State>,
    pub changed: Condvar,
    pub projection: Mutex<Option<Projection>>,
    pub board_subscribers: AtomicUsize,
    pub health_topic: Mutex<(u64, Health)>,
    pub reader: Mutex<Option<std::sync::mpsc::Sender<crate::reader::Job>>>,
    pub attention: Mutex<Option<(u64, corral_ops::attention::AttentionBoard)>>,
}

pub(crate) struct State {
    pub stop: bool,
    pub closed: bool,
    pub upgrading: bool,
    pub pending: bool,
    pub started_seq: u64,
    pub projection_wake: u64,
    pub completed_seq: u64,
    pub workers: usize,
    pub started: Option<Instant>,
    pub started_at: Option<String>,
    pub finished: Option<Instant>,
    pub finished_at: Option<String>,
    pub elapsed: Option<Duration>,
    pub cpu: Duration,
    pub last_error: Option<String>,
    pub server_error: Option<String>,
    pub failures: u8,
    pub last_success: Instant,
    pub foreign: bool,
    pub stores_percent: u64,
    pub stores_pressed: bool,
    pub locks: Vec<String>,
    pub runtime: RuntimeState,
    pub pm: PmState,
    pub supervised: bool,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            projection: Mutex::new(None),
            board_subscribers: AtomicUsize::new(0),
            health_topic: Mutex::new((0, Health::down())),
            reader: Mutex::new(None),
            attention: Mutex::new(None),
            state: Mutex::new(State {
                stop: false,
                closed: false,
                upgrading: false,
                pending: false,
                started_seq: 0,
                completed_seq: 0,
                projection_wake: 0,
                workers: 0,
                started: None,
                started_at: None,
                finished: None,
                finished_at: None,
                elapsed: None,
                cpu: Duration::ZERO,
                last_error: None,
                server_error: None,
                failures: 0,
                last_success: Instant::now(),
                foreign: false,
                stores_percent: 0,
                stores_pressed: false,
                locks: Vec::new(),
                runtime: RuntimeState::Unreachable,
                pm: PmState::Absent,
                supervised: supervised(),
            }),
            changed: Condvar::new(),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub fn projection(&self) -> MutexGuard<'_, Option<Projection>> {
        self.projection
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    pub fn epoch(&self) -> u64 {
        self.projection()
            .as_ref()
            .map_or(0, |projection| projection.epoch)
    }

    pub fn wait_workers(&self, timeout: Duration) {
        drop(
            self.changed
                .wait_timeout_while(self.lock(), timeout, |state| state.workers != 0)
                .unwrap_or_else(|error| error.into_inner()),
        );
    }
    pub fn begin(&self) -> bool {
        let mut state = self.lock();
        if state.stop {
            return false;
        }
        state.pending = false;
        state.started_seq = state.started_seq.saturating_add(1);
        state.started = Some(Instant::now());
        state.started_at = Some(jiff::Timestamp::now().to_string());
        true
    }

    pub fn finish(&self, cpu: Duration, error: Option<String>) {
        let mut state = self.lock();
        state.elapsed = state.started.map(|started| started.elapsed());
        state.started = None;
        state.finished = Some(Instant::now());
        state.finished_at = Some(jiff::Timestamp::now().to_string());
        state.completed_seq = state.started_seq;
        state.cpu = cpu;
        state.failures = if error.is_some() {
            state.failures.saturating_add(1)
        } else {
            0
        };
        if error.is_none() {
            state.last_success = Instant::now();
        }
        state.last_error = error;
        state.projection_wake = state.projection_wake.wrapping_add(1);
        self.changed.notify_all();
    }

    pub fn request_nudge(&self) -> u64 {
        let mut state = self.lock();
        let target = state.started_seq.saturating_add(1);
        if !state.stop {
            state.pending = true;
            state.projection_wake = state.projection_wake.wrapping_add(1);
        }
        self.changed.notify_all();
        target
    }

    pub fn wait_nudge(&self, target: u64, wait: Option<Duration>) -> NudgeAck {
        let mut state = self.lock();
        if let Some(wait) = wait {
            let (next, _) = self
                .changed
                .wait_timeout_while(state, wait, |state| {
                    !state.stop && state.completed_seq < target
                })
                .unwrap_or_else(|error| error.into_inner());
            state = next;
        }
        NudgeAck {
            pass_seq: state.completed_seq,
            completed: wait.is_some() && state.completed_seq >= target,
        }
    }

    pub fn nudge(&self, wait: Option<Duration>) -> NudgeAck {
        self.wait_nudge(self.request_nudge(), wait)
    }

    pub fn fail_server(&self, error: String) {
        let mut state = self.lock();
        state.server_error.get_or_insert(error);
        state.stop = true;
        self.changed.notify_all();
    }

    pub fn stop(&self, upgrading: bool) {
        let mut state = self.lock();
        state.stop = true;
        state.upgrading |= upgrading;
        self.changed.notify_all();
    }

    pub fn close(&self) {
        let mut state = self.lock();
        state.stop = true;
        state.closed = true;
        self.changed.notify_all();
    }

    pub fn wait_closed(&self, timeout: Duration) -> bool {
        let (state, _) = self
            .changed
            .wait_timeout_while(self.lock(), timeout, |state| !state.closed)
            .unwrap_or_else(|error| error.into_inner());
        state.closed
    }

    pub fn wait(&self, duration: Duration, nudges: bool) {
        let state = self.lock();
        drop(
            self.changed
                .wait_timeout_while(state, duration, |state| {
                    !state.stop && (!nudges || !state.pending)
                })
                .unwrap_or_else(|error| error.into_inner()),
        );
    }

    pub fn health(&self) -> Health {
        let state = self.lock();
        let mut reasons = Vec::new();
        if !state.supervised {
            reasons.push("unsupervised".into());
        }
        if state.foreign {
            reasons.push("foreign runtime".into());
        }
        if state.stores_pressed {
            reasons.push(format!("pm stores {}%", state.stores_percent));
        }
        let age = state.started.map(|start| start.elapsed());
        if age.is_some_and(|age| age > Duration::from_secs(300)) || state.failures >= 3 {
            let age = if state.failures >= 3 {
                state.last_success.elapsed()
            } else {
                age.unwrap_or_default()
            };
            reasons.push(format!("sync stalled {}s", age.as_secs()));
        }
        reasons.extend(state.locks.iter().cloned());
        if state.upgrading {
            reasons.push("restarting for upgrade".into());
        }
        Health {
            daemon: if reasons.is_empty() {
                DaemonState::Healthy
            } else {
                DaemonState::Degraded
            },
            reasons,
            runtimes: vec![RuntimeHealth {
                kind: "herdr-tui".into(),
                state: state.runtime,
            }],
            pm: state.pm,
            loop_facts: LoopFacts {
                pass_seq: state.completed_seq,
                last_pass_at: state.finished_at.clone(),
                last_pass_ms: state
                    .elapsed
                    .map(|elapsed| elapsed.as_millis().min(u64::MAX as u128) as u64),
                age_secs: state.finished.map(|end| end.elapsed().as_secs()),
            },
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }

    pub fn bye_reason(&self) -> &'static str {
        if self.lock().upgrading {
            "restarting for upgrade"
        } else {
            "daemon stopped"
        }
    }
}

fn supervised() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("INVOCATION_ID").is_some()
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("XPC_SERVICE_NAME").is_some_and(|value| value != "0")
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        false
    }
}
