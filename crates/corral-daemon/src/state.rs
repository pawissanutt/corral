//! Daemon lifecycle, sync wakeups, and the revision of observed read models.
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use corral_engine::{Actor, Engine, PipelineMode, Runner};
use corral_protocol::{DaemonState, Health, NudgeAck};

use crate::devices::Devices;

pub(crate) type RunnerFactory = Arc<dyn Fn() -> Box<dyn Runner> + Send + Sync>;

pub(crate) struct Shared {
    root: PathBuf,
    runner: Option<RunnerFactory>,
    devices: Arc<Devices>,
    epoch: u64,
    revision: AtomicU64,
    config_fingerprint: Mutex<Option<[u8; 32]>>,
    pub(crate) latest_seq: AtomicU64,
    pub(crate) row_cache: crate::views::RowCache,
    pub(crate) compared_heads: crate::views::ComparedHeads,
    pub(crate) system: crate::system::System,
    pub(crate) host: Arc<crate::host::Sampler>,
    pub(crate) harness_inventory: crate::harnesses::Inventory,
    pub(crate) terminal_cache: crate::topics::TerminalCache,
    pub(crate) session_live: crate::topics::SessionLive,
    pub(crate) chat: crate::assistant::Service,
    pub(crate) build_ops: Mutex<()>,
    pub(crate) late_ops: crate::ops::Late,
    state: Mutex<State>,
    pub(crate) changed: Condvar,
}

pub(crate) struct State {
    pub(crate) stop: bool,
    pub(crate) closed: bool,
    pub(crate) workers: usize,
    pub(crate) app_workers: usize,
    pub(crate) op_workers: usize,
    pub(crate) server_error: Option<String>,
    pending_sync: bool,
    started: u64,
    completed: u64,
    health: Health,
}

impl Shared {
    #[cfg(test)]
    pub(crate) fn new(root: PathBuf, runner: Option<RunnerFactory>) -> Result<Self> {
        let devices = Devices::open(&root.join(crate::STATE_DIR))?;
        Self::with_devices(root, runner, devices)
    }

