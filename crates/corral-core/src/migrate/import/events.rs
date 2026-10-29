//! `events.jsonl`, imported verbatim: one event per line in file order; a line that does not
//! decode is kept whole as a malformed event (plan section 9).

use std::path::Path;

use anyhow::{Context, Result, ensure};

use super::{ImportReport, apply};
use crate::backend::{EventQuery, ImportBatch, ImportedEvent, NewEvent, StoreBackend};
use crate::sqlite::SqliteBackend;

/// One line of the legacy log and its decoded event.
pub(crate) struct Line {
    pub(crate) text: String,
    pub(crate) event: Option<crate::Event>,
}

pub(super) fn read(corral: &Path) -> Result<Vec<Line>> {
    let path = corral.join("events.jsonl");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let text = String::from_utf8_lossy(&bytes);
    Ok(text
        .split_inclusive('\n')
        .map(|line| {
            let text = line.strip_suffix('\n').unwrap_or(line).to_owned();
            let event = serde_json::from_str::<crate::Event>(&text).ok();
            Line { text, event }
        })
        .collect())
}

fn imported(line: &Line, at: jiff::Timestamp) -> Result<ImportedEvent> {
    Ok(match &line.event {
        Some(event) => ImportedEvent {
            event: NewEvent {
                ts: event.ts,
                task: event.task.clone(),
                kind: event.kind.clone(),
                actor: event
                    .actor
                    .map(serde_json::to_value)
                    .transpose()?
                    .and_then(|actor| actor.as_str().map(str::to_owned)),
                via: event.via.clone(),
                run: None,
                detail: event.detail.clone(),
            },
            malformed: false,
        },
        None => ImportedEvent {
            event: NewEvent {
                ts: at,
                task: None,
                kind: "malformed".to_owned(),
                actor: None,
                via: None,
                run: None,
                detail: line.text.clone().into(),
            },
            malformed: true,
        },
    })
}

pub(super) fn import(
    backend: &SqliteBackend,
    lines: &[Line],
    report: &mut ImportReport,
) -> Result<()> {
    let at = jiff::Timestamp::now();
    let events = lines
        .iter()
        .map(|line| imported(line, at))
        .collect::<Result<Vec<_>>>()?;
    report.events = events.len() as u64;
    report.malformed_events = events.iter().filter(|e| e.malformed).count() as u64;
    apply(
        backend,
        ImportBatch {
            blobs: Vec::new(),
            tasks: Vec::new(),
            events,
            records: Vec::new(),
        },
        "events",
    )
}

/// Every legacy line is imported exactly once, in order, with its decoded content.
pub(super) fn verify(corral: &Path, backend: &SqliteBackend) -> Result<()> {
    let lines = read(corral)?;
    let view = backend.view()?;
    let stored =
        crate::backend::conformance::all_pages(|page| view.events(&EventQuery::default(), page))?;
    ensure!(
        stored.len() == lines.len(),
        "{} legacy event lines but {} imported events",
        lines.len(),
        stored.len()
    );
    for (n, (line, event)) in lines.iter().zip(&stored).enumerate() {
        let expected = imported(line, event.event.ts)?;
        let mut want = expected.event;
        want.run = want
            .detail
            .get("run")
            .and_then(|run| run.as_str())
            .map(str::to_owned);
        ensure!(
            event.event == want && event.malformed == expected.malformed,
            "event line {} was not imported verbatim",
            n + 1
        );
    }
    Ok(())
}
