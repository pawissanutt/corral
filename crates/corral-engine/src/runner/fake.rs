use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result};

use super::{LaunchSpec, Origin, Runner, SessionRef, SessionStatus};
use crate::error::refuse;

type StartHook = Arc<dyn Fn(&LaunchSpec) -> Result<()> + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendRecord {
    pub session: SessionRef,
    pub text: String,
    pub origin: Origin,
}

/// A cloneable fake with shared history. Hooks run outside the state lock, so an
/// agent simulation may call back into the engine (and this runner) on launch.
#[derive(Clone, Default)]
pub struct FakeRunner {
    state: Arc<Mutex<State>>,
}

struct State {
    next_id: u64,
    default_status: SessionStatus,
    sessions: HashMap<SessionRef, FakeSession>,
    starts: Vec<LaunchSpec>,
    sends: Vec<SendRecord>,
    stops: Vec<SessionRef>,
    focuses: Vec<SessionRef>,
    start_hook: Option<StartHook>,
}

#[derive(Default)]
struct FakeSession {
    status: Option<SessionStatus>,
    queue: VecDeque<SessionStatus>,
    refuses_send: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            next_id: 1,
            default_status: SessionStatus::Working,
            sessions: HashMap::new(),
            starts: Vec::new(),
            sends: Vec::new(),
            stops: Vec::new(),
            focuses: Vec::new(),
            start_hook: None,
        }
    }
}

impl FakeRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Used for live sessions without an explicit status or queued response.
    pub fn set_default_status(&self, status: SessionStatus) {
        self.lock().default_status = status;
    }

    /// Replace a session's status and discard its remaining scripted responses.
    pub fn set_status(&self, session: &SessionRef, status: SessionStatus) {
        let mut state = self.lock();
        let entry = state.sessions.entry(session.clone()).or_default();
        entry.status = Some(status);
        entry.queue.clear();
    }

    /// Each status call consumes one response, then falls back to the set/default status.
    pub fn queue_status(&self, session: &SessionRef, status: SessionStatus) {
        self.lock()
            .sessions
            .entry(session.clone())
            .or_default()
            .queue
            .push_back(status);
    }

    /// Simulate a live session that cannot receive a prompt, like ProcessRunner.
    pub fn refuse_send(&self, session: &SessionRef) {
        self.lock()
            .sessions
            .entry(session.clone())
            .or_default()
            .refuses_send = true;
    }

    pub fn set_start_hook<F>(&self, hook: F)
    where
        F: Fn(&LaunchSpec) -> Result<()> + Send + Sync + 'static,
    {
        self.lock().start_hook = Some(Arc::new(hook));
    }

    pub fn clear_start_hook(&self) {
        self.lock().start_hook = None;
    }

    pub fn starts(&self) -> Vec<LaunchSpec> {
        self.lock().starts.clone()
    }

    pub fn sends(&self) -> Vec<SendRecord> {
        self.lock().sends.clone()
    }

    pub fn stops(&self) -> Vec<SessionRef> {
        self.lock().stops.clone()
    }

    pub fn focuses(&self) -> Vec<SessionRef> {
        self.lock().focuses.clone()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl Runner for FakeRunner {
    fn start(&self, spec: &LaunchSpec) -> Result<SessionRef> {
        let (session, hook) = {
            let mut state = self.lock();
            let id = state.next_id;
            state.next_id = id
                .checked_add(1)
                .context("allocate fake runner session id")?;
            let session = SessionRef::Herdr {
                workspace: "fake".to_owned(),
                pane: format!("session-{id}"),
            };
            state.starts.push(spec.clone());
            state
                .sessions
                .insert(session.clone(), FakeSession::default());
            (session, state.start_hook.clone())
        };
        if let Some(hook) = hook
            && let Err(error) = hook(spec)
        {
            self.lock().sessions.remove(&session);
            return Err(error)
                .with_context(|| format!("run fake agent launch hook for {}", spec.label));
        }
        Ok(session)
    }

    fn status(&self, session: &SessionRef) -> Result<SessionStatus> {
        let mut state = self.lock();
        let Some(entry) = state.sessions.get_mut(session) else {
            return Ok(SessionStatus::Gone);
        };
        if let Some(status) = entry.queue.pop_front() {
            return Ok(status);
        }
        if let Some(status) = &entry.status {
            return Ok(status.clone());
        }
        Ok(state.default_status.clone())
    }

    fn send(&self, session: &SessionRef, text: &str, origin: Origin) -> Result<()> {
        let mut state = self.lock();
        let status = state
            .sessions
            .get(session)
            .map(|entry| entry.status.as_ref().unwrap_or(&state.default_status));
        if status.is_none() || status == Some(&SessionStatus::Gone) {
            return Err(refuse(
                "fake runner session is gone; use `corral retry <task>`",
                Vec::new(),
            ));
        }
        if state
            .sessions
            .get(session)
            .is_some_and(|entry| entry.refuses_send)
        {
            return Err(refuse(
                "fake runner session cannot receive messages",
                vec![],
            ));
        }
        state.sends.push(SendRecord {
            session: session.clone(),
            text: text.to_owned(),
            origin,
        });
        Ok(())
    }

    fn stop(&self, session: &SessionRef) -> Result<()> {
        let mut state = self.lock();
        state.stops.push(session.clone());
        let entry = state.sessions.entry(session.clone()).or_default();
        entry.status = Some(SessionStatus::Gone);
        entry.queue.clear();
        Ok(())
    }

    fn focus(&self, session: &SessionRef) -> Result<()> {
        self.lock().focuses.push(session.clone());
        Ok(())
    }
}
