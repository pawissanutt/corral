//! Reading the host's counters: pure parsers over kernel text, and the probe that feeds them.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use corral_protocol::host::{HostCoverage, HostFilesystem};

use super::Counters;

/// `/proc/diskstats` counts 512-byte sectors whatever the device's own sector size.
const SECTOR: u64 = 512;

pub(super) const NET_RULE: &str = "Physical interfaces only (those backed by a device); \
     loopback and virtual interfaces such as bridges, veth pairs, tunnels and VPNs are left \
     out so traffic is counted once.";
pub(super) const NET_RULE_FALLBACK: &str = "No physical interface is visible, so every \
     interface except loopback is summed; virtual interfaces may count traffic twice.";
pub(super) const DISK_RULE: &str = "Whole disks backed by a device; partitions, loop, RAM \
     and device-mapper devices are left out so I/O is counted once.";
const UNSUPPORTED: &str = "Not sampled on this operating system.";

/// Busy and total CPU ticks from the aggregate `cpu` line of `/proc/stat`.
/// Guest time is already inside user and nice, so only the first eight fields add up.
pub(crate) fn cpu(stat: &str) -> Option<(u64, u64)> {
    let line = stat.lines().find(|line| line.starts_with("cpu "))?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    if fields.len() < 4 {
        return None;
    }
    let total = fields
        .iter()
        .try_fold(0_u64, |sum, v| sum.checked_add(*v))?;
    let idle = fields[3].saturating_add(fields.get(4).copied().unwrap_or(0));
    Some((total.checked_sub(idle)?, total))
}

/// Used and total bytes from `/proc/meminfo`; used is total less `MemAvailable`.
pub(crate) fn memory(meminfo: &str) -> Option<(u64, u64)> {
    let field = |name: &str| {
        meminfo.lines().find_map(|line| {
            let rest = line.strip_prefix(name)?.strip_prefix(':')?;
            let kib: u64 = rest.split_whitespace().next()?.parse().ok()?;
            kib.checked_mul(1024)
        })
    };
    let total = field("MemTotal")?;
    let available = field("MemAvailable")?;
    Some((total.checked_sub(available)?, total))
}

/// Received and sent bytes per interface in `/proc/net/dev` that `keep` accepts.
pub(crate) fn net(dev: &str, keep: impl Fn(&str) -> bool) -> BTreeMap<String, (u64, u64)> {
    dev.lines()
        .filter_map(|line| {
            let (name, rest) = line.split_once(':')?;
            let name = name.trim();
            if !keep(name) {
                return None;
            }
            let fields: Vec<&str> = rest.split_whitespace().collect();
            let rx = fields.first()?.parse().ok()?;
            let tx = fields.get(8)?.parse().ok()?;
            Some((name.to_owned(), (rx, tx)))
        })
        .collect()
}

/// Read and written bytes per device in `/proc/diskstats` that `keep` accepts.
pub(crate) fn disks(stats: &str, keep: impl Fn(&str) -> bool) -> BTreeMap<String, (u64, u64)> {
    stats
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let name = *fields.get(2)?;
            if !keep(name) {
                return None;
            }
            let read: u64 = fields.get(5)?.parse().ok()?;
            let written: u64 = fields.get(9)?.parse().ok()?;
            Some((
                name.to_owned(),
                (read.checked_mul(SECTOR)?, written.checked_mul(SECTOR)?),
            ))
        })
        .collect()
}

/// The mount point in `/proc/self/mountinfo` that holds `path` on device `dev` (major, minor):
/// the longest one containing it. None when the longest is another device's, so a label never
/// names a filesystem other than the one measured.
pub(crate) fn mount_of(mountinfo: &str, path: &Path, dev: (u32, u32)) -> Option<String> {
    let device = format!("{}:{}", dev.0, dev.1);
    let (mount, id) = mountinfo
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(' ');
            let id = fields.nth(2)?;
            let mount = unescape(fields.nth(1)?);
            Some((mount, id))
        })
        .filter(|(mount, _)| path.starts_with(mount))
        .max_by_key(|(mount, _)| mount.len())?;
    (id == device).then_some(mount)
}

