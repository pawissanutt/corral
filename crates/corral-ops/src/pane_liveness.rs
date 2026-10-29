//! Persisted inventory observations shared by every pane owner.

use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::Ctx;

pub(crate) struct Inventory {
    pub panes: Vec<corral_herdr::PaneInfo>,
    pub ticket: Ticket,
    // Keep ownership stable until every consumer has finished with these IDs. The lease
    // also carries this pass's single read of the session binding.
    _ownership: corral_core::StoreLock,
}

impl Inventory {
    pub const fn snapshot<'a>(
        &'a self,
        panes: &'a HashMap<String, corral_herdr::PaneInfo>,
    ) -> Snapshot<'a> {
        Snapshot {
            panes,
            ticket: Some(self.ticket),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Snapshot<'a> {
    pub panes: &'a HashMap<String, corral_herdr::PaneInfo>,
    pub ticket: Option<Ticket>,
}

impl<'a> Snapshot<'a> {
    /// Public injected-inventory APIs describe a new observation on each call.
    pub const fn injected(panes: &'a HashMap<String, corral_herdr::PaneInfo>) -> Self {
        Self {
            panes,
            ticket: None,
        }
    }
}

#[derive(Clone, Copy, Deserialize, Serialize)]
pub(crate) struct Ticket {
    incarnation: [u8; 16],
    generation: u64,
}

#[derive(Deserialize, Serialize)]
struct State {
    version: u8,
    incarnation: [u8; 16],
    generation: u64,
    observations: BTreeMap<String, Observation>,
}

impl State {
    fn new(observations: BTreeMap<String, Observation>) -> Result<Self> {
        let mut incarnation = [0; 16];
        getrandom::fill(&mut incarnation)
            .map_err(|error| anyhow::anyhow!("reading inventory identity entropy: {error}"))?;
        let mut observations = observations;
        // Legacy misses have no causal identity and cannot authorize a new decision.
        for observation in observations.values_mut() {
            observation.misses = 0;
            observation.generation = 0;
        }
        Ok(Self {
            version: 1,
            incarnation,
            generation: 0,
            observations,
        })
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Stored {
    Current(State),
    Legacy(BTreeMap<String, Observation>),
}

fn read(ctx: &Ctx) -> Result<Option<Stored>> {
    let path = ctx.store.state_dir().join("pane-liveness.json");
    let stored: Stored = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("parsing pane observations")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("reading pane observations"),
    };
    match &stored {
        Stored::Current(state) => {
            anyhow::ensure!(state.version == 1, "unsupported pane observation version");
        }
        Stored::Legacy(_) => {}
    }
    Ok(Some(stored))
}

/// Confirmed absence from retained observations; never probes or advances inventory.
pub(crate) fn recorded_missing(ctx: &Ctx) -> Result<BTreeMap<String, Timestamp>> {
    let observations = match read(ctx)? {
        Some(Stored::Current(state)) => state.observations,
        Some(Stored::Legacy(_)) | None => return Ok(BTreeMap::new()),
    };
    let grace = i64::try_from(ctx.store.config.sync.startup_grace_secs).unwrap_or(i64::MAX);
    Ok(observations
        .into_iter()
        .filter_map(|(identity, observation)| {
            let (_, started) = identity.rsplit_once('@')?;
            let started: Timestamp = started.parse().ok()?;
            let absent = if observation.seen {
                observation.misses >= 2
            } else {
                observation.at.duration_since(started).as_secs() >= grace
            };
            absent.then_some((identity, observation.at))
        })
        .collect())
}

fn save(ctx: &Ctx, state: &State) -> Result<()> {
    std::fs::create_dir_all(ctx.store.state_dir())
        .context("creating pane observation directory")?;
    let path = ctx.store.state_dir().join("pane-liveness.json");
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(state)?).context("writing pane observations")?;
    std::fs::rename(&tmp, &path).context("saving pane observations")
}

fn issue(ctx: &Ctx) -> Result<Option<Ticket>> {
    let Some(_lock) = ctx.store.try_lock("pane-liveness")? else {
        return Ok(None);
    };
    let mut state = match read(ctx)? {
        Some(Stored::Current(state)) => state,
        Some(Stored::Legacy(observations)) => State::new(observations)?,
        None => State::new(BTreeMap::new())?,
    };
    state.generation = state
        .generation
        .checked_add(1)
        .context("pane inventory generation exhausted")?;
    save(ctx, &state)?;
    Ok(Some(Ticket {
        incarnation: state.incarnation,
        generation: state.generation,
    }))
}

