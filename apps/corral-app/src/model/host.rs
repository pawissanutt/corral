//! Host performance: the machine running corrald (`host.v1`), never this device.
//!
//! Every figure is host-wide: the project scope does not change it, so nothing here reads the
//! scope. A figure corrald does not report, a rate without two comparable samples, and a
//! reading that stopped arriving are said as such, never drawn as zero or as current.

use corral_protocol::host::{HOST_V1, HostCoverage, HostMetrics, HostSample};

use crate::model::daemon::{Feed, Link};
use crate::model::health::{Availability, capability_availability};

pub const TITLE: &str = "Host performance";
/// Under every title: whose machine this is and that the scope does not apply.
pub const SCOPE_NOTE: &str =
    "The machine running corrald, host-wide; the project scope does not change these figures.";
pub const OPEN: &str = "Open Host performance";
/// A reading older than this many sample intervals is stale.
const STALE_INTERVALS: u64 = 3;

/// Where the host's readings stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// This corrald does not offer `host.v1` (an older daemon).
    NotServed,
    /// No session has negotiated yet, or the first page has not arrived.
    Connecting,
    /// corrald answered but has not taken two samples yet.
    Waiting,
    /// Fresh readings.
    Live,
    /// The newest reading is older than it should be, or the link is down; values are the
    /// last ones received, shown as such.
    Stale { age_secs: Option<u64> },
}

/// Which figure a [`Metric`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Cpu,
    Memory,
    /// Capacity of the workspace's filesystem, not its I/O.
    Space,
    Network,
    /// Disk reads and writes per second, not capacity.
    DiskIo,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Cpu,
        Kind::Memory,
        Kind::Space,
        Kind::Network,
        Kind::DiskIo,
    ];
    /// Test-id slug (`host-<slug>`).
    pub const fn slug(self) -> &'static str {
        match self {
            Kind::Cpu => "cpu",
            Kind::Memory => "memory",
            Kind::Space => "space",
            Kind::Network => "network",
            Kind::DiskIo => "disk-io",
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            Kind::Cpu => "CPU",
            Kind::Memory => "Memory",
            Kind::Space => "Disk space",
            Kind::Network => "Network",
            Kind::DiskIo => "Disk I/O",
        }
    }
    /// The terminal's short label.
    pub const fn short(self) -> &'static str {
        match self {
            Kind::Cpu => "CPU",
            Kind::Memory => "RAM",
            Kind::Space => "Space",
            Kind::Network => "Net",
            Kind::DiskIo => "I/O",
        }
    }
}

/// One trend line, oldest first; `None` is a gap (no reading), never zero.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub label: &'static str,
    pub points: Vec<Option<f64>>,
    /// The top of the scale: 100 for a percentage, the total for memory, else the largest
    /// point (at least 1).
    pub max: f64,
}

/// One figure as every form shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Metric {
    pub kind: Kind,
    /// The headline value, absent when there is none.
    pub value: Option<String>,
    /// The second line: totals, the split of a rate.
    pub detail: Option<String>,
    /// Why there is no value.
    pub absent: Option<String>,
    /// How full, 0 to 1, for a gauge.
    pub share: Option<f32>,
    pub series: Vec<Series>,
    /// What the figure covers: the sampled path, the interfaces or disks summed.
    pub note: Option<String>,
}

/// Everything Host performance shows.
#[derive(Debug, Clone, PartialEq)]
pub struct HostView {
    pub standing: Standing,
    /// "corrald host <name> · linux · 8 CPUs".
    pub host_line: String,
    /// How fresh the readings are, in one line.
    pub status: String,
    pub metrics: Vec<Metric>,
    /// Seconds between two readings, once corrald has said.
    pub interval_secs: Option<u64>,
}

/// What a trend chart's readout says until a reading is pointed at.
pub const PROMPT: &str = "Point at a trend for its reading.";
/// Under a trend's newest end.
pub const NEWEST: &str = "now";
/// Beside the host in a compact form, where the full note does not fit.
pub const HOST_WIDE: &str = "host-wide, not scoped";
/// What the absent value shows in a figure's place: a dash, its reason beside it.
pub const NO_VALUE: &str = "—";
/// What leads [`compact`]'s line on a terminal's Home.
pub const LINE_LABEL: &str = "corrald host";
/// Beside a value drawn from readings that stopped arriving.
pub const STALE: &str = "stale";

