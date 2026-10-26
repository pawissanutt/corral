//! On-disk task envelopes deliberately cannot be decoded by legacy flat readers.
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::{RecoveryRecord, Store, Task, TaskRecord, parse_task_record};

pub const STORE_SCHEMA_VERSION: u32 = 3;
pub(crate) const BASE_SCHEMA_VERSION: u32 = 2;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskEnvelope {
    schema_version: u32,
    task: Task,
    #[serde(default)]
    recoveries: Vec<RecoveryRecord>,
}

#[derive(Serialize)]
struct TaskRef<'a> {
    schema_version: u32,
    task: &'a Task,
    #[serde(skip_serializing_if = "<[RecoveryRecord]>::is_empty")]
    recoveries: &'a [RecoveryRecord],
}

pub(crate) fn parse(yaml: &str) -> Result<TaskRecord> {
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(yaml).context("parsing task schema")?;
    if let Some(version) = value.get("schema_version") {
        ensure!(
            matches!(version.as_u64(), Some(2 | 3)),
            "unsupported task schema {version:?}; expected 2 or {STORE_SCHEMA_VERSION}"
        );
        ensure!(
            version.as_u64() == Some(u64::from(STORE_SCHEMA_VERSION))
                || value.get("recoveries").is_none(),
            "recovery records require task schema 3"
        );
        ensure!(
            value
                .get("task")
                .and_then(|t| t.get("recoveries"))
                .is_none(),
            "recovery records belong in the schema-3 envelope"
        );
        let envelope: TaskEnvelope =
            serde_yaml_ng::from_value(value).context("parsing versioned task")?;
        debug_assert!(matches!(envelope.schema_version, 2 | 3));
        let record = TaskRecord {
            task: envelope.task,
            recoveries: envelope.recoveries,
        };
        validate(&record)?;
        Ok(record)
    } else {
        ensure!(
            value.get("recoveries").is_none(),
            "legacy tasks cannot contain recovery records"
        );
        Ok(TaskRecord {
            task: serde_yaml_ng::from_value(value).context("parsing legacy task")?,
            recoveries: Vec::new(),
        })
    }
}

fn validate(record: &TaskRecord) -> Result<()> {
    let mut ids = BTreeSet::new();
    for recovery in &record.recoveries {
        recovery.validate()?;
        ensure!(
            recovery.preview.request.target.task == record.task.id,
            "recovery target does not match canonical task"
        );
        ensure!(ids.insert(&recovery.id), "duplicate recovery identity");
    }
    Ok(())
}

pub(crate) fn render(record: &TaskRecord, version: u32) -> Result<String> {
    validate(record)?;
    ensure!(
        matches!(version, 2 | 3),
        "unsupported task schema {version}"
    );
    ensure!(
        version == 3 || record.recoveries.is_empty(),
        "recovery records require task schema 3"
    );
    serde_yaml_ng::to_string(&TaskRef {
        schema_version: version,
        task: &record.task,
        recoveries: &record.recoveries,
    })
    .context("serializing task envelope")
}

