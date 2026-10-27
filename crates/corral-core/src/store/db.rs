//! The store's database and the v2 open protocol (plan T-253 section 12).
//!
//! Every opener holds the store gate `lock/store` shared for as long as its database handle
//! lives and registers in `lock/store.holders/<pid>-<start>`; maintenance takes the gate
//! exclusively, so no v2 connection exists while it runs. An open accepts only a published,
//! finalizing or finalized migration journal, or no journal with a database `corral init`
//! created, each with fence 4 at `state/schema-version`; every other state is refused naming
//! the command that moves it on. One handle per database is shared by a process.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::backend::{ChangeSeq, StoreBackend};
use crate::sqlite::{SqliteBackend, SqliteOptions};

/// The fence value of a v2 store.
pub const FENCE: &str = "4";
pub const JOURNAL: &str = "migration.json";
pub const GATE: &str = "store";
pub const HOLDERS: &str = "store.holders";
/// How long an opener waits for maintenance to release the gate.
const GATE_WAIT: Duration = Duration::from_secs(2);

pub(crate) struct Db {
    pub(crate) backend: Box<dyn StoreBackend>,
    /// A second handle on the database file for the operations outside the contract
    /// (database metadata, fixtures).
    pub(crate) sqlite: Option<SqliteBackend>,
    /// The last full listing and the change seq it was read at.
    pub(crate) listing: Mutex<Option<(ChangeSeq, Arc<Vec<crate::Task>>)>>,
    /// Task ids a fixture keeps [`crate::Store::publish_staged`] from publishing.
    pub(crate) blocked: Mutex<std::collections::BTreeSet<String>>,
    _gate: Option<Arc<StoreGate>>,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Db")
    }
}

impl Db {
    /// A database over any backend, without a gate (test fixtures over the fake backend).
    pub(crate) fn unguarded(backend: Box<dyn StoreBackend>) -> Arc<Db> {
        Arc::new(Db {
            backend,
            sqlite: None,
            listing: Mutex::default(),
            blocked: Mutex::default(),
            _gate: None,
        })
    }
}

struct StoreGate {
    file: fs::File,
    holder: PathBuf,
}

impl Drop for StoreGate {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.holder)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            crate::messages::report_file(
                &self.holder,
                format!("removing store holder {}: {error}", self.holder.display()),
            );
        }
        if let Err(error) = self.file.unlock() {
            crate::messages::report(None, "store gate", format!("releasing store gate: {error}"));
        }
    }
}

fn open_gate_file(corral: &Path) -> Result<fs::File> {
    let dir = corral.join("lock");
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(GATE);
    fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))
}

fn acquire_gate(corral: &Path) -> Result<StoreGate> {
    let file = open_gate_file(corral)?;
    let deadline = Instant::now() + GATE_WAIT;
    loop {
        match file.try_lock_shared() {
            Ok(()) => break,
            Err(fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(fs::TryLockError::WouldBlock) => {
                bail!("store maintenance in progress (corral migrate --status)")
            }
            Err(fs::TryLockError::Error(error)) => {
                return Err(error).context("taking the store gate");
            }
        }
    }
    let holders = corral.join("lock").join(HOLDERS);
    fs::create_dir_all(&holders).with_context(|| format!("creating {}", holders.display()))?;
    let me = crate::blob::ProcessIdentity::current()?;
    static ACQUIRED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = ACQUIRED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let holder = holders.join(format!("{}-{}-{n}", me.pid, me.start_time));
    fs::write(&holder, format!("{}\n", me.owner()))
        .with_context(|| format!("registering {}", holder.display()))?;
    Ok(StoreGate { file, holder })
}

/// The exclusive store gate maintenance takes; `None` while any opener holds it.
///
/// # Errors
/// Returns filesystem errors.
pub fn try_exclusive_store_gate(corral: &Path) -> Result<Option<crate::StoreLock>> {
    let file = open_gate_file(corral)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(crate::StoreLock::new(file))),
        Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(fs::TryLockError::Error(error)) => Err(error).context("taking the store gate"),
    }
}

