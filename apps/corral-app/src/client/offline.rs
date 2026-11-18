//! Restart-only hints, never an authenticated transport or a live projection.
//!
//! Only complete wire Tasks/Plans/Projects snapshots and Health are retained. Later
//! complete deltas patch their saved baseline transactionally; partial pages and
//! deltas without a baseline are not saved. An oversized update fails without
//! truncating or replacing the last complete snapshot (2 MiB encoded limit). A snapshot
//! saved by an older wire version is discarded, not migrated.
//!
//! Namespace switches and `deactivate()` discard hints, not pending owner words.
//! Explicit `forget()` removes the active hint and namespace records and writes a
//! persistent `forgotten/{namespace}` tombstone in one transaction, even from a
//! fresh AppState. Reconnecting never clears that tombstone.
use super::{core::Event, durable, outbox::namespace_key};
use crate::model::{
    Id,
    daemon::{Board, Feed, Link, Task, TopicState},
    devices::DeviceLink,
};
use corral_protocol::{
    Health,
    app::AppSession,
    rows::{PlanRow, ProjectRow, TaskRow},
    subscription::SubDelta,
};
use nsh_core::prelude::Signal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
};

const MAX_BYTES: usize = 2 * 1024 * 1024;
/// Version 2 carries the engine's task rows.
const SNAPSHOT_VERSION: u32 = 2;
const ACTIVE: &str = "active";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Marker {
    version: u32,
    store: String,
    device: String,
    session: AppSession,
    capabilities: BTreeSet<String>,
    last_sync: Option<String>,
}

impl Marker {
    fn namespace(&self) -> String {
        namespace_key(&self.store, &self.device)
    }

    fn check(&self) -> Result<(), String> {
        if self.version != 1
            || self.store.is_empty()
            || self.device.is_empty()
            || self.session.device.id != self.device
            || self.session.device.revoked_at.is_some()
        {
            return Err(error("invalid or unsupported active identity"));
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct TopicRows<T> {
    epoch: u64,
    rows: BTreeMap<String, T>,
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    tasks: Option<TopicRows<TaskRow>>,
    plans: Option<TopicRows<PlanRow>>,
    projects: Option<TopicRows<ProjectRow>>,
    health: Option<Health>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            version: SNAPSHOT_VERSION,
            tasks: None,
            plans: None,
            projects: None,
            health: None,
        }
    }
}

/// A saved snapshot of this wire version; an older one is dropped rather than misread.
fn stored(value: Value) -> Result<Snapshot, String> {
    if value["version"].as_u64() != Some(u64::from(SNAPSHOT_VERSION)) {
        return Ok(Snapshot::default());
    }
    decode(value)
}

/// Owned capture context. Saving always patches the latest durable DTOs, not stale memory.
#[derive(Default)]
pub(super) struct Cache {
    marker: Option<Marker>,
    feed: Option<Signal<Feed>>,
}

impl Cache {
    /// Restore only the paired browser device or the native credential for the saved store.
    pub(super) async fn restore(
        feed: Signal<Feed>,
        expected_device: Option<&str>,
    ) -> Result<Option<((String, String), Self)>, String> {
        Self::restore_using(feed, async |marker| {
            #[cfg(target_arch = "wasm32")]
            {
                Ok(expected_device.is_some_and(|device| device == marker.device))
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = expected_device;
                let store = marker.store.clone();
                let device = nsh_core::prelude::blocking(move || super::credentials::load(&store))
                    .await
                    .map_err(|cause| error(format_args!("cannot verify native identity: {cause}")))?
                    .map_err(|cause| {
                        error(format_args!("cannot verify native identity: {cause}"))
                    })?;
                Ok(device.as_deref() == Some(marker.device.as_str()))
            }
        })
        .await
    }

    #[cfg(test)]
    async fn restore_with(
        feed: Signal<Feed>,
        authorize: impl FnOnce(&Marker) -> Result<bool, String>,
    ) -> Result<Option<((String, String), Self)>, String> {
        Self::restore_using(feed, async move |marker| authorize(marker)).await
    }

    async fn restore_using(
        feed: Signal<Feed>,
        authorize: impl std::ops::AsyncFnOnce(&Marker) -> Result<bool, String>,
    ) -> Result<Option<((String, String), Self)>, String> {
        let (_, mut records) = durable::read().await?;
        let Some(value) = records.remove(ACTIVE) else {
            return Ok(None);
        };
        let marker: Marker = decode(value)?;
        marker.check()?;
        if !authorize(&marker).await? {
            Self::deactivate_marker(Some(&marker)).await?;
            return Ok(None);
        }
        let snapshot = match records.remove(&snapshot_key(&marker.namespace())) {
            Some(value) => {
                check_bound(&value)?;
                stored(value)?
            }
            None => Snapshot::default(),
        };
        project(feed, &marker, snapshot);
        let namespace = (marker.store.clone(), marker.device.clone());
        Ok(Some((
            namespace,
            Self {
                marker: Some(marker),
                feed: Some(feed),
            },
        )))
    }

