//! Best-effort, opt-in process and daemon-pass timing records.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use corral_core::timing::{self, Total};
use serde::Serialize;

const MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    lock: Total,
    lock_try_failed: u64,
    busy: Total,
    apply: Total,
    herdr: Total,
    git: Total,
    cpu: Option<Duration>,
    spans: BTreeMap<&'static str, (u64, u64)>,
}

pub fn snapshot() -> Snapshot {
    let (herdr_count, herdr_nanos) = corral_herdr::work::totals();
    let (git_count, git_nanos) = corral_git::work::totals();
    Snapshot {
        lock: timing::LOCK_WAIT.total(),
        lock_try_failed: timing::lock_try_failed(),
        busy: timing::SQLITE_BUSY.total(),
        apply: timing::SQLITE_APPLY.total(),
        herdr: Total {
            count: herdr_count,
            nanos: herdr_nanos,
        },
        git: Total {
            count: git_count,
            nanos: git_nanos,
        },
        cpu: crate::loop_pacing::process_cpu(),
        spans: timing::spans(),
    }
}

/// Uses only parser-resolved command names, never positional or option values.
/// The caller must pass subcommand names from its command parser, not raw argv.
pub fn verb(command: &str, subcommand: Option<&str>) -> String {
    match subcommand {
        Some(subcommand) => format!("{command} {subcommand}"),
        None => command.to_owned(),
    }
}

/// Emits once at the CLI's common exit funnel, without opening a store.
pub fn process(started: Instant, verb: &str) {
    if let Some(dir) = timing::store_dir() {
        let _ = append(
            &crate::daemon_client::runtime_dir_of(dir),
            "process",
            verb,
            started.elapsed(),
            &Snapshot::default(),
        );
    }
}

/// Emits a difference of process-wide counters and returns the sampled endpoint.
pub fn pass(dir: &Path, started: Instant, before: &Snapshot) -> Snapshot {
    append(
        &crate::daemon_client::runtime_dir_of(dir),
        "pass",
        "daemon pass",
        started.elapsed(),
        before,
    )
    .unwrap_or_else(snapshot)
}

fn ms(nanos: u64) -> f64 {
    nanos as f64 / 1_000_000.0
}

fn delta(after: Total, before: Total) -> Total {
    Total {
        count: after.count.saturating_sub(before.count),
        nanos: after.nanos.saturating_sub(before.nanos),
    }
}

#[derive(Serialize)]
struct SpanTotal {
    n: u64,
    ms: f64,
}

#[derive(Serialize)]
struct Record<'a> {
    ts: String,
    pid: u32,
    kind: &'a str,
    verb: &'a str,
    wall_ms: f64,
    cpu_ms: Option<f64>,
    max_rss_kb: Option<u64>,
    lock_wait_ms: f64,
    lock_waits: u64,
    lock_try_failed: u64,
    sqlite_busy_ms: f64,
    sqlite_busy_waits: u64,
    sqlite_apply_ms: f64,
    sqlite_applies: u64,
    herdr_ms: f64,
    herdr_calls: u64,
    git_ms: f64,
    git_calls: u64,
    spans: BTreeMap<&'static str, SpanTotal>,
}

fn record<'a>(
    kind: &'a str,
    verb: &'a str,
    wall: Duration,
    before: &Snapshot,
) -> (Record<'a>, Snapshot) {
    let after = snapshot();
    let lock = delta(after.lock, before.lock);
    let busy = delta(after.busy, before.busy);
    let apply = delta(after.apply, before.apply);
    let herdr = delta(after.herdr, before.herdr);
    let git = delta(after.git, before.git);
    let cpu = after.cpu.and_then(|after| {
        if kind == "process" {
            Some(after)
        } else {
            before.cpu.map(|before| after.saturating_sub(before))
        }
    });
    let spans = after
        .spans
        .iter()
        .filter_map(|(name, (count, nanos))| {
            let (old_count, old_nanos) = before.spans.get(name).copied().unwrap_or_default();
            let n = count.saturating_sub(old_count);
            let nanos = nanos.saturating_sub(old_nanos);
            (n != 0 || nanos != 0).then_some((*name, SpanTotal { n, ms: ms(nanos) }))
        })
        .collect();
    let record = Record {
        ts: jiff::Timestamp::now().to_string(),
        pid: std::process::id(),
        kind,
        verb,
        wall_ms: wall.as_secs_f64() * 1000.0,
        cpu_ms: cpu.map(|cpu| cpu.as_secs_f64() * 1000.0),
        max_rss_kb: max_rss_kb(),
        lock_wait_ms: ms(lock.nanos),
        lock_waits: lock.count,
        lock_try_failed: after.lock_try_failed.saturating_sub(before.lock_try_failed),
        sqlite_busy_ms: ms(busy.nanos),
        sqlite_busy_waits: busy.count,
        sqlite_apply_ms: ms(apply.nanos),
        sqlite_applies: apply.count,
        herdr_ms: ms(herdr.nanos),
        herdr_calls: herdr.count,
        git_ms: ms(git.nanos),
        git_calls: git.count,
        spans,
    };
    (record, after)
}

fn max_rss_kb() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        status.lines().find_map(|line| {
            line.strip_prefix("VmHWM:")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn append(
    dir: &Path,
    kind: &str,
    verb: &str,
    wall: Duration,
    before: &Snapshot,
) -> Option<Snapshot> {
    if !dir.join("timing-on").exists() {
        return None;
    }
    let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("timing.jsonl"))
    else {
        return None;
    };
    if !file
        .metadata()
        .is_ok_and(|metadata| metadata.len() <= MAX_BYTES)
    {
        return None;
    }
    let (record, after) = record(kind, verb, wall, before);
    let Ok(mut line) = serde_json::to_vec(&record) else {
        return Some(after);
    };
    line.push(b'\n');
    if line.len() < 4096 {
        // A single O_APPEND write prevents records from concurrent CLI processes interleaving.
        let _ = file.write(&line);
    }
    Some(after)
}

#[cfg(test)]
mod tests {
    #[test]
    fn timing_record_is_json_without_argument_values() {
        let dir = tempfile::tempdir().unwrap();
        let command = super::verb("pm", Some("brief"));
        let _ = super::append(
            dir.path(),
            "process",
            &command,
            std::time::Duration::ZERO,
            &super::Snapshot::default(),
        );
        assert!(!dir.path().join("timing.jsonl").exists());
        std::fs::write(dir.path().join("timing-on"), "").unwrap();
        let _ = super::append(
            dir.path(),
            "process",
            &command,
            std::time::Duration::ZERO,
            &super::Snapshot::default(),
        );
        let text = std::fs::read_to_string(dir.path().join("timing.jsonl")).unwrap();
        assert_eq!(text.lines().count(), 1);
        let value: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(value["verb"], "pm brief");
        for key in [
            "ts",
            "pid",
            "kind",
            "verb",
            "wall_ms",
            "cpu_ms",
            "max_rss_kb",
            "lock_wait_ms",
            "lock_waits",
            "lock_try_failed",
            "sqlite_busy_ms",
            "sqlite_busy_waits",
            "sqlite_apply_ms",
            "sqlite_applies",
            "herdr_ms",
            "herdr_calls",
            "git_ms",
            "git_calls",
            "spans",
        ] {
            assert!(value.get(key).is_some(), "missing {key}");
        }
    }
}
