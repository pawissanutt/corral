//! Read-only, bounded application projections over the simple engine.
mod chat;
mod citation;
mod details;
mod diagram;
mod diagram_compare;
pub(crate) use diagram_compare::ComparedHeads;
mod review;
pub(crate) mod rows;
mod session_files;
mod session_parts;
mod system;
mod terminal_screen;
pub(crate) mod transcript;

#[cfg(test)]
mod board_wire_tests;
#[cfg(test)]
mod cite_tests;

use std::{
    hash::{DefaultHasher, Hasher},
    io::{self, Read, Seek, SeekFrom, Write},
};

use anyhow::{Context, Result, ensure};
use corral_engine::{Engine, SessionRef};
use corral_protocol::{
    app::{APP_CONTROL_MAX, CHANGES_LIMIT_MAX, ChangesPage, ChangesQuery, SeenCursor},
    detail::{DetailGet, DetailReply, TERMINAL_ARRAYS_MAX, TERMINAL_ROWS_MAX, TerminalSnapshot},
};
use serde::Serialize;

use crate::state::Shared;
pub(crate) use rows::{RowCache, Rows};
pub(crate) use session_files::StaleWorktreeFile;
pub(crate) use system::SystemFeatures;
pub(crate) use transcript::StaleTranscriptCursor;

// The transport reserves room for the response envelope and bounded request id.
const REPLY_MAX: usize = APP_CONTROL_MAX - 2048;