    /// Persist the active authenticated identity before accepting subsequent cache events.
    pub(super) async fn authenticated(
        &mut self,
        feed: Signal<Feed>,
        session: &AppSession,
    ) -> Result<(), String> {
        let marker = feed.with_untracked(|feed| -> Result<Marker, String> {
            Ok(Marker {
                version: 1,
                store: feed
                    .store
                    .clone()
                    .ok_or_else(|| error("authenticated session has no store"))?,
                device: session.device.id.clone(),
                session: session.clone(),
                capabilities: feed.capabilities.clone(),
                last_sync: feed.synced.clone(),
            })
        })?;
        marker.check()?;
        let namespace = marker.namespace();
        let value = encode(&marker)?;
        durable::transaction(|records| {
            if let Some(previous) = records.get(ACTIVE) {
                let previous: Marker = decode(previous.clone())?;
                previous.check()?;
                let old = previous.namespace();
                if old != namespace {
                    // Outbox lifecycle owns old pending words; only the display hint expires.
                    records.remove(&snapshot_key(&old));
                }
            }
            records.insert(ACTIVE.into(), value.clone());
            Ok(())
        })
        .await?;
        self.marker = Some(marker);
        self.feed = Some(feed);
        Ok(())
    }

    /// Persist only complete supported events, deriving each patch from the latest snapshot.
    pub(super) async fn capture(&mut self, event: &Event) -> Result<(), String> {
        if !cacheable(event) {
            return Ok(());
        }
        let Some(context) = self.marker.as_ref() else {
            return Ok(());
        };
        let namespace = context.namespace();
        let key = snapshot_key(&namespace);
        let last_sync = self.feed.and_then(|feed| {
            feed.with_untracked(|feed| {
                if feed.clock.is_empty() {
                    feed.synced.clone()
                } else {
                    Some(feed.clock.clone())
                }
            })
        });
        let (updated, _) = durable::transaction(|records| {
            let Some(active) = records.get(ACTIVE) else {
                return Err(error("active identity was forgotten"));
            };
            let mut marker: Marker = decode(active.clone())?;
            marker.check()?;
            if marker.namespace() != namespace {
                return Err(error("active identity changed before snapshot persistence"));
            }
            let mut snapshot = match records.get(&key) {
                Some(value) => stored(value.clone())?,
                None => Snapshot::default(),
            };
            if !apply_event(&mut snapshot, event) {
                return Ok(None);
            }
            let value = encode(&snapshot)?;
            check_bound(&value)?;
            if last_sync.is_some() {
                marker.last_sync = last_sync.clone();
            }
            records.insert(key.clone(), value);
            records.insert(ACTIVE.into(), encode(&marker)?);
            Ok(Some(marker))
        })
        .await?;
        if let Some(marker) = updated {
            self.marker = Some(marker);
        }
        Ok(())
    }

    async fn deactivate_marker(expected: Option<&Marker>) -> Result<(), String> {
        durable::transaction(|records| {
            let Some(value) = records.get(ACTIVE) else {
                return Ok(());
            };
            let marker: Marker = decode(value.clone())?;
            marker.check()?;
            // Native credential verification yields to a worker. Do not erase an
            // identity another instance authenticated while that check was in flight.
            if expected.is_some_and(|expected| expected != &marker) {
                return Ok(());
            }
            records.remove(&snapshot_key(&marker.namespace()));
            records.remove(ACTIVE);
            Ok(())
        })
        .await?;
        Ok(())
    }

    /// Explicitly delete the active namespace's local words and hints, not other identities.
    pub(super) async fn forget() -> Result<(), String> {
        durable::transaction(|records| {
            let Some(value) = records.get(ACTIVE) else {
                return Ok(());
            };
            let marker: Marker = decode(value.clone())?;
            marker.check()?;
            let namespace = marker.namespace();
            records.remove(ACTIVE);
            erase_namespace(records, &namespace);
            Ok(())
        })
        .await?;
        Ok(())
    }