/// Empty output cannot distinguish an engine failure from every pane closing.
/// Only a usable nonempty inventory is evidence for individual missing panes.
pub(crate) fn inventory(ctx: &Ctx) -> Option<Inventory> {
    match crate::session_scope::owns_inventory(ctx) {
        Ok(true) => {}
        Ok(false) => return None,
        Err(error) => {
            unknown(&format!(
                "cannot establish herdr session ownership: {error:#}"
            ));
            return None;
        }
    }
    let ticket = match issue(ctx) {
        Ok(Some(ticket)) => ticket,
        Ok(None) => return None,
        Err(error) => {
            unknown(&format!("cannot register pane inventory: {error:#}"));
            return None;
        }
    };
    match ctx.herdr.pane_list() {
        // Ownership can be established by the first launch while pane_list is in flight.
        Ok(panes) if !panes.is_empty() => match (|| -> Result<Option<Inventory>> {
            let Some(ownership) = ctx.store.try_begin_session_pass()? else {
                return Ok(None);
            };
            if !crate::session_scope::owns_inventory(ctx)? {
                return Ok(None);
            }
            Ok(Some(Inventory {
                panes,
                ticket,
                _ownership: ownership,
            }))
        })() {
            Ok(inventory) => inventory,
            Err(error) => {
                unknown(&format!(
                    "cannot establish herdr session ownership: {error:#}"
                ));
                None
            }
        },
        Ok(_) => {
            unknown("herdr returned an empty pane inventory");
            None
        }
        Err(error) => {
            unknown(&format!("herdr pane listing failed: {error:#}"));
            None
        }
    }
}

pub(crate) fn unknown(reason: &str) {
    crate::messages::report(
        None,
        "pane inventory",
        format!("{reason}; pane liveness is unknown, retaining task states and slots"),
    );
}

pub(crate) struct Sample<'a> {
    pub task: &'a corral_core::Task,
    pub pane: Option<&'a str>,
    pub started: Timestamp,
    pub seen: bool,
    pub present: bool,
    pub at: Timestamp,
    pub ticket: Option<Ticket>,
    /// Admission probes may record presence, but cannot turn one sync's miss into two.
    pub count_miss: bool,
}

#[derive(Serialize, Deserialize)]
struct Observation {
    seen: bool,
    misses: u8,
    at: Timestamp,
    #[serde(default)]
    generation: u64,
}

/// A listed pane is always alive. An unseen launch gets startup grace; after a
/// sighting, two distinct consecutive missing samples are needed, even minutes
/// after startup. The launch timestamp prevents pane-ID reuse inheriting misses.
pub(crate) fn gone(ctx: &Ctx, sample: Sample<'_>) -> Result<bool> {
    if !crate::session_scope::owns_task(ctx, sample.task)? {
        return Ok(false);
    }
    let grace = SignedDuration::from_secs(
        i64::try_from(ctx.store.config.sync.startup_grace_secs).unwrap_or(i64::MAX),
    );
    let expired = sample.at.duration_since(sample.started) >= grace;
    let Some(pane) = sample.pane else {
        return Ok(expired);
    };
    let ticket = match sample.ticket {
        Some(ticket) => ticket,
        None => match issue(ctx)? {
            Some(ticket) => ticket,
            None => return Ok(false),
        },
    };
    let Some(_lock) = ctx.store.try_lock("pane-liveness")? else {
        // Another observer is updating the evidence; defer a negative decision.
        return Ok(false);
    };
    let mut state = match read(ctx)? {
        Some(Stored::Current(state)) if state.incarnation == ticket.incarnation => state,
        Some(Stored::Current(_) | Stored::Legacy(_)) | None => return Ok(false),
    };
    let key = format!("{pane}@{}", sample.started);
    let observation = state.observations.entry(key).or_insert(Observation {
        seen: sample.seen,
        misses: 0,
        at: sample.started,
        generation: 0,
    });
    if ticket.generation < observation.generation {
        return Ok(false);
    }
    if ticket.generation == observation.generation {
        // A second consumer may use the latest result, but cannot change its evidence.
        return Ok(!sample.present
            && if observation.seen {
                observation.misses >= 2
            } else {
                expired
            });
    }
    observation.generation = ticket.generation;
    observation.seen |= sample.seen || sample.present;
    if sample.present {
        observation.misses = 0;
    } else if sample.count_miss && sample.at != observation.at {
        // Equal injected timestamps still suppress duplicate misses. Generation,
        // not wall-clock direction, orders independently sampled inventories.
        observation.misses = observation.misses.saturating_add(1);
    }
    observation.at = sample.at;
    let gone = !sample.present
        && if observation.seen {
            observation.misses >= 2
        } else {
            expired
        };
    save(ctx, &state)?;
    Ok(gone)
}

#[cfg(test)]
#[path = "pane_liveness_tests.rs"]
mod tests;
