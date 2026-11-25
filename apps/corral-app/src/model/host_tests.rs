use corral_protocol::host::{HOST_V1, HostCoverage, HostFilesystem, HostMetrics, HostSample};

use super::*;
use crate::model::daemon::{Feed, Link};

fn sample(at: u64, cpu: Option<f32>, rx: Option<u64>) -> HostSample {
    HostSample {
        at_ms: at * 1000,
        cpu_percent: cpu,
        memory_used: Some(4 << 30),
        memory_total: Some(16 << 30),
        net_rx_bps: rx,
        net_tx_bps: rx,
        disk_read_bps: None,
        disk_write_bps: None,
    }
}

fn feed(samples: Vec<HostSample>) -> Feed {
    let mut feed = Feed {
        link: Link::Up,
        now: 1_000,
        ..Feed::default()
    };
    feed.capabilities.insert(HOST_V1.into());
    feed.daemon_host = Some(HostMetrics {
        hostname: Some("build-box".into()),
        os: "linux".into(),
        cpus: Some(8),
        interval_ms: 2_000,
        samples,
        filesystem: Some(HostFilesystem {
            path: "/srv/work".into(),
            mount: Some("/srv".into()),
            total: 100 << 30,
            used: 40 << 30,
            free: 60 << 30,
        }),
        network: HostCoverage {
            sources: vec!["eth0".into()],
            rule: "Physical interfaces only.".into(),
            unavailable: None,
        },
        disk: HostCoverage {
            sources: Vec::new(),
            rule: String::new(),
            unavailable: Some("Not sampled on this operating system.".into()),
        },
    });
    feed.daemon_host_at = Some(1_000);
    feed
}

#[test]
fn an_older_daemon_is_not_served_rather_than_zero() {
    let mut feed = Feed {
        link: Link::Up,
        ..Feed::default()
    };
    feed.capabilities.insert(corral_protocol::TASKS_V1.into());
    let view = view(&feed);
    assert_eq!(view.standing, Standing::NotServed);
    assert!(view.metrics.iter().all(|m| m.value.is_none() && m.absent.is_some()));
}

#[test]
fn the_first_sample_waits_and_missing_counters_say_why() {
    let view = view(&feed(vec![sample(998, None, None)]));
    assert_eq!(view.standing, Standing::Waiting);
    let cpu = view.metric(Kind::Cpu).unwrap();
    assert_eq!(cpu.value, None);
    assert!(cpu.absent.is_some());
    let io = view.metric(Kind::DiskIo).unwrap();
    assert_eq!(io.value, None);
    assert_eq!(io.absent.as_deref(), Some("Not sampled on this operating system."));
    assert!(view.metric(Kind::Memory).unwrap().value.is_some());
}

#[test]
fn a_gap_in_the_trend_stays_a_gap() {
    let view = view(&feed(vec![
        sample(994, Some(10.0), Some(100)),
        sample(996, None, None),
        sample(998, Some(30.0), Some(300)),
    ]));
    assert_eq!(view.standing, Standing::Live);
    let net = view.metric(Kind::Network).unwrap();
    assert_eq!(net.series[0].points, vec![Some(100.0), None, Some(300.0)]);
    assert!(net.value.is_some());
}

#[test]
fn readings_that_stop_arriving_or_a_lost_link_are_stale() {
    let samples = vec![sample(994, Some(5.0), Some(1)), sample(996, Some(6.0), Some(1))];
    let mut late = feed(samples.clone());
    late.now = 1_030;
    assert_eq!(
        view(&late).standing,
        Standing::Stale { age_secs: Some(30) }
    );
    let mut down = feed(samples);
    down.link = Link::Down;
    let shown = view(&down);
    assert!(shown.stale());
    assert!(compact(&shown).ends_with("stale"));
}