impl HostView {
    pub fn metric(&self, kind: Kind) -> Option<&Metric> {
        self.metrics.iter().find(|metric| metric.kind == kind)
    }
    /// Whether values are drawn as the last received rather than current.
    pub fn stale(&self) -> bool {
        matches!(self.standing, Standing::Stale { .. })
    }
}

/// Where the readings stand, from the capability, the link and when the last page arrived.
pub fn standing(feed: &Feed) -> Standing {
    match capability_availability(feed, HOST_V1) {
        Availability::Missing(_) => return Standing::NotServed,
        Availability::Connecting if feed.daemon_host.is_none() => return Standing::Connecting,
        _ => {}
    }
    let Some(metrics) = &feed.daemon_host else {
        return if feed.link == Link::Down {
            Standing::Stale { age_secs: None }
        } else {
            Standing::Connecting
        };
    };
    // A reset, an error or a reopened topic leaves the retained row: not current until a page.
    if feed
        .topics
        .get("host")
        .is_some_and(|topic| topic.stale || topic.error.is_some())
    {
        let age = feed.daemon_host_at.map(|at| feed.now.saturating_sub(at));
        return Standing::Stale { age_secs: age };
    }
    let age = feed.daemon_host_at.map(|at| feed.now.saturating_sub(at));
    let limit = (metrics.interval_ms * STALE_INTERVALS)
        .div_ceil(1000)
        .max(1);
    if feed.link != Link::Up || age.is_none_or(|age| age > limit) {
        return Standing::Stale { age_secs: age };
    }
    if metrics.samples.len() < 2 {
        return Standing::Waiting;
    }
    Standing::Live
}

/// Host performance from the feed. Never reads the project scope.
pub fn view(feed: &Feed) -> HostView {
    let standing = standing(feed);
    let metrics = feed.daemon_host.as_ref();
    let host_line = metrics.map_or_else(
        || "corrald host".to_owned(),
        |m| {
            let mut line = format!(
                "corrald host {}",
                m.hostname.as_deref().unwrap_or("(unnamed)")
            );
            line.push_str(&format!(" · {}", m.os));
            if let Some(cpus) = m.cpus {
                line.push_str(&format!(" · {cpus} CPUs"));
            }
            line
        },
    );
    let status = match &standing {
        Standing::NotServed => {
            "This corrald does not report host performance; update it to see these figures."
                .to_owned()
        }
        Standing::Connecting => "Connecting to corrald…".to_owned(),
        Standing::Waiting => "Sampling; rates show after the second reading.".to_owned(),
        Standing::Live => metrics.map_or_else(String::new, |m| {
            format!("Live · every {} s", (m.interval_ms / 1000).max(1))
        }),
        Standing::Stale { age_secs } => {
            let why = if feed.link == Link::Up {
                "no new reading"
            } else {
                "corrald is not connected"
            };
            match age_secs {
                Some(age) => format!("Stale · {why}; last reading {} ago", ago(*age)),
                None => format!("Stale · {why}"),
            }
        }
    };
    let waiting = match standing {
        Standing::NotServed => "Not reported by this corrald.",
        _ => "No reading yet.",
    }
    .to_owned();
    let metrics = match (&standing, metrics) {
        (Standing::NotServed | Standing::Connecting, _) | (_, None) => Kind::ALL
            .into_iter()
            .map(|kind| absent(kind, Some(waiting.clone())))
            .collect(),
        (_, Some(m)) => Kind::ALL.into_iter().map(|kind| metric(m, kind)).collect(),
    };
    HostView {
        standing,
        host_line,
        status,
        metrics,
        interval_secs: feed
            .daemon_host
            .as_ref()
            .map(|m| (m.interval_ms / 1000).max(1)),
    }
}

/// The span the trends cover, for their caption: "Last 5 min". None before two readings.
pub fn window(view: &HostView) -> Option<String> {
    let interval = view.interval_secs?;
    let count = view
        .metrics
        .iter()
        .flat_map(|m| m.series.iter())
        .map(|s| s.points.len())
        .max()?;
    (count >= 2).then(|| format!("Last {}", ago(interval * (count as u64 - 1))))
}