/// The registered holders of the store gate (diagnostics only).
///
/// # Errors
/// Returns directory read errors other than an absent registry.
pub fn store_holders(corral: &Path) -> Result<Vec<String>> {
    let dir = corral.join("lock").join(HOLDERS);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", dir.display())),
    };
    let mut holders = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        holders.push(entry.file_name().to_string_lossy().into_owned());
    }
    holders.sort();
    Ok(holders)
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

pub(crate) fn fence(corral: &Path) -> Result<Option<String>> {
    Ok(
        read_optional(&corral.join("state").join("schema-version"))? // path-guard: allow (migration fence)
            .map(|text| text.trim().to_owned()),
    )
}

fn journal(corral: &Path) -> Result<Option<(String, Option<String>)>> {
    let Some(text) = read_optional(&corral.join(JOURNAL))? else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing {}", corral.join(JOURNAL).display()))?;
    let field = |name: &str| value.get(name).and_then(|v| v.as_str()).map(str::to_owned);
    let state = field("state").context("the migration journal has no state")?;
    Ok(Some((state, field("step"))))
}

fn db_dir_present(corral: &Path) -> bool {
    SqliteBackend::db_path(corral).is_file()
}

/// Refuses every journal and fence combination a v2 opener must not open.
fn check_openable(corral: &Path) -> Result<()> {
    let fence = fence(corral)?;
    match journal(corral)? {
        Some((state, _)) if matches!(state.as_str(), "published" | "finalizing" | "finalized") => {
            if fence.as_deref() != Some(FENCE) {
                bail!(
                    "store migration is {state} but the fence reads {}; run `corral migrate --status`",
                    fence.as_deref().unwrap_or("nothing")
                );
            }
            if !db_dir_present(corral) {
                bail!(
                    "store migration is {state} but the database is missing; run `corral migrate --status`"
                );
            }
            Ok(())
        }
        Some((state, step)) if state == "rolling_back" => bail!(
            "store rollback is in progress (step {}); run `corral migrate --rollback`",
            step.as_deref().unwrap_or("unknown")
        ),
        Some((state, _)) if matches!(state.as_str(), "fenced" | "importing" | "verified") => bail!(
            "store migration is in progress ({state}); run `corral migrate --apply` or `corral migrate --rollback`"
        ),
        Some((state, _)) => {
            bail!("unknown store migration state {state:?}; run `corral migrate --status`")
        }
        None => match fence.as_deref() {
            Some(FENCE) if db_dir_present(corral) => Ok(()),
            Some(FENCE) => bail!("the store database is missing; run `corral init` to create it"),
            Some(other) => bail!(
                "this workspace uses the legacy file store (schema {other}); run `corral migrate` to move it into the database"
            ),
            None => bail!(
                "this workspace uses the legacy file store; run `corral migrate` to move it into the database"
            ),
        },
    }
}

fn registry() -> &'static Mutex<HashMap<PathBuf, Weak<Db>>> {
    static OPEN: OnceLock<Mutex<HashMap<PathBuf, Weak<Db>>>> = OnceLock::new();
    OPEN.get_or_init(Mutex::default)
}

/// Opens the database of the store at `corral` (its `.corral` dir) by the v2 protocol, or
/// shares this process's open handle.
///
/// # Errors
/// Refuses a held gate and every state the protocol does not open; returns database errors.
pub(crate) fn open(corral: &Path) -> Result<Arc<Db>> {
    let mut open = registry().lock().unwrap_or_else(PoisonError::into_inner);
    open.retain(|_, db| db.strong_count() > 0);
    if let Some(db) = open.get(corral).and_then(Weak::upgrade) {
        return Ok(db);
    }
    let gate = Arc::new(acquire_gate(corral)?);
    check_openable(corral)?;
    let mut backend = SqliteBackend::open(corral, backend_options(corral))?;
    backend.retain_in_views(Arc::clone(&gate) as crate::sqlite::Keepalive);
    if journal(corral)?.is_none() && backend.meta("origin")?.as_deref() != Some("init") {
        bail!(
            "the store database was not created by `corral init` and has no migration journal; run `corral migrate --status`"
        );
    }
    let sqlite = backend.handle()?;
    let db = Arc::new(Db {
        backend: Box::new(backend),
        sqlite: Some(sqlite),
        listing: Mutex::default(),
        blocked: Mutex::default(),
        _gate: Some(gate),
    });
    open.insert(corral.to_path_buf(), Arc::downgrade(&db));
    Ok(db)
}