#[derive(Debug, Clone, Copy)]
pub(crate) struct NotFound(pub(crate) &'static str);

impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for NotFound {}

#[cfg(test)]
fn detail(shared: &Shared, query: DetailGet) -> Result<DetailReply> {
    detail_for_client(shared, query, SystemFeatures::default())
}

pub(crate) fn detail_for_client(
    shared: &Shared,
    query: DetailGet,
    system_features: SystemFeatures,
) -> Result<DetailReply> {
    if let Some(reply) = system::read(shared, &query, system_features)? {
        return Ok(reply);
    }
    let engine = shared.engine()?;
    if let Some(reply) = diagram::read(shared, &engine, &query)? {
        return Ok(reply);
    }
    if let Some(reply) = session_files::read(shared, &engine, &query)? {
        return Ok(reply);
    }
    if let Some(reply) = session_parts::read(shared, &engine, &query)? {
        return Ok(reply);
    }
    if let Some(reply) = chat::detail(shared, &engine, &query)? {
        return Ok(reply);
    }
    details::read(&engine, query)
}

pub(crate) struct ChangesRequest {
    pub query: ChangesQuery,
    pub seen: SeenCursor,
}

pub(crate) fn changes(shared: &Shared, request: ChangesRequest) -> Result<ChangesPage> {
    let ChangesRequest { query, seen } = request;
    ensure!(
        (1..=CHANGES_LIMIT_MAX).contains(&query.limit),
        "invalid: changes limit must be 1..={CHANGES_LIMIT_MAX}"
    );
    ensure!(
        !matches!((query.after, query.before), (Some(after), Some(before)) if after >= before),
        "invalid: changes after must precede before"
    );
    let engine = shared.engine()?;
    let store = &engine.store;
    store.read_snapshot(|| {
        let head = store.latest_seq()?;
        // All simple-engine events are owner-visible; cursors are raw sequences,
        // never inferred from the count or from the current page.
        let total = store.event_count_after(0)?;
        let unseen = store.event_count_after(seen.through)?;
        let mut events = store.event_page(
            None,
            query.after,
            query.before,
            query.after.is_none(),
            query.limit,
        )?;
        let mut bytes = REPLY_MAX - 1024;
        let mut count = 0;
        for event in &events {
            let need = json_len(event)? + 1;
            if need > bytes {
                ensure!(count > 0, "too_large: one change exceeds CONTROL budget");
                break;
            }
            bytes -= need;
            count += 1;
        }
        events.truncate(count);
        let older = if let Some(seq) = events.iter().map(|event| event.seq).min() {
            (!store.event_page(None, None, Some(seq), true, 1)?.is_empty()).then_some(seq)
        } else {
            None
        };
        Ok(ChangesPage {
            events,
            head,
            older,
            seen,
            total: Some(total),
            unseen: Some(unseen),
        })
    })
}

pub(crate) fn run_scope(id: &str) -> Result<(&str, &str)> {
    run_scope_with_separator(id, '_')
}

pub(crate) fn run_scope_with_separator(id: &str, separator: char) -> Result<(&str, &str)> {
    ensure!(id.len() <= 32, "invalid: terminal scope is too long");
    let (task, run) = id
        .split_once(separator)
        .context("invalid: expected terminal scope T-001_R1")?;
    task_number(task)?;
    ensure!(
        run.strip_prefix('R')
            .and_then(|id| id.parse::<u64>().ok())
            .is_some_and(|n| n > 0)
            && run.len() <= 20
            && run[1..].bytes().all(|byte| byte.is_ascii_digit()),
        "invalid: expected terminal scope T-001_R1"
    );
    Ok((task, run))
}

fn task_number(id: &str) -> Result<u32> {
    id.strip_prefix("T-")
        .filter(|value| !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|number| number.parse().ok())
        .context("invalid: expected a task number")
}

pub(crate) fn terminal(engine: &Engine, id: &str) -> Result<TerminalSnapshot> {
    let (task, run) = run_scope(id)?;
    let run = engine
        .store
        .get_run(task, run)?
        .context("terminal run not found")?;
    placed_terminal(
        engine,
        &run.session.context("run has no terminal session")?,
        run.ended.is_none(),
    )
}

fn placed_terminal(engine: &Engine, session: &SessionRef, live: bool) -> Result<TerminalSnapshot> {
    match session {
        SessionRef::Pi { inner, .. } => placed_terminal(engine, inner, live),
        SessionRef::Herdr { pane, .. } => {
            let text = crate::health::herdr(&engine.config.runner)
                .pane_terminal(pane, TERMINAL_ROWS_MAX)?
                .context("terminal pane is no longer available")?;
            terminal_screen::snapshot(&text, live)
        }
        SessionRef::Process { dir, .. } => {
            let path = dir.join("session.log");
            let mut file = match std::fs::File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(NotFound("session log not found").into());
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("open session log {}", path.display()));
                }
            };
            let len = file.metadata().context("stat session log")?.len();
            let start = len.saturating_sub(TERMINAL_ARRAYS_MAX as u64);
            file.seek(SeekFrom::Start(start))
                .context("seek to session log tail")?;
            let mut bytes = Vec::with_capacity((len - start) as usize);
            file.take(len - start)
                .read_to_end(&mut bytes)
                .context("read session log tail")?;
            let text = String::from_utf8_lossy(&bytes);
            let text = if start != 0 {
                text.split_once('\n')
                    .map_or(text.as_ref(), |(_, rest)| rest)
            } else {
                text.as_ref()
            };
            let exited = dir
                .join("exit_code")
                .try_exists()
                .context("inspect process exit marker")?;
            terminal_screen::snapshot(text, live && !exited)
        }
    }
}

pub(crate) fn json_len(value: &impl Serialize) -> Result<usize> {
    #[derive(Default)]
    struct Count(usize);
    impl Write for Count {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut size = Count::default();
    serde_json::to_writer(&mut size, value)?;
    Ok(size.0)
}

fn fingerprint(value: &impl Serialize) -> Result<String> {
    #[derive(Default)]
    struct Digest(DefaultHasher);
    impl Write for Digest {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.write(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut digest = Digest::default();
    serde_json::to_writer(&mut digest, value)?;
    Ok(format!("{:016x}", digest.0.finish()))
}

#[cfg(test)]
pub(crate) mod tests;
