//! One daemon-host sampler shared by every `Topic::Host` subscriber.
//!
//! Sampling runs on its own thread, started by the first subscriber's read and stopped once
//! nobody has read for `IDLE`; requests and pages only clone the newest snapshot. The rate
//! is capped at `HOST_INTERVAL_MS` and the history at `HOST_HISTORY_MAX`, in memory only.
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
};

use corral_protocol::host::{
    HOST_HISTORY_MAX, HOST_INTERVAL_MS, HostCoverage, HostFilesystem, HostMetrics, HostSample,
};

use crate::views::Rows;

mod read;
#[cfg(test)]
mod tests;

/// How long the sampler keeps running after the last subscriber read.
const IDLE: Duration = Duration::from_secs(30);
/// A gap this many intervals long (a stalled thread, a suspended host) ends a rate series.
const MAX_GAP_INTERVALS: u32 = 5;

/// Cumulative counters read at one instant, before any rate is derived.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Counters {
    pub at_ms: u64,
    /// Monotonic time since an arbitrary origin; rates use this, never the wall clock.
    pub mono: Duration,
    /// Busy and total CPU time, in the kernel's ticks.
    pub cpu: Option<(u64, u64)>,
    /// Used and total bytes.
    pub memory: Option<(u64, u64)>,
    /// Received and sent bytes per counted interface.
    pub net: Option<BTreeMap<String, (u64, u64)>>,
    /// Read and written bytes per counted disk.
    pub disk: Option<BTreeMap<String, (u64, u64)>>,
}

/// Turns successive counters into samples: rates from elapsed time and deltas, bounded history.
#[derive(Default)]
pub(crate) struct Tracker {
    prev: Option<Counters>,
    samples: VecDeque<HostSample>,
}

impl Tracker {
    pub(crate) fn push(&mut self, now: Counters) -> &VecDeque<HostSample> {
        let interval = Duration::from_millis(HOST_INTERVAL_MS);
        let prev = self.prev.take().filter(|prev| {
            now.mono
                .checked_sub(prev.mono)
                .is_some_and(|gap| !gap.is_zero() && gap <= interval * MAX_GAP_INTERVALS)
        });
        if prev.is_none() {
            // Without a comparable predecessor the old trend no longer joins this one.
            self.samples.clear();
        }
        let secs = prev
            .as_ref()
            .map_or(0.0, |prev| (now.mono - prev.mono).as_secs_f64());
        let cpu = prev
            .as_ref()
            .and_then(|prev| cpu_percent(prev.cpu?, now.cpu?));
        let (net_rx_bps, net_tx_bps) = prev
            .as_ref()
            .and_then(|prev| rates(prev.net.as_ref()?, now.net.as_ref()?, secs))
            .unzip();
        let (disk_read_bps, disk_write_bps) = prev
            .as_ref()
            .and_then(|prev| rates(prev.disk.as_ref()?, now.disk.as_ref()?, secs))
            .unzip();
        let sample = HostSample {
            at_ms: now.at_ms,
            cpu_percent: cpu,
            memory_used: now.memory.map(|(used, _)| used),
            memory_total: now.memory.map(|(_, total)| total),
            net_rx_bps,
            net_tx_bps,
            disk_read_bps,
            disk_write_bps,
        };
        if self.samples.len() == HOST_HISTORY_MAX {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
        self.prev = Some(now);
        &self.samples
    }
}

/// Busy share between two readings; absent when the counters went backwards or did not move.
pub(crate) fn cpu_percent(prev: (u64, u64), now: (u64, u64)) -> Option<f32> {
    let busy = now.0.checked_sub(prev.0)?;
    let total = now.1.checked_sub(prev.1)?;
    if total == 0 || busy > total {
        return None;
    }
    let percent = busy as f64 * 100.0 / total as f64;
    Some(((percent * 10.0).round() / 10.0) as f32)
}

/// Summed per-second rates. Absent when the set of sources changed (the total would cover
/// other sources than the coverage names), none is left, no time passed, or any counter went
/// backwards (a reset or wrap).
pub(crate) fn rates(
    prev: &BTreeMap<String, (u64, u64)>,
    now: &BTreeMap<String, (u64, u64)>,
    secs: f64,
) -> Option<(u64, u64)> {
    if secs <= 0.0 || now.is_empty() || !prev.keys().eq(now.keys()) {
        return None;
    }
    let mut shared = 0;
    let (mut a, mut b) = (0_u64, 0_u64);
    for (name, (now_a, now_b)) in now {
        let Some((prev_a, prev_b)) = prev.get(name) else {
            continue;
        };
        a = a.checked_add(now_a.checked_sub(*prev_a)?)?;
        b = b.checked_add(now_b.checked_sub(*prev_b)?)?;
        shared += 1;
    }
    (shared > 0).then(|| {
        (
            (a as f64 / secs).round() as u64,
            (b as f64 / secs).round() as u64,
        )
    })
}

/// The newest snapshot and whether a sampling thread is serving it.
pub(crate) struct Sampler {
    inner: Mutex<Inner>,
    wake: Condvar,
}

struct Inner {
    latest: Arc<Rows>,
    /// The newest readings and when they were taken; `None` before the first and when idle.
    newest: Option<(HostMetrics, Instant)>,
    /// The whole seconds of age `latest` carries; 0 while sampling keeps pace.
    served_age: u64,
    demand: Instant,
    running: bool,
    stopped: bool,
}

impl Default for Sampler {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                latest: Arc::new(Rows::Host(vec![empty(None)])),
                newest: None,
                served_age: 0,
                demand: Instant::now(),
                running: false,
                stopped: false,
            }),
            wake: Condvar::new(),
        }
    }
}