/// Whether `corral` holds legacy task files, which a store without a marker is (schema 2).
fn legacy_tasks_present(corral: &Path) -> Result<bool> {
    let tasks = corral.join("tasks");
    let entries = match fs::read_dir(&tasks) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).with_context(|| format!("reading {}", tasks.display())),
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", tasks.display()))?;
        if entry.path().extension().is_some_and(|ext| ext == "md") {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The backend options of the store in `corral`: the effective `[storage].view_max_age` of its
/// configuration. A configuration that does not load keeps the default here; the store's own
/// configuration load, which follows the open, reports it.
pub(crate) fn backend_options(corral: &Path) -> SqliteOptions {
    let mut options = SqliteOptions::default();
    if let Ok(config) =
        crate::ConfigLayers::load(&corral.join("corral.toml")).and_then(|layers| layers.config())
    {
        options.config.view_max_age = config.storage.view_max_age();
    }
    options
}

/// Creates the database and fence of a new store, then opens it. An existing v2 store opens
/// as it is; a legacy store or one in migration is refused.
///
/// # Errors
/// Refuses legacy and migrating stores; returns filesystem or database errors.
pub(crate) fn init(corral: &Path) -> Result<Arc<Db>> {
    if journal(corral)?.is_none() {
        match fence(corral)?.as_deref() {
            None if legacy_tasks_present(corral)? => bail!(
                "{} holds a legacy board without a schema marker; its tasks stay in place. Run `corral migrate --status`",
                corral.display()
            ),
            None | Some(FENCE) => {
                if !db_dir_present(corral) {
                    let _gate = acquire_gate(corral)?;
                    drop(SqliteBackend::open(corral, SqliteOptions::default())?);
                }
                write_fence(corral)?;
            }
            Some(_) => {}
        }
    }
    open(corral)
}

fn write_fence(corral: &Path) -> Result<()> {
    let state = corral.join("state"); // path-guard: allow (migration fence)
    fs::create_dir_all(&state).with_context(|| format!("creating {}", state.display()))?;
    let path = state.join("schema-version");
    if fence(corral)?.as_deref() == Some(FENCE) {
        return Ok(());
    }
    let tmp = state.join("schema-version.tmp");
    fs::write(&tmp, format!("{FENCE}\n")).with_context(|| format!("writing {}", tmp.display()))?;
    fs::File::open(&tmp)
        .and_then(|file| file.sync_all())
        .with_context(|| format!("syncing {}", tmp.display()))?;
    fs::rename(&tmp, &path).with_context(|| format!("publishing {}", path.display()))?;
    fs::File::open(&state)
        .and_then(|dir| dir.sync_all())
        .with_context(|| format!("syncing {}", state.display()))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::backend::{ManualClock, StoreBackend, TaskNum, ViewError};
    use crate::sqlite::SqliteBackend;

    #[test]
    fn the_store_backend_expires_views_at_the_configured_age() {
        // Given a store configured with a non-default view_max_age.
        let dir = tempfile::tempdir().unwrap();
        crate::Store::init(dir.path()).unwrap();
        let corral = dir.path().join(".corral");
        std::fs::write(
            corral.join("corral.toml"),
            "[storage]\nview_max_age = 300\n",
        )
        .unwrap();
        // When its backend is opened with the store's options on a controllable clock.
        let mut options = super::backend_options(&corral);
        assert_eq!(options.config.view_max_age, Duration::from_secs(300));
        let clock = ManualClock::new("2026-01-01T00:00:00Z".parse().unwrap());
        options.clock = Arc::new(clock.clone());
        let backend = SqliteBackend::open(&corral, options).unwrap();
        let view = backend.view().unwrap();
        // Then a view outlives the 120 s default and expires only after the configured age.
        clock.advance(Duration::from_secs(200));
        assert!(view.head(TaskNum(1)).is_ok());
        clock.advance(Duration::from_secs(101));
        assert!(matches!(
            view.head(TaskNum(1)),
            Err(ViewError::Expired { .. })
        ));
    }

    #[test]
    fn cached_store_openers_use_their_configured_age_without_changing_pinned_views() {
        for unobserved in [false, true] {
            for reopen in [false, true] {
                // Given a live Store with a real cached SQLite backend on a manual clock.
                let dir = tempfile::tempdir().unwrap();
                let initial = crate::Store::init(dir.path()).unwrap();
                let mut config = initial.config.clone();
                config.storage.view_max_age = 300;
                std::fs::write(initial.config_path(), config.to_toml()).unwrap();
                let old = initial.reopen().unwrap();
                let clock = Arc::new(ManualClock::new("2026-01-01T00:00:00Z".parse().unwrap()));
                let mut options = super::backend_options(&old.dir);
                options.clock = clock.clone();
                let backend = SqliteBackend::open(&old.dir, options).unwrap();
                let old = old.with_backend(Box::new(backend));
                super::registry()
                    .lock()
                    .unwrap()
                    .insert(old.dir.clone(), Arc::downgrade(&old.db));
                let task = old
                    .create("expiry", None, crate::Status::Backlog, "")
                    .unwrap();
                let pinned = old.view().unwrap();
                let old_head = old.head(&task.id).unwrap();
                let snapshot = old.read_only_snapshot().unwrap();
                // When another opener loads a shorter age while the cached Store stays alive.
                config.storage.view_max_age = 30;
                std::fs::write(old.config_path(), config.to_toml()).unwrap();
                let mut new = match (reopen, unobserved) {
                    (true, false) => old.reopen(),
                    (true, true) => old.reopen_unobserved(),
                    (false, false) => crate::Store::open(dir.path()),
                    (false, true) => crate::Store::open_unobserved(dir.path()),
                }
                .unwrap();
                assert!(Arc::ptr_eq(&old.db, &new.db), "exercise the cached handle");
                assert_eq!(new.config.storage.view_max_age, 30);
                let view = new.view().unwrap();
                let reader = new.reader().unwrap();
                let new_snapshot = new.read_only_snapshot().unwrap();
                let new_head = new.head(&task.id).unwrap();
                // Then its views expire at the new age, but existing views retain their age.
                clock.advance(Duration::from_secs(30));
                view.ensure_fresh().unwrap();
                clock.advance(Duration::from_secs(1));
                assert!(matches!(
                    view.ensure_fresh(),
                    Err(ViewError::Expired { .. })
                ));
                assert!(matches!(
                    reader.ensure_fresh(),
                    Err(ViewError::Expired { .. })
                ));
                assert!(new_snapshot.view().unwrap().ensure_fresh().is_err());
                assert!(new_head.emitted_comments().is_err());
                pinned.ensure_fresh().unwrap();
                old_head.emitted_comments().unwrap();
                snapshot.view().unwrap().ensure_fresh().unwrap();
                old.view().unwrap().ensure_fresh().unwrap();
                drop(reader);
                // A later reload can extend new views without reviving expired pinned ones.
                config.storage.view_max_age = 60;
                std::fs::write(new.config_path(), config.to_toml()).unwrap();
                new.reload_config_if_changed().unwrap();
                let reloaded = new.view().unwrap();
                clock.advance(Duration::from_secs(60));
                reloaded.ensure_fresh().unwrap();
                assert!(view.ensure_fresh().is_err());
                clock.advance(Duration::from_secs(1));
                assert!(reloaded.ensure_fresh().is_err());
                pinned.ensure_fresh().unwrap();
            }
        }
    }
}