/// Mountinfo escapes space, tab, newline and backslash as three octal digits.
fn unescape(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut rest = field;
    while let Some(at) = rest.find('\\') {
        out.push_str(&rest[..at]);
        let code = rest.get(at + 1..at + 4);
        match code.and_then(|code| u8::from_str_radix(code, 8).ok()) {
            Some(byte) => {
                out.push(char::from(byte));
                rest = &rest[at + 4..];
            }
            None => {
                out.push('\\');
                rest = &rest[at + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Reads one sampler's counters; holds what it last said about coverage.
pub(super) struct Probe {
    root: PathBuf,
    pub network: HostCoverage,
    pub disk: HostCoverage,
}

impl Probe {
    pub(super) fn new(root: &Path) -> Self {
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        Self {
            root,
            network: unsupported(),
            disk: unsupported(),
        }
    }

    pub(super) fn counters(&mut self, mono: Duration) -> Counters {
        let at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            });
        if !cfg!(target_os = "linux") {
            return Counters {
                at_ms,
                mono,
                ..Counters::default()
            };
        }
        let proc = |name: &str| std::fs::read_to_string(Path::new("/proc").join(name)).ok();
        let net = proc("net/dev").map(|dev| {
            let all = net(&dev, |name| name != "lo");
            let physical = backed("/sys/class/net");
            let kept: BTreeMap<_, _> = all
                .iter()
                .filter(|(name, _)| physical.contains(name))
                .map(|(name, row)| (name.clone(), *row))
                .collect();
            let (rows, rule) = if kept.is_empty() {
                (all, NET_RULE_FALLBACK)
            } else {
                (kept, NET_RULE)
            };
            self.network = coverage(&rows, rule);
            rows
        });
        let disk = proc("diskstats").map(|stats| {
            let whole = backed("/sys/block");
            let rows = disks(&stats, |name| whole.iter().any(|kept| kept == name));
            self.disk = coverage(&rows, DISK_RULE);
            rows
        });
        if net.is_none() {
            self.network = missing("/proc/net/dev is unreadable.");
        }
        if disk.is_none() {
            self.disk = missing("/proc/diskstats is unreadable.");
        }
        Counters {
            at_ms,
            mono,
            cpu: proc("stat").as_deref().and_then(cpu),
            memory: proc("meminfo").as_deref().and_then(memory),
            net: net.filter(|rows| !rows.is_empty()),
            disk: disk.filter(|rows| !rows.is_empty()),
        }
    }

    pub(super) fn filesystem(&self) -> Option<HostFilesystem> {
        use rustix::fs::{Mode, OFlags};
        // One open directory for the device and the capacity, so both describe the same
        // filesystem even if a mount changes what the path names meanwhile.
        let dir = rustix::fs::open(
            &self.root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .ok()?;
        let dev = rustix::fs::fstat(&dir).ok()?.st_dev;
        let stat = rustix::fs::fstatvfs(&dir).ok()?;
        // Resolved with every measurement; the mount now over the path labels the figures only
        // when it is the measured device.
        let mount = std::fs::read_to_string("/proc/self/mountinfo")
            .ok()
            .and_then(|info| {
                mount_of(
                    &info,
                    &self.root,
                    (rustix::fs::major(dev), rustix::fs::minor(dev)),
                )
            });
        let block = stat.f_frsize;
        let total = stat.f_blocks.checked_mul(block)?;
        let used = stat
            .f_blocks
            .checked_sub(stat.f_bfree)?
            .checked_mul(block)?;
        let free = stat.f_bavail.checked_mul(block)?;
        Some(HostFilesystem {
            path: self.root.display().to_string(),
            mount,
            total,
            used,
            free,
        })
    }
}

/// Entries of a sysfs class directory backed by a hardware device.
fn backed(dir: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("device").exists())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
}

fn coverage(rows: &BTreeMap<String, (u64, u64)>, rule: &str) -> HostCoverage {
    HostCoverage {
        sources: rows.keys().cloned().collect(),
        rule: rule.into(),
        unavailable: rows
            .is_empty()
            .then(|| "No counted source is visible on this host.".into()),
    }
}

fn missing(why: &str) -> HostCoverage {
    HostCoverage {
        sources: Vec::new(),
        rule: String::new(),
        unavailable: Some(why.into()),
    }
}

fn unsupported() -> HostCoverage {
    missing(UNSUPPORTED)
}