impl Sampler {
    /// The newest snapshot. Keeps the sampler running, starting it when idle; never samples.
    pub(crate) fn latest(self: &Arc<Self>, root: &Path) -> Arc<Rows> {
        let mut inner = self.lock();
        inner.demand = Instant::now();
        if !inner.running && !inner.stopped {
            let sampler = Arc::clone(self);
            let root = root.to_path_buf();
            let started = thread::Builder::new()
                .name("corral-host-sampler".into())
                .spawn(move || sampler.run(root));
            // A failed spawn leaves the empty snapshot; the next read tries again.
            inner.running = started.is_ok();
        }
        inner.serve(Instant::now())
    }

    pub(crate) fn stop(&self) {
        self.lock().stopped = true;
        self.wake.notify_all();
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn run(&self, root: PathBuf) {
        let origin = Instant::now();
        let mut probe = read::Probe::new(&root);
        let mut tracker = Tracker::default();
        let interval = Duration::from_millis(HOST_INTERVAL_MS);
        let mut next = Instant::now();
        loop {
            let mut inner = self.lock();
            loop {
                if inner.stopped || inner.demand.elapsed() >= IDLE {
                    inner.running = false;
                    // A later subscriber must not see this run's last values as current.
                    inner.clear();
                    return;
                }
                let wait = next.saturating_duration_since(Instant::now());
                if wait.is_zero() {
                    break;
                }
                inner = self
                    .wake
                    .wait_timeout(inner, wait)
                    .unwrap_or_else(|error| error.into_inner())
                    .0;
            }
            drop(inner);
            let started = Instant::now();
            let counters = probe.counters(origin.elapsed());
            let samples = tracker.push(counters).iter().cloned().collect();
            let metrics = HostMetrics {
                samples,
                filesystem: probe.filesystem(),
                network: probe.network.clone(),
                disk: probe.disk.clone(),
                ..empty(None)
            };
            let mut inner = self.lock();
            if inner.stopped {
                inner.running = false;
                return;
            }
            inner.publish(metrics, started);
            // Measured from this reading, so a late wakeup never brings the next one closer.
            next = started + interval;
        }
    }
}

impl Inner {
    fn clear(&mut self) {
        self.latest = Arc::new(Rows::Host(vec![empty(None)]));
        self.newest = None;
        self.served_age = 0;
    }

    fn publish(&mut self, metrics: HostMetrics, taken: Instant) {
        self.latest = Arc::new(Rows::Host(vec![metrics.clone()]));
        self.newest = Some((metrics, taken));
        self.served_age = 0;
    }

    /// The snapshot to serve at `now`. While sampling keeps pace it is the shared one as
    /// published; once the newest reading is more than a second late (a stalled read) it
    /// carries its age, refreshed once a second, so no client can take it for current.
    fn serve(&mut self, now: Instant) -> Arc<Rows> {
        if let Some((metrics, taken)) = &self.newest {
            let elapsed = now.saturating_duration_since(*taken);
            let late = elapsed.saturating_sub(Duration::from_millis(HOST_INTERVAL_MS));
            let age = if late < Duration::from_secs(1) {
                0
            } else {
                elapsed.as_secs()
            };
            if age != self.served_age {
                let mut aged = metrics.clone();
                aged.age_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
                self.latest = Arc::new(Rows::Host(vec![aged]));
                self.served_age = age;
            }
        }
        Arc::clone(&self.latest)
    }
}

fn empty(filesystem: Option<HostFilesystem>) -> HostMetrics {
    HostMetrics {
        hostname: crate::devices::reach::hostname(),
        os: std::env::consts::OS.into(),
        cpus: thread::available_parallelism()
            .ok()
            .and_then(|count| u32::try_from(count.get()).ok()),
        interval_ms: HOST_INTERVAL_MS,
        age_ms: 0,
        samples: Vec::new(),
        filesystem,
        network: HostCoverage::default(),
        disk: HostCoverage::default(),
    }
}
