//! Persisted inventory observations shared by every pane owner.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::Ctx;

pub(crate) struct Inventory {
    pub panes: Vec<corral_herdr::PaneInfo>,
    // Keep ownership stable until every consumer has finished with these IDs.
    _ownership: corral_core::StoreLock,
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
    match ctx.herdr.pane_list() {
        // Ownership can be established by the first launch while pane_list is in flight.
        Ok(panes) if !panes.is_empty() => match (|| -> Result<Option<Inventory>> {
            let Some(ownership) = ctx.store.try_lock_shared("session-ownership")? else {
                return Ok(None);
            };
            if !crate::session_scope::owns_inventory(ctx)? {
                return Ok(None);
            }
            Ok(Some(Inventory {
                panes,
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
    /// Admission probes may record presence, but cannot turn one sync's miss into two.
    pub count_miss: bool,
}

#[derive(Serialize, Deserialize)]
struct Observation {
    seen: bool,
    misses: u8,
    at: Timestamp,
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
    let Some(_lock) = ctx.store.try_lock("pane-liveness")? else {
        // Another observer is updating the evidence; defer a negative decision.
        return Ok(false);
    };
    let path = ctx.store.state_dir().join("pane-liveness.json");
    let mut observations: BTreeMap<String, Observation> = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("parsing pane observations")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(error) => return Err(error).context("reading pane observations"),
    };
    let key = format!("{pane}@{}", sample.started);
    let observation = observations.entry(key).or_insert(Observation {
        seen: sample.seen,
        misses: 0,
        at: sample.started,
    });
    observation.seen |= sample.seen || sample.present;
    if sample.present {
        observation.misses = 0;
    } else if sample.count_miss && sample.at != observation.at {
        // Wall time can step backwards. It identifies duplicate calls within
        // one sync, not the causal order of independently sampled inventories.
        observation.misses = observation.misses.saturating_add(1);
    }
    observation.at = sample.at;
    let gone = !sample.present
        && if observation.seen {
            observation.misses >= 2
        } else {
            expired
        };
    std::fs::create_dir_all(ctx.store.state_dir())
        .context("creating pane observation directory")?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&observations)?)
        .context("writing pane observations")?;
    std::fs::rename(&tmp, &path).context("saving pane observations")?;
    Ok(gone)
}