/// A trend's readout for reading `at` (oldest first): how long ago, then each line's value,
/// a gap said as no reading rather than zero.
pub fn reading(view: &HostView, metric: &Metric, at: usize) -> String {
    let count = metric
        .series
        .iter()
        .map(|s| s.points.len())
        .max()
        .unwrap_or(0);
    let back = count.saturating_sub(at + 1) as u64;
    let when = match view.interval_secs {
        Some(_) if back == 0 => "Newest reading".to_owned(),
        Some(interval) => format!("{} earlier", ago(back * interval)),
        None => format!("Reading {} of {count}", at + 1),
    };
    let values = metric
        .series
        .iter()
        .map(|s| match s.points.get(at).copied().flatten() {
            Some(value) => format!("{} {}", s.label, figure(metric.kind, value)),
            None => format!("{} no reading", s.label),
        });
    std::iter::once(when)
        .chain(values)
        .collect::<Vec<_>>()
        .join(" · ")
}

/// What a trend says to a screen reader: its span and its newest reading.
pub fn spoken(view: &HostView, metric: &Metric) -> String {
    let newest = metric
        .series
        .iter()
        .map(|s| s.points.len())
        .max()
        .unwrap_or(0);
    let span = window(view).unwrap_or_else(|| "Recent".to_owned());
    if newest == 0 {
        return format!("{} trend: no readings.", metric.kind.label());
    }
    format!(
        "{} trend, {}. {}.",
        metric.kind.label(),
        span.to_lowercase(),
        reading(view, metric, newest - 1)
    )
}

/// A value of `kind` as a trend reads it, on its scale and in its readout: CPU in percent,
/// memory and space in bytes, network and disk I/O per second.
pub fn figure(kind: Kind, value: f64) -> String {
    let whole = value.max(0.0).round() as u64;
    match kind {
        Kind::Cpu => format!("{value:.0} %"),
        Kind::Memory | Kind::Space => bytes(whole),
        Kind::Network | Kind::DiskIo => rate(whole),
    }
}

/// The terminal's one-line fallback: every figure, numbers only.
pub fn compact(view: &HostView) -> String {
    let mut parts: Vec<String> = view
        .metrics
        .iter()
        .map(|m| format!("{} {}", m.kind.short(), m.value.as_deref().unwrap_or("—")))
        .collect();
    if view.stale() {
        parts.push("stale".into());
    }
    parts.join(" · ")
}

fn metric(m: &HostMetrics, kind: Kind) -> Metric {
    let newest = m.samples.last();
    match kind {
        Kind::Cpu => {
            let cpu = newest.and_then(|s| s.cpu_percent);
            Metric {
                value: cpu.map(|cpu| format!("{cpu:.0} %")),
                share: cpu.map(|cpu| (cpu / 100.0).clamp(0.0, 1.0)),
                series: vec![series(m, "Busy", Some(100.0), |s| {
                    s.cpu_percent.map(f64::from)
                })],
                note: m.cpus.map(|cpus| format!("Busy share of all {cpus} CPUs.")),
                ..absent(
                    kind,
                    why_rate(m, cpu.is_none(), |s| s.cpu_percent.is_some()),
                )
            }
        }
        Kind::Memory => {
            let pair = newest.and_then(|s| Some((s.memory_used?, s.memory_total?)));
            Metric {
                value: pair.map(|(used, _)| bytes(used)),
                detail: pair.map(|(used, total)| {
                    format!(
                        "of {} · {} available",
                        bytes(total),
                        bytes(total - used.min(total))
                    )
                }),
                share: pair.and_then(|(used, total)| share(used, total)),
                // Against the whole memory, as the gauge, so a steady 30 % does not draw as full.
                series: vec![series(
                    m,
                    "Used",
                    pair.map(|(_, total)| total as f64),
                    |s| s.memory_used.map(|v| v as f64),
                )],
                note: Some("Used is total less what the kernel reports as available.".into()),
                ..absent(kind, pair.is_none().then(|| unreported(m)))
            }
        }
        Kind::Space => match &m.filesystem {
            Some(fs) => Metric {
                value: Some(format!("{} free", bytes(fs.free))),
                detail: Some(format!("{} used of {}", bytes(fs.used), bytes(fs.total))),
                share: share(fs.used, fs.total),
                note: Some(match &fs.mount {
                    Some(mount) => format!("Capacity of {mount}, which holds {}.", fs.path),
                    None => format!("Capacity of the filesystem holding {}.", fs.path),
                }),
                ..absent(kind, None)
            },
            None => absent(
                kind,
                Some(if m.samples.is_empty() {
                    "Not sampled yet.".into()
                } else {
                    "corrald could not read its workspace filesystem.".into()
                }),
            ),
        },
        Kind::Network => rates(
            m,
            kind,
            &m.network,
            ("Receive", |s: &HostSample| s.net_rx_bps),
            ("Send", |s: &HostSample| s.net_tx_bps),
            ("↓", "↑"),
        ),
        Kind::DiskIo => rates(
            m,
            kind,
            &m.disk,
            ("Read", |s: &HostSample| s.disk_read_bps),
            ("Write", |s: &HostSample| s.disk_write_bps),
            ("read", "write"),
        ),
    }
}

