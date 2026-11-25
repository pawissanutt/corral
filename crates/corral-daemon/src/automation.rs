//! Durable at-most-once automation claims scoped to one workspace.
use anyhow::{Context, Result, ensure};
use corral_engine::{
    Store,
    config::{Config, automation::matches_minute},
};
use corral_types::{Event, EventKind};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
};

mod worker;
pub(crate) use worker::start;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct Health {
    pub(crate) outcome: Option<String>,
    pub(crate) last_at: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) chat: Option<String>,
}

pub(super) struct Delivery {
    pub(super) outcome: String,
    pub(super) error: Option<String>,
    pub(super) chat: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct Record {
    effective: String,
    #[serde(default)]
    generation: u64,
    #[serde(default)]
    attempt_minute: Option<i64>,
    cursor: u64,
    // Baseline/attempt high water; never rewind on clock rollback.
    minute: i64,
    claim: Option<String>,
    last_skip: Option<String>,
    skipped: u64,
    health: Health,
}

fn safe_id(id: &str) -> bool {
    let mut bytes = id.bytes();
    id.len() <= 40
        && bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
}

fn read<T: serde::de::DeserializeOwned>(path: &Path, limit: u64) -> Result<Option<T>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(
            rustix::fs::OFlags::NOFOLLOW.bits() as i32 | rustix::fs::OFlags::NONBLOCK.bits() as i32,
        )
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("open {}", path.display())),
    };
    ensure!(
        file.metadata()?.is_file(),
        "not a regular file: {}",
        path.display()
    );
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "oversized automation state: {}",
        path.display()
    );
    Ok(Some(
        serde_json::from_slice(&bytes).with_context(|| format!("decode {}", path.display()))?,
    ))
}

use std::os::unix::fs::OpenOptionsExt;

fn save(dir: &Path, path: &Path, record: &impl Serialize) -> Result<()> {
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(&mut temp, record)?;
    temp.write_all(b"\n")?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    File::open(dir)?.sync_all()?;
    Ok(())
}

fn record(path: &Path) -> Result<Option<Record>> {
    read(path, 64 * 1024)
}