pub(crate) fn store_version(store: &Store) -> Result<u32> {
    let path = store.state_dir().join("schema-version");
    crate::work::record(crate::work::Work::SchemaRead, 1);
    match fs::read_to_string(&path) {
        Ok(text) => match text.trim().parse::<u32>() {
            Ok(version @ (2 | 3)) => Ok(version),
            _ => bail!(
                "unsupported store schema {}; restart with a compatible corral",
                text.trim()
            ),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BASE_SCHEMA_VERSION),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn sync_file(path: &std::path::Path) -> Result<()> {
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .with_context(|| format!("syncing {}", path.display()))
}

/// Recovery intent and preserved history must be durable before callers act on success.
pub(crate) fn persist_record(record: &TaskRecord, version: u32) -> Result<()> {
    persist_record_retained(record, version).map(drop)
}

pub(crate) fn persist_record_retained(
    record: &TaskRecord,
    version: u32,
) -> Result<(String, fs::File)> {
    use std::io::Write;
    let content = crate::store::render_record(record, version)?;
    let path = &record.task.path;
    if record.recoveries.is_empty() {
        let file = crate::store::atomic_write_retained(path, &content)?;
        return Ok((content, file));
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut file = fs::File::create(&tmp)
        .with_context(|| format!("creating recovery replacement {}", tmp.display()))?;
    file.write_all(content.as_bytes())
        .with_context(|| format!("writing recovery replacement {}", tmp.display()))?;
    file.sync_all()
        .with_context(|| format!("syncing recovery replacement {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("publishing recovery record {}", path.display()))?;
    sync_file(
        path.parent()
            .context("recovery record has no parent directory")?,
    )?;
    Ok((content, file))
}

/// Explicit protocol upgrade, never invoked by opening, previewing or ordinary writes.
pub(crate) fn upgrade(store: &Store) -> Result<PathBuf> {
    ensure!(
        !store.is_snapshot(),
        "a read-only task snapshot cannot upgrade the store schema"
    );
    let _board = store.lock("dispatch")?;
    store.check_runtime()?;
    let version = store_version(store)?;
    let backup = store.state_dir().join("schema-3-backup");
    let marker = store.state_dir().join("schema-version");
    let tasks = store.dir().join("tasks");
    let mut paths = fs::read_dir(&tasks)
        .with_context(|| format!("reading {}", tasks.display()))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()
        .context("reading task entries for schema upgrade")?;
    paths.retain(|p| p.extension().is_some_and(|ext| ext == "md"));
    paths.sort();
    // Unlike list(), preflight must not skip malformed records.
    let records = paths
        .iter()
        .map(|path| {
            let text =
                fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            let record = parse_task_record(&text, path)?;
            ensure!(
                version == 3 || record.recoveries.is_empty(),
                "unupgraded recovery history"
            );
            Ok((path, text, record))
        })
        .collect::<Result<Vec<_>>>()?;
    if version == BASE_SCHEMA_VERSION {
        fs::create_dir_all(backup.join("tasks")).context("creating schema backup")?;
        for (path, text, _) in &records {
            let dest = backup
                .join("tasks")
                .join(path.file_name().context("task filename")?);
            backup_file(&dest, text)?;
        }
        let previous = match fs::read_to_string(&marker) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error).context("reading schema marker for backup"),
        };
        backup_file(&backup.join("schema-version"), &previous)?;
        backup_file(&backup.join("complete"), "schema-3 backup complete\n")?;
        sync_file(&backup.join("tasks"))?;
        sync_file(&backup)?;
        crate::store::atomic_write(&marker, "3\n")?;
    } else {
        ensure!(
            backup.join("complete").is_file(),
            "schema-3 upgrade backup is missing"
        );
    }
    // A visible marker may come from a failed sync in an earlier call.
    sync_file(&marker)?;
    sync_file(&store.state_dir())?;
    // Marker publication first excludes old readers/writers even after a crash here.
    for (path, _, record) in records {
        persist_record(&record, 3)?;
        sync_file(path)?;
    }
    sync_file(&tasks)?;
    Ok(backup)
}

fn backup_file(path: &std::path::Path, text: &str) -> Result<()> {
    use std::io::Write;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(text.as_bytes())
                .with_context(|| format!("backing up {}", path.display()))?;
            file.sync_all()
                .with_context(|| format!("syncing backup {}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            ensure!(
                fs::read_to_string(path)
                    .with_context(|| format!("reading backup {}", path.display()))?
                    == text,
                "schema backup differs; preserve it and resolve before retrying"
            );
            sync_file(path)
        }
        Err(error) => Err(error).with_context(|| format!("creating backup {}", path.display())),
    }
}

impl Store {
    /// Retains original bytes and explicitly upgrades the protocol before recovery writes.
    ///
    /// # Errors
    /// Refuses incompatible schemas, malformed tasks, conflicting backups and I/O errors.
    pub fn upgrade_schema(&self) -> Result<PathBuf> {
        upgrade(self)
    }

    /// Reads the live canonical envelope, including recovery evidence.
    ///
    /// # Errors
    /// Refuses frozen snapshots; otherwise returns task lookup, schema, decoding or I/O errors.
    pub fn task_record(&self, id: &str) -> Result<TaskRecord> {
        ensure!(
            !self.is_snapshot(),
            "a read-only task snapshot cannot read live recovery records"
        );
        let task = self.get(id)?;
        let text = fs::read_to_string(&task.path)
            .with_context(|| format!("reading {}", task.path.display()))?;
        parse_task_record(&text, &task.path)
    }

    /// Durably persists intent/outcomes without evaluating, authorizing or executing recovery.
    /// Equivalent retries sync the existing record and its directory without rewriting bytes.
    ///
    /// # Errors
    /// Refuses unupgraded stores, changed intent, rewritten history and invalid records.
    /// Propagates publication and synchronization errors, including after a visible rename.
    pub fn save_recovery_record(&self, task: &str, recovery: &RecoveryRecord) -> Result<()> {
        ensure!(
            !self.is_snapshot(),
            "a read-only task snapshot cannot save recovery records"
        );
        let _board = self.lock("dispatch")?;
        let task = Store::normalize_id(task)?;
        let _task = self.lock(&task)?;
        self.check_runtime()?;
        ensure!(
            store_version(self)? == 3,
            "explicit schema-3 upgrade required before recovery writes"
        );
        recovery.validate()?;
        ensure!(
            recovery.preview.request.target.store == self.root(),
            "recovery target does not match canonical store"
        );
        let mut record = self.task_record(&task)?;
        if let Some(previous) = record.recoveries.iter_mut().find(|r| r.id == recovery.id) {
            recovery.preserves(previous)?;
            if previous == recovery {
                // A previous directory-sync error may have left identical bytes visible.
                sync_file(&record.task.path)?;
                return sync_file(
                    record
                        .task
                        .path
                        .parent()
                        .context("recovery record has no parent directory")?,
                );
            }
            *previous = recovery.clone();
        } else {
            record.recoveries.push(recovery.clone());
        }
        record.task.updated = jiff::Timestamp::now();
        persist_record(&record, 3)
    }
}
