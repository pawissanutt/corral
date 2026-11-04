//! Checkpoint on the worker, never wait on another process's snapshot.
use super::*;

/// Soft WAL limit: normal cycles truncate below this; external readers/writers may prevent
/// it. Report that condition rather than blocking the board or discarding their snapshots.
pub const WAL_LIMIT_BYTES: u64 = 16 * 1024 * 1024;

impl SqliteBackend {
    pub fn maintain_wal(&self) -> Result<()> {
        readers::expire_idle(&self.active_readers)?;
        let conn = match self.writer.try_lock() {
            Ok(conn) => Some(conn),
            Err(std::sync::TryLockError::WouldBlock) => None,
            Err(std::sync::TryLockError::Poisoned(error)) => Some(error.into_inner()),
        };
        let busy = if let Some(conn) = conn {
            conn.busy_handler(None)
                .context("disable checkpoint waiting")?;
            let result = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                row.get::<_, i64>(0)
            });
            conn.busy_handler(Some(busy_wait))
                .context("restore busy handler")?;
            result.context("periodic WAL checkpoint")? != 0
        } else {
            true
        };
        let wal = self.path.with_extension("db-wal");
        let bytes = match std::fs::metadata(&wal) {
            Ok(meta) => meta.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error).context("stat WAL size"),
        };
        if bytes > WAL_LIMIT_BYTES {
            crate::messages::report(
                None,
                "WAL size",
                format!(
                    "{} is {bytes} bytes (limit {WAL_LIMIT_BYTES}); checkpoint busy={busy}; an active reader/writer is retaining WAL frames",
                    wal.display()
                ),
            );
        }
        Ok(())
    }
}

/// Recovery intent/outcomes are write-ahead evidence for external side effects. Preserve
/// their fsync-before-success contract, including rewrites of recovery-bearing tasks.
pub(super) fn requires_full(conn: &Connection, op: &Operation) -> rusqlite::Result<bool> {
    let recovery = |part| matches!(part, TaskPart::Recovery | TaskPart::RecoveryOutcome);
    if let Some(change) = op.task_change() {
        if change.rows.iter().any(|row| match row {
            RowWrite::Put(row) => recovery(row.part),
            RowWrite::Delete { part, .. } => recovery(*part),
        }) {
            return Ok(true);
        }
        return conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM task_rows WHERE task = ?1 AND part = 'recovery')",
            params![
                i64::try_from(change.num.0)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?
            ],
            |row| row.get(0),
        );
    }
    Ok(match op {
        Operation::CreateTask(task) => task.rows.iter().any(|row| recovery(row.part)),
        Operation::ImportBatch(batch) => batch
            .tasks
            .iter()
            .any(|task| task.rows.iter().any(|row| recovery(row.part))),
        _ => false,
    })
}
