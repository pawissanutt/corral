//! Lazy app reads. Only this worker opens snapshots or computes typed projections.
//! Connection ticks inspect published Arcs; they never scan the store.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, ensure};
use corral_core::Store;
use corral_ops::{Ctx, app_read};
use corral_protocol::{app, assistant::ThreadEntry, detail, subscription::Topic};

use crate::state::Shared;

mod rows;
pub(crate) use rows::Rows;

const QUEUE: usize = 32;
const POLL: Duration = Duration::from_millis(500);
/// Agent tails and the ended-session window move without a store write, so a watched agents
/// topic is read again at this interval even when the store is unchanged.
const OUTSIDE_STORE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Key {
    pub topic: u8,
    pub scope: Option<String>,
}

impl Key {
    pub fn new(topic: Topic, scope: Option<String>) -> Result<Self> {
        let tag = match topic {
            Topic::Inbox => 0,
            Topic::Plans => 1,
            Topic::Agents => 2,
            Topic::Assistant => 3,
            Topic::Usage => 4,
            Topic::Settings => 5,
            Topic::Thread => 6,
            _ => return Err(anyhow!("unsupported app topic")),
        };
        let scope = if topic == Topic::Thread {
            let scope = scope.context("thread scope is required")?;
            ensure!(
                scope == "quick"
                    || scope.strip_prefix("A-").is_some_and(|n| !n.starts_with('0')
                        && n.len() <= 20
                        && n.bytes().all(|b| b.is_ascii_digit())
                        && n.parse::<u64>().is_ok_and(|n| n > 0)),
                "invalid: thread scope must be quick or A-<n>"
            );
            Some(scope)
        } else {
            // Non-thread topics have exactly one unscoped identity.
            None
        };
        Ok(Self { topic: tag, scope })
    }

    pub fn topic(&self) -> Topic {
        match self.topic {
            0 => Topic::Inbox,
            1 => Topic::Plans,
            2 => Topic::Agents,
            3 => Topic::Assistant,
            4 => Topic::Usage,
            5 => Topic::Settings,
            _ => Topic::Thread,
        }
    }
}

pub(crate) struct Snapshot {
    pub rev: u64,
    pub rows: Rows,
}

struct Entry {
    users: usize,
    checked: Option<(u64, u64)>,
    /// When the rows were last read, for topics with inputs outside the store.
    read: Option<Instant>,
    published: Option<std::result::Result<Arc<Snapshot>, String>>,
}

impl Entry {
    fn overdue(&self, key: &Key) -> bool {
        key.topic() == Topic::Agents && self.read.is_none_or(|at| at.elapsed() >= OUTSIDE_STORE)
    }
}

enum Job {
    Detail(
        detail::DetailGet,
        mpsc::SyncSender<Result<detail::DetailReply>>,
    ),
    Changes(
        app::ChangesQuery,
        app::SeenCursor,
        mpsc::SyncSender<Result<app::ChangesPage>>,
    ),
    Thread(String, Option<u64>, mpsc::SyncSender<Result<ThreadPage>>),
}

pub(crate) struct ThreadPage {
    pub rows: Vec<ThreadEntry>,
    pub next: Option<u64>,
    pub head: u64,
}

pub(crate) struct Service {
    sender: mpsc::SyncSender<Job>,
    entries: Mutex<BTreeMap<Key, Entry>>,
}

/// A counted subscription. Replacement, explicit close and connection drop all release it.
pub(crate) struct Watch {
    service: Arc<Service>,
    pub key: Key,
}

impl Watch {
    pub fn snapshot(&self) -> Result<Option<Arc<Snapshot>>> {
        let entries = self
            .service
            .entries
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match entries
            .get(&self.key)
            .and_then(|entry| entry.published.as_ref())
        {
            Some(Ok(snapshot)) => Ok(Some(Arc::clone(snapshot))),
            Some(Err(error)) => Err(anyhow!("app topic unavailable: {error}")),
            None => Ok(None),
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let removed = {
            let mut entries = self
                .service
                .entries
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = entries.get_mut(&self.key) {
                entry.users -= 1;
                if entry.users == 0 {
                    entries.remove(&self.key)
                } else {
                    None
                }
            } else {
                None
            }
        };
        // Potentially large snapshots are destroyed outside the publication lock.
        drop(removed);
    }
}

pub(crate) fn watch(shared: &Shared, key: Key) -> Result<Watch> {
    let service = service(shared)?;
    {
        let mut entries = service.entries.lock().unwrap_or_else(|e| e.into_inner());
        ensure!(
            entries.contains_key(&key) || entries.len() < 128,
            "too many watched app topics"
        );
        let entry = entries.entry(key.clone()).or_insert(Entry {
            users: 0,
            checked: None,
            read: None,
            published: None,
        });
        entry.users += 1;
    }
    Ok(Watch { service, key })
}

fn service(shared: &Shared) -> Result<Arc<Service>> {
    shared
        .views
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(Arc::clone)
        .context("app read service unavailable")
}

fn enqueue<T>(
    shared: &Shared,
    job: impl FnOnce(mpsc::SyncSender<Result<T>>) -> Job,
) -> Result<mpsc::Receiver<Result<T>>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    service(shared)?
        .sender
        .try_send(job(sender))
        .map_err(|error| match error {
            mpsc::TrySendError::Full(_) => anyhow!("busy: app read queue full"),
            mpsc::TrySendError::Disconnected(_) => anyhow!("app read service unavailable"),
        })?;
    Ok(receiver)
}