pub(crate) fn effective(config: &Config, id: &str) -> Result<String> {
    let rule = config
        .automation
        .rules
        .get(id)
        .context("missing automation rule")?;
    let agent = config
        .automation
        .agents
        .get(&rule.agent)
        .context("missing automation agent")?;
    let profile = config
        .profiles
        .get(&agent.profile)
        .context("missing automation profile")?;
    let harness = config
        .harnesses
        .get(&profile.harness)
        .context("missing automation harness")?;
    let bytes = serde_json::to_vec(&(rule, agent, harness, profile))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Load only configured rule records, with atomic reads during a pass.
pub(crate) fn health(state_dir: &Path, config: &Config) -> Result<BTreeMap<String, Health>> {
    let dir = state_dir.join("automation");
    config
        .automation
        .rules
        .keys()
        .map(|id| {
            ensure!(safe_id(id), "invalid automation rule ID: {id:?}");
            let status = record(&dir.join(format!("{id}.json")))?
                .map(|r| {
                    let mut health = r.health;
                    if r.skipped != 0 {
                        let outcome = health.outcome.get_or_insert_with(String::new);
                        if !outcome.is_empty() {
                            outcome.push_str(" · ");
                        }
                        outcome.push_str(&format!("{} rate-limited matches", r.skipped));
                    }
                    health
                })
                .unwrap_or_default();
            Ok((id.clone(), status))
        })
        .collect()
}

fn clip(value: &str, limit: usize) -> &str {
    &value[..value.floor_char_boundary(limit.min(value.len()))]
}

fn event_input(id: &str, event: &Event) -> String {
    // A string preserves valid outer JSON even when the detail is truncated.
    let detail = event.detail.to_string();
    let mut budget = 2048;
    loop {
        let text = json!({
            "type": "automation_trigger_data", "rule": id, "trigger": "blocked", "seq": event.seq,
            "task": event.task.as_deref().map(|task| clip(task, 128)), "detail": clip(&detail, budget),
        }).to_string();
        if text.len() <= 2048 {
            return text;
        }
        budget = budget.saturating_sub(text.len() - 2048);
    }
}

mod activation;
pub(crate) use activation::stage;

pub(crate) struct Fence {
    dir: std::path::PathBuf,
    _lock: File,
}

pub(crate) fn fence(state_dir: &Path) -> Result<Fence> {
    let dir = state_dir.join("automation");
    fs::create_dir_all(&dir)?;
    ensure!(
        fs::symlink_metadata(&dir)?.is_dir(),
        "automation state directory is not a directory"
    );
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(dir.join(".lock"))?;
    ensure!(
        lock.metadata()?.is_file(),
        "automation lock is not a regular file"
    );
    lock.lock()?;
    File::open(state_dir)?.sync_all()?;
    Ok(Fence { dir, _lock: lock })
}

/// Process at most one event page per rule per pass, leaving a durable cursor.
fn pass(
    fence: &Fence,
    store: &Store,
    config: &Config,
    manifest: Option<&activation::Manifest>,
    now: Timestamp,
    send: &mut impl FnMut(&str, &str, Option<&str>, &str, &str) -> Delivery,
) -> Result<bool> {
    let rules = &config.automation.rules;
    let dir = &fence.dir;
    let mut changed = false;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".json"))
            && safe_id(id)
            && !rules.contains_key(id)
            && entry.file_type()?.is_file()
        {
            changed |= record(&entry.path())
                .ok()
                .flatten()
                .is_some_and(|r| r.health != Health::default());
            fs::remove_file(entry.path())?;
            File::open(dir)?.sync_all()?;
        }
    }
    let head = if rules.is_empty() {
        0
    } else {
        store.latest_seq()?
    };
    let minute = now.as_second().div_euclid(60);
    for (id, rule) in rules {
        ensure!(safe_id(id), "invalid automation rule ID: {id:?}");
        let effective = effective(config, id)?;
        let activation = manifest
            .and_then(|manifest| manifest.rules.get(id))
            .filter(|activation| activation.effective == effective);
        let generation = activation.map_or(0, |activation| activation.generation);
        let path = dir.join(format!("{id}.json"));
        let mut record = record(&path)?.unwrap_or(Record {
            effective: String::new(),
            generation: 0,
            attempt_minute: None,
            cursor: 0,
            minute: i64::MIN,
            claim: None,
            last_skip: None,
            skipped: 0,
            health: Health::default(),
        });
        // Legacy state used a single high-water minute for both purposes.
        if record.attempt_minute.is_none() && record.claim.is_some() {
            record.attempt_minute = Some(record.minute);
        }
        if record.effective != effective || (generation != 0 && record.generation != generation) {
            record.effective = effective;
            record.generation = generation;
            record.cursor =
                activation.map_or(head, |activation| activation.cursor.max(record.cursor));
            record.minute = record
                .minute
                .max(activation.map_or(minute, |activation| activation.minute));
            save(dir, &path, &record)?;
            if activation.is_none() {
                continue;
            }
        }
        if !rule.enabled {
            continue;
        }
        let before = record.health.clone();
        let skipped_before = record.skipped;
        let mut fire = |record: &mut Record, key: String, input: String| -> Result<()> {
            record.minute = record.minute.max(minute);
            record.attempt_minute = Some(minute);
            record.claim = Some(key.clone());
            record.health.last_at = Some(now.to_string());
            record.health.outcome = Some("delivery unknown".into());
            record.health.error = Some("delivery unknown".into());
            save(dir, &path, record)?;
            let input_id = format!("{:x}", Sha256::digest(format!("{id}/{key}")));
            let delivery = send(
                id,
                &record.effective,
                record.health.chat.as_deref(),
                &input_id,
                &input,
            );
            record.health.outcome = Some(clip(&delivery.outcome, 1024).into());
            record.health.error = delivery
                .error
                .as_deref()
                .map(|error| clip(error, 4096).into());
            if let Some(chat) = delivery.chat {
                record.health.chat = Some(clip(&chat, 1024).into());
            }
            save(dir, &path, record)
        };
        if rule.event.as_deref() == Some("blocked") && record.cursor < head {
            for event in store.event_page(
                None,
                Some(record.cursor),
                Some(head.saturating_add(1)),
                false,
                128,
            )? {
                record.cursor = event.seq;
                if event.kind != EventKind::Blocked {
                    continue;
                }
                let key = format!("event:{}", event.seq);
                if record
                    .attempt_minute
                    .is_some_and(|attempt| minute <= attempt)
                {
                    // Keep skips separately so they don't hide failures or unknown delivery.
                    record.last_skip = Some(key);
                    record.skipped = record.skipped.saturating_add(1);
                } else {
                    fire(&mut record, key, event_input(id, &event))?;
                }
            }
            save(dir, &path, &record)?;
        } else if let Some(expression) = &rule.cron
            && minute > record.minute
            && matches_minute(expression, minute)?
        {
            fire(&mut record, format!("cron:{minute}"), json!({
                "type": "automation_trigger_data", "rule": id, "trigger": "cron", "utc_minute": minute,
            }).to_string())?;
        }
        changed |= record.health != before || record.skipped != skipped_before;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests;