type Pick = fn(&HostSample) -> Option<u64>;

fn rates(
    m: &HostMetrics,
    kind: Kind,
    coverage: &HostCoverage,
    (a_label, a): (&'static str, Pick),
    (b_label, b): (&'static str, Pick),
    (a_word, b_word): (&str, &str),
) -> Metric {
    let newest = m.samples.last();
    let pair = newest.and_then(|s| Some((a(s)?, b(s)?)));
    let why = coverage
        .unavailable
        .clone()
        .or_else(|| why_rate(m, pair.is_none(), |s| a(s).is_some() || b(s).is_some()));
    let note = (!coverage.rule.is_empty()).then(|| {
        if coverage.sources.is_empty() {
            coverage.rule.clone()
        } else {
            format!(
                "{} ({}) {}",
                label_sources(kind),
                coverage.sources.join(", "),
                coverage.rule
            )
        }
    });
    Metric {
        kind,
        value: pair.map(|(x, y)| format!("{a_word} {} {b_word} {}", rate(x), rate(y))),
        detail: None,
        absent: if pair.is_some() { None } else { why },
        share: None,
        series: vec![
            series(m, a_label, None, |s| a(s).map(|v| v as f64)),
            series(m, b_label, None, |s| b(s).map(|v| v as f64)),
        ],
        note,
    }
}

fn label_sources(kind: Kind) -> &'static str {
    if kind == Kind::Network {
        "Sums"
    } else {
        "Sums disks"
    }
}

fn absent(kind: Kind, why: Option<String>) -> Metric {
    Metric {
        kind,
        value: None,
        detail: None,
        absent: why,
        share: None,
        series: Vec::new(),
        note: None,
    }
}

/// Why a rate is absent: no readings yet, only one, never reported by this host, or the
/// newest pair was not comparable (a counter reset or a pause).
fn why_rate(
    m: &HostMetrics,
    missing: bool,
    reported: impl Fn(&HostSample) -> bool,
) -> Option<String> {
    if !missing {
        return None;
    }
    Some(match m.samples.len() {
        0 => "Not sampled yet.".into(),
        1 => "Needs a second reading.".into(),
        _ if !m.samples.iter().any(reported) => unreported(m),
        _ => "No comparable reading: a counter reset or sampling paused.".into(),
    })
}

fn unreported(m: &HostMetrics) -> String {
    format!("corrald does not read this on {}.", m.os)
}

fn series(
    m: &HostMetrics,
    label: &'static str,
    max: Option<f64>,
    pick: impl Fn(&HostSample) -> Option<f64>,
) -> Series {
    let points: Vec<Option<f64>> = m.samples.iter().map(pick).collect();
    let top = max.unwrap_or_else(|| points.iter().flatten().copied().fold(1.0, f64::max));
    Series {
        label,
        points,
        max: top,
    }
}

fn share(used: u64, total: u64) -> Option<f32> {
    (total > 0).then(|| (used as f64 / total as f64).clamp(0.0, 1.0) as f32)
}

/// Binary units: `5.1 GiB`, `812 MiB`, `0 B`.
pub fn bytes(value: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut amount = value as f64;
    let mut unit = 0;
    while amount >= 1024.0 && unit + 1 < UNITS.len() {
        amount /= 1024.0;
        unit += 1;
    }
    if unit == 0 || amount >= 100.0 {
        format!("{amount:.0} {}", UNITS[unit])
    } else {
        format!("{amount:.1} {}", UNITS[unit])
    }
}

/// A rate in binary units per second.
pub fn rate(per_second: u64) -> String {
    format!("{}/s", bytes(per_second))
}

fn ago(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs} s"),
        60..3600 => format!("{} min", secs / 60),
        _ => format!("{} h", secs / 3600),
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