    pub(crate) fn with_devices(
        root: PathBuf,
        runner: Option<RunnerFactory>,
        devices: Devices,
    ) -> Result<Self> {
        let mut epoch = [0_u8; 8];
        getrandom::fill(&mut epoch)
            .map_err(|error| anyhow::anyhow!("generate daemon epoch: {error}"))?;
        // Cursor epochs travel through browser clients, including implementations using JS numbers.
        let epoch = u64::from_ne_bytes(epoch) & ((1 << 53) - 1);
        let config_fingerprint = Mutex::new(crate::config::fingerprint(&root)?);
        Ok(Self {
            root,
            runner,
            devices: Arc::new(devices),
            epoch,
            revision: AtomicU64::new(1),
            config_fingerprint,
            latest_seq: AtomicU64::new(0),
            row_cache: Default::default(),
            compared_heads: Default::default(),
            system: Default::default(),
            host: Default::default(),
            harness_inventory: Default::default(),
            terminal_cache: Default::default(),
            session_live: Default::default(),
            chat: Default::default(),
            build_ops: Mutex::default(),
            late_ops: Default::default(),
            state: Mutex::new(State {
                stop: false,
                closed: false,
                workers: 0,
                app_workers: 0,
                op_workers: 0,
                server_error: None,
                pending_sync: true,
                started: 0,
                completed: 0,
                health: crate::health::initial(),
            }),
            changed: Condvar::new(),
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
    pub(crate) fn devices(&self) -> &Arc<Devices> {
        &self.devices
    }
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }
    pub(crate) fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub(crate) fn engine(&self) -> Result<Engine> {
        let mut engine = match &self.runner {
            Some(runner) => Engine::open_with_runner(&self.root, runner()),
            None => Engine::open(&self.root),
        }
        .context("open daemon engine")?;
        engine.actor = Actor::Owner;
        engine.pipeline = PipelineMode::Off;
        Ok(engine)
    }

    pub(crate) fn chat_placement(&self) -> Box<dyn Runner> {
        self.runner.as_ref().map_or_else(
            || Box::new(corral_engine::runner::ProcessRunner) as Box<dyn Runner>,
            |runner| runner(),
        )
    }

    pub(crate) fn private_chat_sockets(&self) -> Option<PathBuf> {
        self.runner.as_ref().map(|_| self.root.join("sock"))
    }

    pub(crate) fn build_data_root(&self) -> Result<PathBuf> {
        if self.runner.is_some() {
            Ok(self.root.join("pi-data"))
        } else {
            corral_engine::runner::pi::pi_data_root()
        }
    }

    pub(crate) fn runner_reachable(&self, engine: &Engine) -> bool {
        self.runner.is_some() || crate::health::runner_reachable(&engine.config.runner)
    }

    pub(crate) fn publish(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn config_fingerprint(&self) -> Option<[u8; 32]> {
        *self
            .config_fingerprint
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    pub(crate) fn refresh_config(&self) -> Result<()> {
        self.update_config_fingerprint(|| crate::config::fingerprint(&self.root))
    }

    pub(crate) fn update_config_fingerprint(
        &self,
        observe: impl FnOnce() -> Result<Option<[u8; 32]>>,
    ) -> Result<()> {
        let mut last = self
            .config_fingerprint
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let current = observe()?;
        if *last != current {
            *last = current;
            self.publish();
            self.nudge(None);
        }
        Ok(())
    }

    pub(crate) fn health(&self) -> Health {
        let mut health = self.lock().health.clone();
        // The engine read is best effort and must not run under the state lock.
        health.slots = self
            .engine()
            .and_then(|engine| crate::health::slots(&engine))
            .ok();
        health
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub(crate) fn begin_sync(&self, interval: Duration) -> bool {
        let mut state = self.wait_while(interval, |state| !state.stop && !state.pending_sync);
        if state.stop {
            return false;
        }
        state.started += 1;
        state.pending_sync = false;
        true
    }

    pub(crate) fn finish_sync(&self, runner_reachable: bool, error: Option<String>) {
        let mut state = self.lock();
        state.health.daemon = if runner_reachable && error.is_none() {
            DaemonState::Healthy
        } else {
            DaemonState::Degraded
        };
        state.health.runner_reachable = runner_reachable;
        state.health.last_sync_at = Some(jiff::Timestamp::now().to_string());
        state.health.last_sync_error = error;
        state.completed = state.started;
        self.publish();
        self.changed.notify_all();
    }

    pub(crate) fn nudge(&self, wait: Option<Duration>) -> NudgeAck {
        let mut state = self.lock();
        let target = state.started + 1;
        if !state.stop {
            state.pending_sync = true;
            self.changed.notify_all();
        }
        if let Some(wait) = wait {
            (state, _) = self
                .changed
                .wait_timeout_while(state, wait, |state| !state.stop && state.completed < target)
                .unwrap_or_else(|error| error.into_inner());
        }
        NudgeAck {
            pass_seq: state.completed,
            completed: state.completed >= target,
        }
    }

    pub(crate) fn fail_server(&self, error: String) {
        let mut state = self.lock();
        state.server_error.get_or_insert(error);
        state.stop = true;
        self.changed.notify_all();
    }

    pub(crate) fn stop(&self) {
        self.lock().stop = true;
        self.changed.notify_all();
        self.system.stop();
        self.host.stop();
    }

    pub(crate) fn close(&self) {
        self.system.stop();
        self.host.stop();
        let mut state = self.lock();
        state.closed = true;
        state.stop = true;
        self.changed.notify_all();
    }

    pub(crate) fn wait(&self, duration: Duration) {
        drop(self.wait_while(duration, |state| !state.stop));
    }

    pub(crate) fn wait_closed(&self, duration: Duration) -> bool {
        self.wait_while(duration, |state| !state.closed).closed
    }

    pub(crate) fn wait_workers(&self, duration: Duration) {
        drop(self.wait_while(duration, |state| state.workers != 0));
    }

    pub(crate) fn wait_ops(&self) {
        drop(
            self.changed
                .wait_while(self.lock(), |state| state.op_workers != 0)
                .unwrap_or_else(|error| error.into_inner()),
        );
    }

    fn wait_while(
        &self,
        duration: Duration,
        predicate: impl Fn(&State) -> bool,
    ) -> MutexGuard<'_, State> {
        self.changed
            .wait_timeout_while(self.lock(), duration, |state| predicate(state))
            .unwrap_or_else(|error| error.into_inner())
            .0
    }
}
