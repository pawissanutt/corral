//! The sample's daemon host: a made-up machine whose figures move with the clock, so Host
//! performance has a trend to draw. Every reading is labelled as the preview's.

use corral_protocol::host::{
    HOST_HISTORY_MAX, HOST_INTERVAL_MS, HostCoverage, HostFilesystem, HostMetrics, HostSample,
};

use crate::model::daemon::Feed;

const GIB: u64 = 1 << 30;

pub(super) fn populate(feed: &mut Feed) {
    let step = HOST_INTERVAL_MS / 1000;
    let newest = feed.now - feed.now % step;
    let samples = (0..60_u64)
        .rev()
        .map(|back| sample(newest.saturating_sub(back * step)))
        .collect();
    feed.daemon_host = Some(HostMetrics {
        hostname: Some("preview-host".into()),
        os: "linux".into(),
        cpus: Some(16),
        interval_ms: HOST_INTERVAL_MS,
        age_ms: 0,
        samples,
        filesystem: Some(HostFilesystem {
            path: "/preview/corral".into(),
            mount: Some("/preview".into()),
            total: 512 * GIB,
            used: 301 * GIB,
            free: 186 * GIB,
        }),
        network: HostCoverage {
            sources: vec!["eth0".into()],
            rule: "Preview · physical interfaces only.".into(),
            unavailable: None,
        },
        disk: HostCoverage {
            sources: vec!["nvme0n1".into()],
            rule: "Preview · whole disks only.".into(),
            unavailable: None,
        },
    });
    feed.daemon_host_at = Some(feed.now);
}

/// One clock tick: a new reading once per sample interval.
pub(super) fn tick(feed: &mut Feed) {
    let step = HOST_INTERVAL_MS / 1000;
    let now = feed.now;
    let Some(host) = feed.daemon_host.as_mut() else {
        return;
    };
    let newest = host.samples.last().map_or(0, |s| s.at_ms / 1000);
    if now >= newest + step {
        host.samples.push(sample(now - now % step));
        let extra = host.samples.len().saturating_sub(HOST_HISTORY_MAX);
        host.samples.drain(..extra);
    }
    feed.daemon_host_at = Some(now);
}

/// A reading at `at` seconds: smooth waves, so neighbouring readings agree.
fn sample(at: u64) -> HostSample {
    let t = at as f64 / 20.0;
    let wave = |phase: f64| (t + phase).sin() * 0.5 + 0.5;
    HostSample {
        at_ms: at * 1000,
        cpu_percent: Some((12.0 + 55.0 * wave(0.0) * wave(1.7)) as f32),
        memory_used: Some((9.0 * GIB as f64 + 3.0 * GIB as f64 * wave(0.4)) as u64),
        memory_total: Some(32 * GIB),
        net_rx_bps: Some((40_000.0 + 2_400_000.0 * wave(2.1).powi(3)) as u64),
        net_tx_bps: Some((12_000.0 + 380_000.0 * wave(0.9).powi(2)) as u64),
        disk_read_bps: Some((150_000.0 * wave(3.0).powi(4)) as u64),
        disk_write_bps: Some((60_000.0 + 2_000_000.0 * wave(1.2).powi(5)) as u64),
    }
}
