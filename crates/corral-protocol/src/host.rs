//! Daemon-host performance (`host.v1`): the machine running corrald, never the client's.
//!
//! Every figure is host-wide and independent of any project scope. A figure the daemon
//! cannot read, or a rate without two comparable samples, is absent, never zero.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Grants `Topic::Host`.
pub const HOST_V1: &str = "host.v1";
/// The fastest the daemon samples, whatever the number of watchers.
pub const HOST_INTERVAL_MS: u64 = 2_000;
/// The most samples one page carries, oldest first.
pub const HOST_HISTORY_MAX: usize = 90;

/// The one row of `Topic::Host`: the newest samples and what they cover.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HostMetrics {
    /// The daemon host's name, absent when the system does not report one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    /// The daemon host's operating system (`linux`, `macos`, ...).
    pub os: String,
    /// Logical CPUs, absent when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u32>,
    /// Milliseconds between samples.
    pub interval_ms: u64,
    /// How old the newest sample was when corrald served this page, by corrald's own clock.
    /// Zero while sampling keeps pace; a client counts a reading's age from it, never from
    /// when the page arrived.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub age_ms: u64,
    /// Oldest first, at most `HOST_HISTORY_MAX`; empty before the first sample.
    pub samples: Vec<HostSample>,
    /// The filesystem holding the daemon's workspace root, absent when it cannot be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filesystem: Option<HostFilesystem>,
    /// Which interfaces the network rates sum.
    pub network: HostCoverage,
    /// Which block devices the disk I/O rates sum.
    pub disk: HostCoverage,
}

/// One sample. Rates are bytes per second over the time since the previous sample.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HostSample {
    /// Daemon-host wall clock, milliseconds since the Unix epoch.
    pub at_ms: u64,
    /// Busy share of all CPUs since the previous sample, 0 to 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_percent: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_used: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_total: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_rx_bps: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_tx_bps: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_read_bps: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_write_bps: Option<u64>,
}

/// Capacity of the filesystem the workspace lives on (not I/O).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostFilesystem {
    /// The daemon's workspace root, the path sampled.
    pub path: String,
    /// Where that filesystem is mounted, absent when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
    pub total: u64,
    pub used: u64,
    /// Available to unprivileged users.
    pub free: u64,
}

/// What a summed rate covers and why.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostCoverage {
    /// Interface or device names summed in the newest sample.
    pub sources: Vec<String>,
    /// Which sources count, in one sentence.
    pub rule: String,
    /// Why the rates are absent when the host cannot report them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}
