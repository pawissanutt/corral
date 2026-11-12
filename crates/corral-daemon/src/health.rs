//! Background health sampling and transitions-only durable state.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use corral_core::{Store, records::RecordArea};
use corral_ops::Ctx;
use corral_protocol::{Health, RuntimeState};
use serde::Serialize;

use crate::state::Shared;

const STATE: RecordArea<Record> = RecordArea::new("daemon");

#[derive(Serialize)]
struct Record {
    state: &'static str,
    reasons: Vec<String>,
    since: String,
    pid: u32,
    version: &'static str,
    started_at: String,
}

pub(crate) struct Transitions {
    started_at: String,
    previous: Option<Vec<String>>,
    stopped: bool,
}

impl Transitions {
    pub fn new() -> Self {
        Self {
            started_at: jiff::Timestamp::now().to_string(),
            previous: None,
            stopped: false,
        }
    }

    fn save(
        &self,
        store: &Store,
        state: &'static str,
        health: &Health,
        event: Option<&str>,
    ) -> Result<()> {
        let record = Record {
            state,
            reasons: health.reasons.clone(),
            since: jiff::Timestamp::now().to_string(),
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION"),
            started_at: self.started_at.clone(),
        };
        STATE
            .save(store, "state", &record)
            .context("recording daemon state")?;
        if let Some(event) = event {
            store
                .log_event(
                    None,
                    event,
                    serde_json::to_value(&record).context("encoding daemon state event")?,
                )
                .context("recording daemon state event")?;
        }
        Ok(())
    }

    pub fn started(&self, store: &Store, health: &Health) -> Result<()> {
        self.save(store, "started", health, Some("daemon_started"))
    }

    pub fn update(&mut self, store: &Store, health: &Health) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        // Ages advance in memory, but a continuously held lock or stall is one condition.
        let keys: Vec<_> = health
            .reasons
            .iter()
            .map(|reason| match reason_key(reason) {
                (name, Some(holder)) => format!("{name} by {holder}"),
                (reason, None) => reason.to_owned(),
            })
            .collect();
        if self.previous.as_ref() == Some(&keys) {
            return Ok(());
        }
        if !keys.is_empty() {
            self.save(store, "degraded", health, Some("daemon_degraded"))?;
        } else if self.previous.as_ref().is_some_and(|old| !old.is_empty()) {
            self.save(store, "recovered", health, Some("daemon_recovered"))?;
        }
        self.previous = Some(keys);
        Ok(())
    }

    pub fn stopped(&mut self, store: &Store, health: &Health) -> Result<()> {
        self.stopped = true;
        self.save(store, "stopped", health, None)
    }
}

/// Compare conditions without treating elapsed ages as new state.
pub(crate) fn reason_key(reason: &str) -> (&str, Option<&str>) {
    if reason.starts_with("sync stalled ") {
        ("sync stalled", None)
    } else if let Some((name, held)) = reason.split_once(" held ") {
        (
            name,
            Some(
                held.split_once(" by ")
                    .map_or("unknown", |(_, holder)| holder),
            ),
        )
    } else {
        (reason, None)
    }
}

pub(crate) fn start(
    ctx: &Ctx,
    shared: &Arc<Shared>,
    transitions: Arc<Mutex<Transitions>>,
) -> Result<()> {
    let store = ctx.store.clone();
    let shared_maintenance = Arc::clone(shared);
    std::thread::Builder::new()
        .name("corrald-maintenance".into())
        .spawn(move || {
            loop {
                shared_maintenance.wait(corral_ops::loop_pacing::SYNC_EVERY, false);
                if shared_maintenance.lock().stop {
                    break;
                }
                if let Err(error) = store.maintain_database() {
                    eprintln!("corrald database maintenance: {error:#}");
                }
            }
        })
        .context("starting daemon maintenance")?;

    let store = ctx.store.clone();
    let watchdog = Arc::clone(shared);
    std::thread::Builder::new()
        .name("corrald-watchdog".into())
        .spawn(move || {
            let mut held = BTreeMap::new();
            loop {
                if watchdog.lock().stop {
                    break;
                }
                let mut reasons = Vec::new();
                for name in corral_ops::SUPERVISOR_LOCKS {
                    match store.try_lock(name) {
                        Ok(Some(lease)) => {
                            drop(lease);
                            held.remove(name);
                        }
                        Ok(None) => {
                            let since = held.entry(name).or_insert_with(Instant::now);
                            if since.elapsed() >= Duration::from_secs(90) {
                                let holder = corral_ops::supervisor_lock_holder(
                                    &store.dir().join("lock").join(format!("{name}.lock")),
                                );
                                let pid = holder
                                    .strip_prefix("pid ")
                                    .and_then(|s| s.split([',', ' ']).next())
                                    .unwrap_or("unknown");
                                reasons.push(format!(
                                    "lock {name} held {}s by pid {pid}",
                                    since.elapsed().as_secs()
                                ));
                            }
                        }
                        Err(error) => eprintln!("corrald probing {name}: {error:#}"),
                    }
                }
                watchdog.lock().locks = reasons;
                watchdog.wait(Duration::from_secs(15), false);
            }
        })
        .context("starting daemon lock watchdog")?;

    let store = ctx.store.clone();
    let mut ctx = ctx.clone();
    let sampler = Arc::clone(shared);
    std::thread::Builder::new()
        .name("corrald-health".into())
        .spawn(move || {
            let mut checked: Option<Instant> = None;
            while !sampler.lock().stop {
                if let Err(error) = ctx.reload_config() {
                    eprintln!("corrald health config: {error:#}");
                }
                match corral_ops::daemon_health(&ctx) {
                    Ok(health) => {
                        let mut state = sampler.lock();
                        state.foreign = !health.owned;
                        state.pm = health.pm;
                        state.stores_percent = health.stores_percent;
                        state.stores_pressed = health.stores_pressed;
                    }
                    Err(error) => eprintln!("corrald sampling health: {error:#}"),
                }
                if checked.is_none_or(|checked| checked.elapsed() >= Duration::from_secs(10)) {
                    checked = Some(Instant::now());
                    let runtime = if ctx.herdr.available() {
                        RuntimeState::Ok
                    } else {
                        RuntimeState::Unreachable
                    };
                    sampler.lock().runtime = runtime;
                }
                // Store reads and a runtime ownership probe: frequent enough for a health line,
                // rare enough to stay invisible beside the pass itself.
                sampler.wait(Duration::from_secs(15), false);
            }
        })
        .context("starting daemon health sampler")?;

    let reporter = Arc::clone(shared);
    std::thread::Builder::new()
        .name("corrald-state".into())
        .spawn(move || {
            while !reporter.lock().stop {
                let health = reporter.health();
                if let Err(error) = transitions
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .update(&store, &health)
                {
                    eprintln!("corrald health transition: {error:#}");
                }
                reporter.wait(Duration::from_secs(1), false);
            }
        })
        .context("starting daemon state reporter")?;
    Ok(())
}