    /// Forget a known identity without disturbing another process's active namespace.
    pub(super) async fn forget_namespace(store: &str, device: &str) -> Result<(), String> {
        let namespace = namespace_key(store, device);
        durable::transaction(|records| {
            if let Some(value) = records.get(ACTIVE) {
                let marker: Marker = decode(value.clone())?;
                marker.check()?;
                if marker.store == store && marker.device == device {
                    records.remove(ACTIVE);
                }
            }
            erase_namespace(records, &namespace);
            Ok(())
        })
        .await?;
        Ok(())
    }
}

fn erase_namespace(records: &mut durable::Records, namespace: &str) {
    let snapshot = snapshot_key(namespace);
    records.retain(|key, _| key != &snapshot && !key.starts_with(namespace));
    records.insert(format!("forgotten/{namespace}"), Value::Bool(true));
}

fn cacheable(event: &Event) -> bool {
    matches!(
        event,
        Event::Health(_)
            | Event::Rows {
                delta: SubDelta::Tasks { .. } | SubDelta::Plans { .. } | SubDelta::Projects { .. },
                ..
            }
    )
}

fn apply_event(snapshot: &mut Snapshot, event: &Event) -> bool {
    match event {
        Event::Health(health) => {
            snapshot.health = Some(health.clone());
            true
        }
        Event::Rows {
            delta: SubDelta::Tasks { delta },
            replacement,
        } if !delta.more => apply_topic(
            &mut snapshot.tasks,
            delta.epoch,
            &delta.rows,
            &delta.left,
            *replacement,
            |row| &row.task.id,
        ),
        Event::Rows {
            delta: SubDelta::Plans { delta },
            replacement,
        } if !delta.more => apply_topic(
            &mut snapshot.plans,
            delta.epoch,
            &delta.rows,
            &delta.left,
            *replacement,
            |row| &row.parent.id,
        ),
        Event::Rows {
            delta: SubDelta::Projects { delta },
            replacement,
        } if !delta.more => apply_topic(
            &mut snapshot.projects,
            delta.epoch,
            &delta.rows,
            &delta.left,
            *replacement,
            |row| &row.id,
        ),
        _ => false,
    }
}

fn apply_topic<T: Clone>(
    cached: &mut Option<TopicRows<T>>,
    epoch: u64,
    rows: &[T],
    left: &[String],
    replacement: bool,
    id: impl Fn(&T) -> &str,
) -> bool {
    if replacement {
        *cached = Some(TopicRows {
            epoch,
            rows: BTreeMap::new(),
        });
    }
    let Some(cached) = cached.as_mut().filter(|cached| cached.epoch == epoch) else {
        return false;
    };
    for row in rows {
        cached.rows.insert(id(row).to_owned(), row.clone());
    }
    for id in left {
        cached.rows.remove(id);
    }
    true
}

fn project(feed: Signal<Feed>, marker: &Marker, snapshot: Snapshot) {
    feed.update(|feed| {
        feed.board = Board::default();
        feed.plans.clear();
        feed.projects.clear();
        feed.health = snapshot.health;
        for topic in ["tasks", "plans", "projects"] {
            feed.topics.remove(topic);
        }
        if let Some(tasks) = snapshot.tasks {
            feed.board.apply_batch(
                tasks.epoch,
                tasks.rows.into_values().map(Task::new).collect(),
                &[],
            );
            feed.topics.insert("tasks".into(), TopicState::default());
        }
        if let Some(plans) = snapshot.plans {
            feed.plans = plans
                .rows
                .into_iter()
                .map(|(id, row)| (Id::new(&id), row))
                .collect();
            feed.topics.insert("plans".into(), TopicState::default());
        }
        if let Some(projects) = snapshot.projects {
            feed.projects = projects
                .rows
                .into_iter()
                .map(|(id, row)| (Id::new(&id), row))
                .collect();
            feed.topics.insert("projects".into(), TopicState::default());
        }
        feed.store = Some(marker.store.clone());
        feed.workspace = std::path::Path::new(&marker.store)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .and_then(|parent| parent.to_str())
            .map(str::to_owned);
        feed.capabilities = marker.capabilities.clone();
        feed.link = Link::Down;
        feed.mark_stale();
        // mark_stale intentionally clears live sessions; this one is identity metadata only.
        feed.session = Some(marker.session.clone());
        feed.device = DeviceLink::Enrolled {
            id: marker.device.clone(),
            name: marker.session.device.name.clone(),
        };
        feed.synced = marker.last_sync.clone();
        feed.problem = Some("Offline: showing the last saved daemon data.".into());
    });
}

fn snapshot_key(namespace: &str) -> String {
    format!("snapshot/{namespace}")
}
fn error(message: impl std::fmt::Display) -> String {
    format!("Offline cache: {message}")
}
fn encode(value: &impl Serialize) -> Result<Value, String> {
    serde_json::to_value(value)
        .map_err(|cause| error(format_args!("cannot encode saved data: {cause}")))
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value)
        .map_err(|cause| error(format_args!("cannot decode saved data: {cause}")))
}

// Measure the encoded Value without allocating a second copy of the snapshot.
struct ByteLimit(usize);
impl Write for ByteLimit {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.0 {
            return Err(io::Error::other(
                "snapshot exceeds 2 MiB; previous complete snapshot retained",
            ));
        }
        self.0 -= bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn check_bound(value: &Value) -> Result<(), String> {
    serde_json::to_writer(ByteLimit(MAX_BYTES), value)
        .map_err(|cause| error(format_args!("cannot save snapshot: {cause}")))
}

#[cfg(test)]
#[path = "offline_tests.rs"]
mod tests;