pub(crate) fn detail(
    shared: &Shared,
    query: detail::DetailGet,
) -> Result<mpsc::Receiver<Result<detail::DetailReply>>> {
    ensure!(
        serde_json::to_vec(&query)?.len() <= app::APP_CONTROL_MAX,
        "too_large: detail request"
    );
    enqueue(shared, |reply| Job::Detail(query, reply))
}

pub(crate) fn changes(
    shared: &Shared,
    query: app::ChangesQuery,
    seen: app::SeenCursor,
) -> Result<mpsc::Receiver<Result<app::ChangesPage>>> {
    ensure!(
        query.limit > 0 && query.limit <= 200,
        "invalid: changes limit must be 1..=200"
    );
    ensure!(
        !matches!((query.after, query.before), (Some(after), Some(before)) if after >= before),
        "invalid: changes after must precede before"
    );
    enqueue(shared, |reply| Job::Changes(query, seen, reply))
}

pub(crate) fn thread(
    shared: &Shared,
    scope: String,
    before: Option<u64>,
) -> Result<mpsc::Receiver<Result<ThreadPage>>> {
    Key::new(Topic::Thread, Some(scope.clone()))?;
    enqueue(shared, |reply| Job::Thread(scope, before, reply))
}

/// Parent provides `Shared.views: Mutex<Option<Arc<views::Service>>>` and calls once.
/// Store/config are opened lazily on the worker, not startup or a connection thread.
pub(crate) fn start(ctx: &Ctx, shared: &Arc<Shared>) -> Result<()> {
    let root = ctx.store.root().to_path_buf();
    let herdr = ctx.herdr.clone();
    let probe = Arc::clone(&ctx.resource_probe);
    let (sender, receiver) = mpsc::sync_channel(QUEUE);
    let service = Arc::new(Service {
        sender,
        entries: Mutex::new(BTreeMap::new()),
    });
    let worker = Arc::clone(&service);
    let shared_worker = Arc::clone(shared);
    std::thread::Builder::new()
        .name("corrald-app-reads".into())
        .spawn(move || {
            let mut ctx = None;
            let mut generation = 0u64;
            let mut revision = 0u64;
            while !shared_worker.lock().closed {
                let job = match receiver.recv_timeout(POLL) {
                    Ok(job) => Some(job),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let active = !worker
                    .entries
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_empty();
                if job.is_none() && !active {
                    continue;
                }
                let opened = (|| -> Result<()> {
                    if ctx.is_none() {
                        ctx = Some(Ctx {
                            store: Store::open_unobserved(&root)
                                .context("opening app reader store")?,
                            herdr: herdr.clone(),
                            resource_probe: Arc::clone(&probe),
                        });
                    }
                    Ok(())
                })();
                if let Err(error) = opened {
                    let message = format!("{error:#}");
                    if let Some(job) = job {
                        reject(job, &message);
                    }
                    fail_watches(&worker, &message);
                    continue;
                }
                let Some(ctx) = ctx.as_mut() else {
                    continue;
                };
                let refreshed = ctx.reload_config().map(|changed| {
                    if changed {
                        generation = generation.wrapping_add(1);
                    }
                });
                if let Some(job) = job {
                    match &refreshed {
                        Ok(()) => serve(ctx, job),
                        Err(error) => reject(job, &format!("{error:#}")),
                    }
                }
                let result =
                    refreshed.and_then(|()| refresh(ctx, &worker, generation, &mut revision));
                if let Err(error) = result {
                    fail_watches(&worker, &format!("{error:#}"));
                }
            }
        })
        .context("starting app read worker")?;
    *shared.views.lock().unwrap_or_else(|e| e.into_inner()) = Some(service);
    Ok(())
}

fn bounded<T: serde::Serialize>(value: T) -> Result<T> {
    // Reserve space for the JSON-RPC response envelope, including a bounded request id.
    ensure!(
        serde_json::to_vec(&value)?.len() <= app::APP_CONTROL_MAX - 2048,
        "too_large: app read page exceeds CONTROL budget"
    );
    Ok(value)
}

fn serve(ctx: &Ctx, job: Job) {
    match job {
        Job::Detail(query, reply) => {
            let _ = reply.send(app_read::read(ctx, &query).and_then(bounded));
        }
        Job::Changes(query, seen, reply) => {
            let _ = reply.send(app_read::changes(ctx, &query, &seen).and_then(bounded));
        }
        Job::Thread(scope, before, reply) => {
            let result = corral_ops::agent::app_runtime::thread(ctx, &scope, before, 100).and_then(
                |(rows, next, head)| {
                    ensure!(rows.len() <= 100, "thread reader exceeded row limit");
                    ensure!(
                        next.is_none() || before.is_none() || next < before,
                        "thread cursor did not advance"
                    );
                    Ok(ThreadPage { rows, next, head })
                },
            );
            let _ = reply.send(result);
        }
    }
}

fn reject(job: Job, message: &str) {
    match job {
        Job::Detail(_, reply) => {
            let _ = reply.send(Err(anyhow!(message.to_owned())));
        }
        Job::Changes(_, _, reply) => {
            let _ = reply.send(Err(anyhow!(message.to_owned())));
        }
        Job::Thread(_, _, reply) => {
            let _ = reply.send(Err(anyhow!(message.to_owned())));
        }
    }
}

fn fail_watches(service: &Service, message: &str) {
    let retired: Vec<_> = {
        let mut entries = service.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries
            .values_mut()
            .map(|entry| {
                entry.checked = None;
                entry.published.replace(Err(message.to_owned()))
            })
            .collect()
    };
    drop(retired);
}

fn refresh(ctx: &Ctx, service: &Service, generation: u64, revision: &mut u64) -> Result<()> {
    refresh_with(ctx, service, generation, revision, rows::read)
}

fn refresh_with(
    ctx: &Ctx,
    service: &Service,
    generation: u64,
    revision: &mut u64,
    mut read: impl FnMut(&Ctx, Topic) -> Result<Rows>,
) -> Result<()> {
    let sequence = app_read::sequence(ctx)?;
    let stamp = (sequence, generation);
    let keys: Vec<Key> = service
        .entries
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(key, entry)| entry.checked != Some(stamp) || entry.overdue(key))
        .map(|(key, _)| key.clone())
        .collect();
    for key in keys {
        let Some((checked_before, overdue, previous)) = ({
            let entries = service.entries.lock().unwrap_or_else(|e| e.into_inner());
            entries.get(&key).map(|entry| {
                (
                    entry.checked,
                    entry.overdue(&key),
                    entry
                        .published
                        .as_ref()
                        .and_then(|result| result.as_ref().ok())
                        .map(Arc::clone),
                )
            })
        }) else {
            continue;
        };
        if let Some((after, config)) = checked_before
            && config == generation
            && !overdue
        {
            let (through, quiet) = ctx.read_step(|snapshot| {
                let view = snapshot.store.view()?;
                Ok((view.seq().0, crate::reader::unchanged(view.as_ref(), after)))
            })?;
            if quiet
                && let Some(entry) = service
                    .entries
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_mut(&key)
                && entry.checked == checked_before
            {
                entry.checked = Some((through, generation));
                continue;
            }
        }
        // The sequence and rows belong to one pinned view. Publish that coherent
        // result even if a writer commits meanwhile; the next refresh catches up.
        let rows = ctx.read_step(|snapshot| {
            let sequence = app_read::sequence(snapshot)?;
            Ok((sequence, read(snapshot, key.topic())?))
        });
        // Compare and destroy outside the publication lock used by connections.
        let mut checked = None;
        let publication = match rows {
            Ok((sequence, rows)) => {
                checked = Some((sequence, generation));
                let changed = previous
                    .as_ref()
                    .is_none_or(|old| old.rows != rows || key.topic() == Topic::Thread);
                if changed {
                    *revision = revision
                        .checked_add(1)
                        .context("app topic revision exhausted")?;
                    Some(Ok(Arc::new(Snapshot {
                        rev: *revision,
                        rows,
                    })))
                } else {
                    None
                }
            }
            Err(error) => Some(Err(format!("{error:#}"))),
        };
        let retired = {
            let mut entries = service.entries.lock().unwrap_or_else(|e| e.into_inner());
            let Some(entry) = entries.get_mut(&key) else {
                continue;
            };
            // A closed/reopened watch starts unchecked and needs its own publication.
            if entry.checked != checked_before {
                continue;
            }
            entry.checked = checked;
            entry.read = Some(Instant::now());
            publication.map(|value| entry.published.replace(value))
        };
        drop(retired);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
