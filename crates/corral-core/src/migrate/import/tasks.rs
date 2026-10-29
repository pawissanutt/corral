//! Legacy task files: imported through the store's own split with the event-only ledger rows,
//! and checked by assembling every imported task back into the record its file parses to.

use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};

use super::events::Line;
use super::{ImportReport, apply};
use crate::backend::{ImportBatch, StoreBackend, TaskNum, TaskParts};
use crate::migrate::blockers::LegacyBoard;
use crate::sqlite::SqliteBackend;
use crate::store::migrate_rows::{assembled, import_task};

const CHUNK: usize = 32;

pub(super) fn import(
    backend: &SqliteBackend,
    board: &LegacyBoard,
    lines: &[Line],
    report: &mut ImportReport,
) -> Result<()> {
    let mut by_task: BTreeMap<&str, Vec<crate::Event>> = BTreeMap::new();
    for line in lines {
        if let Some(event) = &line.event
            && let Some(task) = event.task.as_deref()
        {
            by_task.entry(task).or_default().push(event.clone());
        }
    }
    for chunk in board.tasks.chunks(CHUNK) {
        let tasks = chunk
            .iter()
            .map(|(task, _)| {
                let events = by_task
                    .get(task.task.id.as_str())
                    .map_or(&[][..], Vec::as_slice);
                let mut imported = import_task(task, events)
                    .with_context(|| format!("importing {}", task.task.id))?;
                if let Some(rows) = report.gate_rows.get(&task.task.id) {
                    imported.rows.extend(rows.iter().cloned());
                }
                Ok(imported)
            })
            .collect::<Result<Vec<_>>>()?;
        report.tasks += tasks.len() as u64;
        apply(
            backend,
            ImportBatch {
                blobs: Vec::new(),
                tasks,
                events: Vec::new(),
                records: Vec::new(),
            },
            "tasks",
        )?;
    }
    Ok(())
}

/// Every legacy task equals its database load.
pub(super) fn verify(backend: &SqliteBackend, board: &LegacyBoard) -> Result<()> {
    let view = backend.view()?;
    for (task, _) in &board.tasks {
        let id = &task.task.id;
        let num = TaskNum(crate::store::id_number(id));
        let doc = view
            .task(num, &TaskParts::All)
            .with_context(|| format!("loading imported {id}"))?
            .with_context(|| format!("{id} is missing from the imported database"))?;
        let mut loaded = assembled(&doc).with_context(|| format!("assembling imported {id}"))?;
        let mut legacy = task.record.clone();
        loaded.task.path = std::path::PathBuf::new();
        legacy.task.path = std::path::PathBuf::new();
        ensure!(
            loaded == legacy,
            "imported {id} does not load equal to its legacy file {}",
            task.file.display()
        );
    }
    Ok(())
}
