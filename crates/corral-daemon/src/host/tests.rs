use std::{
    collections::BTreeMap,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use corral_protocol::host::{HOST_HISTORY_MAX, HOST_INTERVAL_MS};

use super::{Counters, Inner, Tracker, read};
use crate::views::Rows;

fn counters(secs: u64, cpu: (u64, u64), rx: u64, disk_read: u64) -> Counters {
    Counters {
        at_ms: secs * 1000,
        mono: Duration::from_secs(secs),
        cpu: Some(cpu),
        memory: Some((3 << 30, 8 << 30)),
        net: Some(BTreeMap::from([("eth0".into(), (rx, 2 * rx))])),
        disk: Some(BTreeMap::from([("sda".into(), (disk_read, 0))])),
    }
}

#[test]
fn first_sample_has_no_rates_and_later_ones_use_elapsed_time() {
    let mut tracker = Tracker::default();
    let first = tracker.push(counters(10, (100, 1000), 1_000, 0))[0].clone();
    assert_eq!(first.cpu_percent, None);
    assert_eq!(first.net_rx_bps, None);
    assert_eq!(first.disk_read_bps, None);
    assert_eq!(first.memory_used, Some(3 << 30));

    let second = tracker.push(counters(12, (150, 1200), 5_000, 4096))[1].clone();
    assert_eq!(second.cpu_percent, Some(25.0));
    assert_eq!(second.net_rx_bps, Some(2_000));
    assert_eq!(second.net_tx_bps, Some(4_000));
    assert_eq!(second.disk_read_bps, Some(2048));
    assert_eq!(second.disk_write_bps, Some(0));
}

#[test]
fn a_counter_reset_or_long_gap_is_absent_not_zero() {
    let mut tracker = Tracker::default();
    tracker.push(counters(10, (100, 1000), 9_000, 9_000));
    let reset = tracker.push(counters(12, (50, 500), 10, 10))[1].clone();
    assert_eq!(reset.cpu_percent, None);
    assert_eq!(reset.net_rx_bps, None);
    assert_eq!(reset.disk_read_bps, None);

    let gap = 12 + 6 * HOST_INTERVAL_MS / 1000;
    let after_gap = tracker.push(counters(gap, (60, 600), 20, 20));
    assert_eq!(after_gap.len(), 1, "a gap ends the old trend");
    assert_eq!(after_gap[0].net_rx_bps, None);
}

#[test]
fn rates_are_absent_when_the_summed_sources_change() {
    let prev = BTreeMap::from([("a".into(), (100, 0)), ("b".into(), (5, 5))]);
    let same = BTreeMap::from([("a".into(), (300, 0)), ("b".into(), (25, 5))]);
    assert_eq!(super::rates(&prev, &same, 2.0), Some((110, 0)));
    let swapped = BTreeMap::from([("a".into(), (300, 0)), ("new".into(), (999, 999))]);
    assert_eq!(super::rates(&prev, &swapped, 2.0), None);
    assert_eq!(super::rates(&prev, &same, 0.0), None);
}

#[test]
fn history_stays_bounded() {
    let mut tracker = Tracker::default();
    let step = HOST_INTERVAL_MS / 1000;
    let mut len = 0;
    for tick in 0..(HOST_HISTORY_MAX as u64 + 7) {
        let samples = tracker.push(counters(tick * step, (tick, tick * 4), tick, tick));
        len = samples.len();
        assert_eq!(samples.back().map(|s| s.at_ms), Some(tick * step * 1000));
    }
    assert_eq!(len, HOST_HISTORY_MAX);
}

#[test]
fn parsers_read_kernel_text_and_reject_missing_fields() {
    let stat = "cpu  10 2 8 70 10 0 0 0 5 0\ncpu0 1 1 1 1 1 1 1 1\n";
    assert_eq!(read::cpu(stat), Some((20, 100)));
    assert_eq!(read::cpu("intr 1 2 3\n"), None);

    let meminfo = "MemTotal:  1000 kB\nMemFree: 100 kB\nMemAvailable:  250 kB\n";
    assert_eq!(read::memory(meminfo), Some((750 * 1024, 1000 * 1024)));
    assert_eq!(read::memory("MemTotal: 1000 kB\n"), None);

    let dev = "Inter-|   Receive\n face |bytes\n    lo: 50 1 0 0 0 0 0 0 60 1 0 0 0 0 0 0\n  eth0: 1000 9 0 0 0 0 0 0 2000 9 0 0 0 0 0 0\n";
    let rows = read::net(dev, |name| name != "lo");
    assert_eq!(rows, BTreeMap::from([("eth0".into(), (1000, 2000))]));

    let diskstats =
        "   8       0 sda 10 0 4 0 20 0 8 0 0 0 0\n   8       1 sda1 10 0 4 0 20 0 8 0 0 0 0\n";
    let rows = read::disks(diskstats, |name| name == "sda");
    assert_eq!(rows, BTreeMap::from([("sda".into(), (2048, 4096))]));

    let mountinfo = "22 1 8:1 / / rw - ext4 /dev/sda1 rw\n30 22 8:2 / /home/my\\040files rw - ext4 /dev/sda2 rw\n";
    let path = Path::new("/home/my files/work");
    assert_eq!(
        read::mount_of(mountinfo, path, (8, 2)).as_deref(),
        Some("/home/my files")
    );
    assert_eq!(
        read::mount_of(mountinfo, Path::new("/var"), (8, 1)).as_deref(),
        Some("/")
    );
    // Another filesystem now answers for the path: no label rather than the old one's.
    assert_eq!(read::mount_of(mountinfo, path, (8, 9)), None);
}

#[test]
fn a_stalled_reading_is_served_with_its_age_not_as_current() {
    let taken = Instant::now();
    let mut inner = Inner {
        latest: Arc::new(Rows::Host(Vec::new())),
        blank: Arc::new(Rows::Host(Vec::new())),
        newest: None,
        served_age: 0,
        demand: taken,
        running: true,
        stopped: false,
    };
    let mut metrics = super::empty(None);
    metrics
        .samples
        .push(super::Tracker::default().push(counters(1, (1, 2), 1, 1))[0].clone());
    inner.publish(metrics, taken);
    let interval = Duration::from_millis(HOST_INTERVAL_MS);
    let on_time = inner.serve(taken + interval);
    assert!(
        Arc::ptr_eq(&on_time, &inner.serve(taken + interval)),
        "shared while on time"
    );
    let age = |rows: &Rows| match rows {
        Rows::Host(rows) => rows[0].age_ms,
        _ => unreachable!(),
    };
    assert_eq!(age(&on_time), 0);
    let stalled = inner.serve(taken + Duration::from_secs(8));
    assert_eq!(age(&stalled), 8_000);
    assert!(Arc::ptr_eq(
        &stalled,
        &inner.serve(taken + Duration::from_millis(8_400))
    ));
}

/// Reads the real counters twice without generating load: the probe only reads files.
#[cfg(target_os = "linux")]
#[test]
fn real_counters_smoke() {
    let root = std::env::temp_dir();
    let mut probe = read::Probe::new(&root);
    let mut tracker = Tracker::default();
    tracker.push(probe.counters(Duration::from_millis(0)));
    std::thread::sleep(Duration::from_millis(300));
    let samples = tracker.push(probe.counters(Duration::from_millis(300)));
    let sample = &samples[1];
    let (used, total) = (sample.memory_used.unwrap(), sample.memory_total.unwrap());
    assert!(used <= total && total > 0);
    assert!(
        sample
            .cpu_percent
            .is_none_or(|cpu| (0.0..=100.0).contains(&cpu))
    );
    let filesystem = probe.filesystem().expect("statvfs of the temp dir");
    assert!(filesystem.used <= filesystem.total && filesystem.total > 0);
    assert!(filesystem.mount.is_some());
    assert!(!probe.network.rule.is_empty() || probe.network.unavailable.is_some());
}
