use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use corral_core::Store;
use corral_core::backend::*;
use corral_core::sqlite::{SqliteBackend, SqliteOptions};

/// Reject exactly the selected record operation while preserving real database reads and all
/// unrelated writes. Reopening the workspace sees the same database, without the injected fault.
pub fn fail_records(store: &Store, area: &str, key: &str) -> (Store, Arc<AtomicBool>) {
    let enabled = Arc::new(AtomicBool::new(true));
    let backend = RecordFailure {
        inner: SqliteBackend::open(store.dir(), SqliteOptions::default()).unwrap(),
        area: area.to_owned(),
        key: key.to_owned(),
        enabled: enabled.clone(),
    };
    (store.with_backend(Box::new(backend)), enabled)
}

struct RecordFailure {
    inner: SqliteBackend,
    area: String,
    key: String,
    enabled: Arc<AtomicBool>,
}

impl StoreBackend for RecordFailure {
    fn view(&self) -> Result<Box<dyn ReadView + Send>, ViewError> {
        self.inner.view()
    }

    fn view_with_max_age(&self, age: Duration) -> Result<Box<dyn ReadView + Send>, ViewError> {
        self.inner.view_with_max_age(age)
    }

    fn apply(&self, op: Operation) -> Result<Committed, ApplyError> {
        if self.enabled.load(Ordering::SeqCst)
            && let Operation::PutRecords(records) = &op
            && records
                .writes
                .iter()
                .any(|write| write.area == self.area && write.key == self.key)
        {
            return Err(ApplyError::Io(anyhow::anyhow!(
                "injected record publication failure"
            )));
        }
        self.inner.apply(op)
    }

    fn check(&self) -> Result<IntegrityReport> {
        self.inner.check()
    }

    fn backup_database(&self, dest: &Path) -> Result<()> {
        self.inner.backup_database(dest)
    }
}
