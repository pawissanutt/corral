//! Watch-driver-only health, lock diagnostics and opt-in PM recovery.
use std::collections::BTreeMap;

use anyhow::{Context, Result};
use corral_core::agent::pm_state;
use corral_core::backend::{PageReq, RecordQuery};
use corral_core::inbox::{Inbox, ReceiptLevel, Recipient, Urgency};
use corral_core::records::RecordArea;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Ctx;

const HEARTBEAT: RecordArea<Heartbeat> = RecordArea::new("pm-supervisor");
const INPUT_LIMIT: RecordArea<u32> = RecordArea::new("pm-supervisor");
const INTERVAL: i64 = 10;
const BACKOFF: i64 = 300;
const LOCKS: [&str; 4] = [
    "artifact-storage",
    "pm-question-wake-outbox",
    "dispatch",
    "pm-events",
];

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
struct Attempts {
    last_at: Option<i64>,
    failures: u8,
    reported: bool,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
struct LockWatch {
    first_seen: i64,
    reported: bool,
}

impl LockWatch {
    fn report_due(&mut self, now: i64) -> bool {
        if self.reported || now.saturating_sub(self.first_seen) < BACKOFF {
            return false;
        }
        self.reported = true;
        true
    }
}

#[derive(Deserialize, Serialize)]
struct Heartbeat {
    at: Timestamp,
    pid: u32,
    owned: bool,
    herdr_available: bool,
    status: String,
    last_action: Option<String>,
    last_error: Option<String>,
    #[serde(default)]
    generation: Option<u32>,
    #[serde(default)]
    attempts: Attempts,
    #[serde(default)]
    locks: BTreeMap<String, LockWatch>,
    // Scan only a fixed page of the unfinished index, never settled inbox history.
    #[serde(default)]
    pending_after: Option<String>,
    #[serde(default)]
    watched: Vec<u64>,
    #[serde(default)]
    oldest_wake: Option<u64>,
    #[serde(default)]
    usage_guard: Value,
}

impl Heartbeat {
    fn new(now: Timestamp) -> Self {
        Self {
            at: now,
            pid: std::process::id(),
            owned: false,
            herdr_available: false,
            status: String::new(),
            last_action: None,
            last_error: None,
            generation: None,
            attempts: Attempts::default(),
            locks: BTreeMap::new(),
            pending_after: None,
            watched: Vec::new(),
            oldest_wake: None,
            usage_guard: Value::Null,
        }
    }
}

struct Snapshot {
    owned: bool,
    supervise: bool,
    host_alive: bool,
    pending: bool,
    wake_age: Option<u64>,
    attention_secs: u64,
    input_limit: bool,
    attempts: Attempts,
    now: i64,
}

#[derive(Debug, PartialEq, Eq)]
enum Action {
    None,
    Relaunch { reason: &'static str, replace: bool },
    Failed,
}

fn decide(s: &Snapshot) -> Action {
    if !s.owned || !s.supervise {
        return Action::None;
    }
    if s.attempts.failures >= 3 {
        return if s.attempts.reported {
            Action::None
        } else {
            Action::Failed
        };
    }
    if s.attempts
        .last_at
        .is_some_and(|at| s.now.saturating_sub(at) < BACKOFF)
    {
        return Action::None;
    }
    if !s.host_alive && s.pending {
        Action::Relaunch {
            reason: "host_dead",
            replace: false,
        }
    } else if s.host_alive && s.wake_age.is_some_and(|age| age > s.attention_secs) {
        Action::Relaunch {
            reason: "wake_unobserved",
            replace: true,
        }
    } else if s.input_limit {
        Action::Relaunch {
            reason: "input_limit",
            replace: true,
        }
    } else {
        Action::None
    }
}

/// Read-only projection for `pm brief`; absence distinguishes an unwatched store.
pub(crate) fn health(ctx: &Ctx) -> Result<Value> {
    let Some(heartbeat) = HEARTBEAT.load(&ctx.store, "heartbeat")? else {
        return Ok(Value::Null);
    };
    let age = Timestamp::now()
        .as_second()
        .saturating_sub(heartbeat.at.as_second())
        .max(0);
    let mut value = serde_json::to_value(heartbeat)?;
    value["age_secs"] = json!(age);
    value["usage_guard"] = crate::pm_usage_guard::health(ctx)?;
    Ok(value)
}

/// A bridge refusal is generation-local, not evidence that the PM accepted the envelope.
pub(crate) fn input_limit(ctx: &Ctx, generation: u32) -> Result<()> {
    INPUT_LIMIT.save(&ctx.store, "input_limit", &generation)
}

/// Run only after reconciliation in `sync --watch`, never in hooks or one-shot sync.
pub fn supervise(ctx: &Ctx) -> Result<()> {
    // Multiple watch drivers must not race the attempt counter or duplicate diagnostics.
    let Some(_lease) = ctx.store.try_lock("pm-supervisor")? else {
        return Ok(());
    };
    let now = Timestamp::now();
    let mut heartbeat = HEARTBEAT
        .load(&ctx.store, "heartbeat")?
        .unwrap_or_else(|| Heartbeat::new(now));
    let old_status = heartbeat.status.clone();
    let ownership = crate::session_scope::owns_inventory(ctx);
    heartbeat.owned = ownership.as_ref().copied().unwrap_or(false);
    heartbeat.herdr_available = ctx.herdr.available();
    heartbeat.usage_guard = crate::pm_usage_guard::health(ctx)?;
    heartbeat.status = if !heartbeat.owned {
        "unowned"
    } else if !heartbeat.herdr_available {
        "herdr_unavailable"
    } else {
        "healthy"
    }
    .into();
    if !heartbeat.owned && old_status != heartbeat.status {
        crate::messages::report(
            None,
            "supervise",
            "selected herdr socket has foreign or unknown ownership; supervisor actions disabled"
                .into(),
        );
    } else if !heartbeat.herdr_available && old_status != heartbeat.status {
        crate::messages::report(
            None,
            "supervise",
            "herdr is unavailable; waiting without starting a server".into(),
        );
    }
    let mut force = false;
    let result = (|| -> Result<()> {
        ownership?;
        if !heartbeat.owned {
            return Ok(());
        }
        watchdog(ctx, &mut heartbeat, now.as_second())?;
        if heartbeat.locks.values().any(|lock| lock.reported) {
            heartbeat.status = "lock_held".into();
        }
        if !ctx.store.config.pm.supervise || !heartbeat.herdr_available {
            return Ok(());
        }
        force |= supervise_pm(ctx, &mut heartbeat, now.as_second())?;
        Ok(())
    })();
    if let Err(error) = &result {
        heartbeat.last_error = Some(format!("{error:#}"));
        heartbeat.status = if heartbeat.owned { "error" } else { "unowned" }.into();
    }
    force |= old_status != heartbeat.status || heartbeat.attempts.last_at == Some(now.as_second());
    if force || now.as_second().saturating_sub(heartbeat.at.as_second()) >= INTERVAL {
        heartbeat.at = now;
        heartbeat.pid = std::process::id();
        HEARTBEAT.save(&ctx.store, "heartbeat", &heartbeat)?;
    }
    result
}

fn watchdog(ctx: &Ctx, heartbeat: &mut Heartbeat, now: i64) -> Result<()> {
    for name in LOCKS {
        if let Some(lease) = ctx.store.try_lock(name)? {
            drop(lease);
            heartbeat.locks.remove(name);
            continue;
        }
        let watch = heartbeat.locks.entry(name.into()).or_insert(LockWatch {
            first_seen: now,
            reported: false,
        });
        if watch.report_due(now) {
            let seconds = now.saturating_sub(watch.first_seen);
            let holder = lock_holder(&ctx.store.dir().join("lock").join(format!("{name}.lock")));
            if let Err(error) = ctx.store.log_event(
                None,
                "lock_held",
                json!({"lock":name,"held_secs":seconds,"holder":holder}),
            ) {
                watch.reported = false;
                return Err(error.context("recording long-held store lock"));
            }
            crate::messages::report(
                None,
                "supervise",
                format!("store lock {name} held for {seconds}s; holder: {holder}"),
            );
        }
    }
    Ok(())
}

fn supervise_pm(ctx: &Ctx, heartbeat: &mut Heartbeat, now: i64) -> Result<bool> {
    let Some(pm) = pm_state::load_current(&ctx.store)? else {
        return Ok(false);
    };
    let generation = pm.state.binding.generation;
    let mut force = false;
    if heartbeat.generation != Some(generation) {
        heartbeat.generation = Some(generation);
        heartbeat.attempts.failures = 0;
        heartbeat.attempts.reported = false;
        heartbeat.pending_after = None;
        heartbeat.watched.clear();
        heartbeat.oldest_wake = None;
    }
    if heartbeat.usage_guard["status"]["state"] == "paused" {
        // A cache hold is intentional owner review, not an unresponsive host.
        // Relaunching would discard its warm cache and repeat the expensive read.
        heartbeat.status = "cache_guard_paused".into();
        return Ok(false);
    }
    let marker = INPUT_LIMIT.load(&ctx.store, "input_limit")?;
    if marker.is_some_and(|old| generation > old) {
        INPUT_LIMIT.delete(&ctx.store, "input_limit")?;
    }
    let inbox = Inbox::new(&ctx.store, Recipient::Pm)?;
    let (pending, wake_age, observed) = inbox_snapshot(&inbox, heartbeat, now)?;
    if observed {
        heartbeat.attempts.failures = 0;
        heartbeat.attempts.reported = false;
    }
    if heartbeat.attempts.failures >= 3 {
        heartbeat.status = "relaunch_failed".into();
        report_relaunch_failure(ctx, heartbeat)?;
        return Ok(force);
    }
    let host_alive = crate::agent::pi::host_alive(&pm.state)?;
    let action = decide(&Snapshot {
        owned: heartbeat.owned,
        supervise: ctx.store.config.pm.supervise,
        host_alive,
        pending,
        wake_age,
        attention_secs: ctx.store.config.pm.attention_secs,
        input_limit: marker == Some(generation),
        attempts: heartbeat.attempts.clone(),
        now,
    });
    match action {
        Action::None => {}
        Action::Failed => {
            report_relaunch_failure(ctx, heartbeat)?;
            force = true;
        }
        Action::Relaunch { reason, replace } => {
            heartbeat.attempts.last_at = Some(now);
            heartbeat.last_action = Some(reason.into());
            // Persist before the potentially slow effect, including an interrupted attempt.
            heartbeat.attempts.failures = heartbeat.attempts.failures.saturating_add(1);
            heartbeat.at = Timestamp::now();
            HEARTBEAT.save(&ctx.store, "heartbeat", heartbeat)?;
            match crate::pm::relaunch(ctx, reason, replace) {
                Ok(_) => {
                    heartbeat.attempts.failures = 0;
                    heartbeat.last_error = None;
                    // Keep the cooldown across the generation this relaunch just created.
                    heartbeat.generation =
                        pm_state::load_current(&ctx.store)?.map(|pm| pm.state.binding.generation);
                }
                Err(error) => {
                    heartbeat.last_error = Some(format!("{error:#}"));
                    // Recovery may have advanced the generation before startup failed.
                    // That is still this attempt, not an external reset of the circuit.
                    if let Ok(Some(pm)) = pm_state::load_current(&ctx.store) {
                        heartbeat.generation = Some(pm.state.binding.generation);
                    }
                    if heartbeat.attempts.failures >= 3 {
                        report_relaunch_failure(ctx, heartbeat)?;
                    }
                    heartbeat.status = "relaunch_failed".into();
                    return Err(error.context("relaunching supervised PM"));
                }
            }
            force = true;
        }
    }
    Ok(force)
}

fn report_relaunch_failure(ctx: &Ctx, heartbeat: &mut Heartbeat) -> Result<()> {
    if !heartbeat.attempts.reported {
        ctx.store.log_event(
            None,
            "pm_relaunch_failed",
            json!({
                "generation":heartbeat.generation, "attempts":heartbeat.attempts.failures,
                "last_error":heartbeat.last_error,
            }),
        )?;
        heartbeat.attempts.reported = true;
        crate::messages::report(
            None,
            "supervise",
            format!(
                "PM relaunch stopped after three failures: {}",
                heartbeat
                    .last_error
                    .as_deref()
                    .unwrap_or("interrupted attempt")
            ),
        );
    }
    Ok(())
}

fn inbox_snapshot(
    inbox: &Inbox<'_>,
    heartbeat: &mut Heartbeat,
    now: i64,
) -> Result<(bool, Option<u64>, bool)> {
    let mut observed = false;
    for seq in &heartbeat.watched {
        observed |= inbox.receipt(*seq, ReceiptLevel::Observed)?.is_some();
    }
    let mut oldest = heartbeat
        .oldest_wake
        .map(|seq| -> Result<_> {
            let seen = inbox.receipt(seq, ReceiptLevel::Observed)?.is_some();
            observed |= seen;
            if seen || inbox.terminal(seq)?.is_some() {
                return Ok(None);
            }
            Ok(Some(inbox.message(seq)?))
        })
        .transpose()?
        .flatten();
    let area = format!("inbox/{}/pending", inbox.key());
    let view = inbox.store.view()?;
    let query = RecordQuery {
        area: Some(area),
        key: None,
    };
    let first = view.records(&query, PageReq::first(1))?;
    let mut pending = false;
    if let Some(row) = first.items.first() {
        let seq = row
            .key
            .parse::<u64>()
            .context("decoding PM pending sequence")?;
        pending =
            inbox.receipt(seq, ReceiptLevel::Observed)?.is_none() && inbox.terminal(seq)?.is_none();
    }
    let page = view.records(
        &query,
        PageReq {
            after: heartbeat.pending_after.clone(),
            limit: 32,
        },
    )?;
    heartbeat.pending_after = page.next;
    heartbeat.watched.clear();
    for row in page.items {
        let seq = row
            .key
            .parse::<u64>()
            .context("decoding PM pending sequence")?;
        if inbox.receipt(seq, ReceiptLevel::Observed)?.is_some() || inbox.terminal(seq)?.is_some() {
            continue;
        }
        pending = true;
        heartbeat.watched.push(seq);
        let message = inbox.message(seq)?;
        if message.urgency == Urgency::Wake
            && oldest
                .as_ref()
                .is_none_or(|old| message.created < old.created)
        {
            oldest = Some(message);
        }
    }
    heartbeat.oldest_wake = oldest.as_ref().map(|message| message.seq);
    let age = oldest.map(|message| now.saturating_sub(message.created.as_second()).max(0) as u64);
    Ok((pending, age, observed))
}

#[cfg(target_os = "linux")]
fn lock_holder(path: &std::path::Path) -> String {
    use std::os::unix::fs::MetadataExt;
    let found = (|| -> Option<String> {
        let metadata = std::fs::metadata(path).ok()?;
        let dev = metadata.dev();
        let major = ((dev >> 8) & 0xfff) | ((dev >> 32) & 0xfffff000);
        let minor = (dev & 0xff) | ((dev >> 12) & 0xffffff00);
        let locks = std::fs::read_to_string("/proc/locks").ok()?;
        for line in locks.lines() {
            let fields: Vec<_> = line.split_whitespace().collect();
            // Blocked waiters are not holders. OFD locks with pid -1 are unknown.
            if fields.len() < 6 || fields.get(1) == Some(&"->") {
                continue;
            }
            let Some(device) = fields.get(5) else {
                continue;
            };
            let mut parts = device.split(':');
            if u64::from_str_radix(parts.next()?, 16).ok()? != major
                || u64::from_str_radix(parts.next()?, 16).ok()? != minor
                || parts.next()?.parse::<u64>().ok()? != metadata.ino()
            {
                continue;
            }
            let pid = fields.get(4)?.parse::<u32>().ok()?;
            let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok();
            let argv = cmdline.map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', " "));
            return Some(format!(
                "pid {pid}, argv {}",
                argv.as_deref().unwrap_or("unknown").trim()
            ));
        }
        None
    })();
    found.unwrap_or_else(|| "unknown".into())
}

#[cfg(not(target_os = "linux"))]
fn lock_holder(_path: &std::path::Path) -> String {
    "unknown".into()
}

#[cfg(test)]
#[path = "pm_supervisor_tests.rs"]
mod tests;
