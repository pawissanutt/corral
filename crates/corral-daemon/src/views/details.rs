use std::{
    collections::BTreeSet,
    io::{Read, Seek, SeekFrom},
    mem,
};

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use corral_engine::{
    Engine, EventKind, artifacts::ArtifactFilter, events::EventFilter, store::TaskFilter,
};
use corral_protocol::detail::*;
use jiff::Timestamp;
use serde::Serialize;

use super::{
    NotFound, REPLY_MAX, citation, fingerprint, json_len, review, rows, task_number, terminal,
};

const ROW_LIMIT: usize = 100;
const TEXT_BYTES: usize = 8 * 1024; // Worst-case JSON escaping still fits CONTROL.
const BINARY_BYTES: usize = 32 * 1024;
/// Bytes of an artifact's head read to tell text from binary before its first page.
pub(super) const INSPECT_BYTES: u64 = 64 * 1024;

pub(super) fn read(engine: &Engine, query: DetailGet) -> Result<DetailReply> {
    if let DetailRef::Terminal { id } = &query.target {
        ensure!(
            query.page.is_none(),
            "invalid: terminal does not accept a page token"
        );
        let snapshot = terminal(engine, id)?;
        let etag = fingerprint(&(
            &query.target,
            &snapshot.lines,
            &snapshot.rows,
            snapshot.live,
        ))?;
        return reply(
            &query,
            engine.store.latest_seq()?,
            etag,
            DetailBody::Terminal(snapshot),
            None,
        );
    }
    engine.store.read_snapshot(|| {
        let seq = engine.store.latest_seq()?;
        match &query.target {
            DetailRef::Artifact { reference } => {
                return citation::artifact(engine, &query, seq, reference);
            }
            DetailRef::Artifacts { filter, narrowing } => {
                return artifacts(engine, &query, seq, filter.as_deref(), narrowing.as_ref());
            }
            DetailRef::ReviewFile { task, path } => {
                return review::file(engine, &query, seq, task, path);
            }
            DetailRef::Closed { before } => return closed(engine, &query, seq, before.as_deref()),
            _ => {}
        }
        let mut body = body(engine, &query.target)?;
        let etag = fingerprint(&(&query.target, &body))?;
        if query.page.is_none() && query.etag.as_ref() == Some(&etag) {
            return reply(&query, seq, etag, body, None);
        }
        let offset = offset(query.page.as_deref(), &etag)?;
        let next = paginate(&mut body, offset)?.map(|next| format!("{etag}:{next}"));
        reply(&query, seq, etag, body, next)
    })
}

fn body(engine: &Engine, target: &DetailRef) -> Result<DetailBody> {
    let store = &engine.store;
    Ok(match target {
        DetailRef::Citation { reference } => {
            DetailBody::Citation(citation::citation(engine, reference)?)
        }
        DetailRef::Task { id } => {
            // `not_found`, not a refusal: a client tells a cited task that does not exist from
            // a read that failed.
            let task = store.get_task(id)?.ok_or(NotFound("task not found"))?;
            let mut events = store.event_page(Some(id), None, None, true, 50)?;
            events.reverse();
            DetailBody::Task(TaskDetail {
                task,
                runs: store.list_runs(id)?,
                checks: store.list_checks(id)?,
                artifacts: store.list_artifacts(Some(id))?,
                events,
            })
        }
        DetailRef::Plan { id } => {
            let parent = store.require_task(id)?;
            let mut child_rows = rows::task_rows(
                engine,
                store.list_tasks(&TaskFilter {
                    parent: Some(id.clone()),
                    ..Default::default()
                })?,
            )?;
            // A child is sent twice here, so one long document would outgrow a reply. Its own
            // task detail serves it.
            for row in &mut child_rows {
                row.task.body.clear();
            }
            let children = child_rows.iter().map(|row| row.task.clone()).collect();
            DetailBody::Plan(PlanDetail {
                parent,
                children,
                child_rows: Some(child_rows),
            })
        }
        DetailRef::Review { task } => DetailBody::Review(review::detail(engine, task)?),
        DetailRef::Timeline { from, to, by } => {
            ensure!(
                *by != TimelineBy::Other,
                "invalid: unknown timeline grouping"
            );
            let from: Timestamp = from.parse().context("invalid: timeline start timestamp")?;
            let to: Timestamp = to.parse().context("invalid: timeline end timestamp")?;
            ensure!(from < to, "invalid: timeline start must precede end");
            let mut runs = store.all_runs()?;
            runs.retain(|run| run.started < to && run.ended.is_none_or(|end| end > from));
            DetailBody::Timeline(TimelinePage {
                runs,
                events: store.events_in_window(from, to)?,
            })
        }
        DetailRef::Burnup { scope } => DetailBody::Burnup(burnup(engine, scope)?),
        DetailRef::UsageSeries {
            from,
            to,
            by,
            utc_offset_minutes,
            time_zone,
        } => {
            let window = corral_engine::usage::UsageWindow::parse(
                from,
                to,
                *utc_offset_minutes,
                time_zone.as_deref(),
            )
            .map_err(|error| anyhow::anyhow!("invalid: {error:#}"))?;
            DetailBody::UsageSeries(UsageSeries {
                days: engine.usage_series(window, *by)?,
            })
        }
        _ => bail!("invalid: unsupported detail"),
    })
}

fn artifacts(
    engine: &Engine,
    query: &DetailGet,
    seq: u64,
    words: Option<&str>,
    narrowing: Option<&ArtifactNarrowing>,
) -> Result<DetailReply> {
    let selector = fingerprint(&query.target)?;
    let before = query
        .page
        .as_deref()
        .map(|token| {
            let (key, anchor) = token
                .split_once(':')
                .context("invalid: artifact page token")?;
            ensure!(
                key == selector,
                "invalid: artifact page belongs to a different selection"
            );
            let (at, reference) = anchor
                .split_once('/')
                .context("invalid: artifact page anchor")?;
            corral_engine::artifacts::parse_reference(reference)
                .context("invalid: artifact page reference")?;
            Ok::<_, anyhow::Error>((
                at.parse().context("invalid: artifact page timestamp")?,
                reference,
            ))
        })
        .transpose()?;
    let filter = ArtifactFilter {
        all: narrowing.is_some_and(|filters| filters.all),
        words,
        kind: narrowing.and_then(|filters| filters.kind.as_deref()),
        task: narrowing.and_then(|filters| filters.task.as_deref()),
        project: narrowing.and_then(|filters| filters.project.as_deref()),
    };
    let mut page = RowPage::new(0);
    let rows = page.take(
        engine
            .store
            .artifacts_before(filter, before, ROW_LIMIT + 1)?,
    )?;
    let next = if page.more {
        rows.last()
            .map(|row| format!("{selector}:{}/{}", row.at, row.reference()))
    } else {
        None
    };
    let body = DetailBody::Artifacts(ArtifactsPage { rows });
    let etag = fingerprint(&(&query.target, &query.page, &body, &next))?;
    let unchanged = query.etag.as_ref() == Some(&etag);
    reply_page(seq, etag, body, next, unchanged)
}

fn burnup(engine: &Engine, scope: &str) -> Result<Burnup> {
    let store = &engine.store;
    let mut filter = EventFilter {
        kinds: vec![
            EventKind::Created,
            EventKind::Landed,
            EventKind::Dropped,
            EventKind::Reopened,
        ],
        ..Default::default()
    };
    let mut members = None;
    if let Some(project) = scope.strip_prefix("project:") {
        ensure!(!project.is_empty(), "invalid: burnup project required");
        filter.project = Some(project.to_owned());
    } else if let Some(parent) = scope.strip_prefix("plan:") {
        store.require_task(parent)?;
        members = Some(
            store
                .list_tasks(&TaskFilter {
                    parent: Some(parent.to_owned()),
                    ..Default::default()
                })?
                .into_iter()
                .map(|task| task.id)
                .collect::<BTreeSet<_>>(),
        );
    } else {
        ensure!(scope == "workspace", "invalid: unsupported burnup scope");
    }
    let mut events = store.events_after(&filter, 0)?;
    if let Some(members) = members {
        events.retain(|event| event.task.as_ref().is_some_and(|id| members.contains(id)));
    }
    Ok(Burnup {
        scope: scope.to_owned(),
        events,
    })
}

fn closed(
    engine: &Engine,
    query: &DetailGet,
    seq: u64,
    anchor: Option<&str>,
) -> Result<DetailReply> {
    ensure!(query.page.is_none(), "invalid: use the closed older anchor");
    let before = anchor
        .map(|anchor| {
            ensure!(
                anchor.len() <= CLOSED_ANCHOR_MAX,
                "invalid: oversized closed anchor"
            );
            let (at, id) = anchor.split_once('/').context("invalid: closed anchor")?;
            Ok::<_, anyhow::Error>((
                at.parse().context("invalid: closed anchor timestamp")?,
                task_number(id)?,
            ))
        })
        .transpose()?;
    let total = Some(engine.store.closed_task_count()?);
    let rows = engine
        .store
        .closed_tasks_before(before, CLOSED_PAGE_MAX as u16 + 1)?;
    let mut page = RowPage::new(0);
    page.rows_left = CLOSED_PAGE_MAX as usize;
    let page_rows = page.take(rows)?;
    let older = if page.more {
        page_rows
            .last()
            .map(|row| format!("{}/{}", row.updated, row.id))
    } else {
        None
    };
    let body = DetailBody::Closed(ClosedPage {
        total,
        rows: page_rows,
        older,
    });
    let etag = fingerprint(&(&query.target, &body))?;
    reply(query, seq, etag, body, None)
}

pub(super) fn offset(token: Option<&str>, etag: &str) -> Result<usize> {
    let Some(token) = token else { return Ok(0) };
    let (key, offset) = token
        .split_once(':')
        .context("invalid: malformed detail page token")?;
    ensure!(
        key == etag,
        "invalid: detail page changed or belongs to a different selection"
    );
    let offset = offset
        .parse()
        .context("invalid: malformed detail page offset")?;
    ensure!(offset != 0, "invalid: detail page offset must advance");
    Ok(offset)
}

pub(super) fn reply(
    query: &DetailGet,
    seq: u64,
    etag: String,
    body: DetailBody,
    next: Option<String>,
) -> Result<DetailReply> {
    let unchanged = query.page.is_none() && query.etag.as_ref() == Some(&etag);
    reply_page(seq, etag, body, next, unchanged)
}

fn reply_page(
    seq: u64,
    etag: String,
    body: DetailBody,
    next: Option<String>,
    unchanged: bool,
) -> Result<DetailReply> {
    let reply = DetailReply {
        etag,
        as_of: Timestamp::now().to_string(),
        seq,
        next: if unchanged { None } else { next },
        body: if unchanged { None } else { Some(body) },
    };
    ensure!(
        json_len(&reply)? <= REPLY_MAX,
        "too_large: detail reply exceeds CONTROL budget"
    );
    Ok(reply)
}

struct RowPage {
    skip: usize,
    emitted: usize,
    rows_left: usize,
    bytes_left: usize,
    more: bool,
}

impl RowPage {
    fn new(offset: usize) -> Self {
        Self {
            skip: offset,
            emitted: 0,
            rows_left: ROW_LIMIT,
            bytes_left: REPLY_MAX - 1024,
            more: false,
        }
    }

    fn reserve(&mut self, base: &impl Serialize) -> Result<()> {
        self.bytes_left = self
            .bytes_left
            .checked_sub(json_len(base)?)
            .context("too_large: detail metadata exceeds CONTROL budget")?;
        Ok(())
    }

    fn take<T: Serialize>(&mut self, rows: Vec<T>) -> Result<Vec<T>> {
        let start = self.skip.min(rows.len());
        self.skip -= start;
        if self.more || start == rows.len() {
            return Ok(Vec::new());
        }
        let mut count = 0;
        for row in &rows[start..] {
            let size = json_len(row)? + 1;
            if count == self.rows_left || size > self.bytes_left {
                ensure!(
                    self.emitted + count != 0,
                    "too_large: one detail row exceeds CONTROL budget"
                );
                self.more = true;
                break;
            }
            self.bytes_left -= size;
            count += 1;
        }
        self.emitted += count;
        self.rows_left -= count;
        Ok(rows.into_iter().skip(start).take(count).collect())
    }
}

pub(super) fn paginate(body: &mut DetailBody, offset: usize) -> Result<Option<usize>> {
    let mut page = RowPage::new(offset);
    macro_rules! paginate_fields {
        ($model:ident, $($field:ident),+ $(,)?) => {{
            $(let $field = mem::take(&mut $model.$field);)+
            page.reserve($model)?;
            $($model.$field = page.take($field)?;)+
        }};
    }
    match body {
        DetailBody::Task(model) => paginate_fields!(model, runs, checks, artifacts, events),
        DetailBody::Plan(model) => {
            let children = mem::take(&mut model.children);
            let child_rows = model
                .child_rows
                .as_mut()
                .context("plan child rows missing")?;
            let child_rows = mem::take(child_rows);
            page.reserve(model)?;
            let pairs = children.into_iter().zip(child_rows).collect();
            let (children, rows) = page.take(pairs)?.into_iter().unzip();
            model.children = children;
            model.child_rows = Some(rows);
        }
        DetailBody::Review(model) => paginate_fields!(model, checks, files),
        DetailBody::SystemMap(model) => paginate_fields!(model, rows),
        DetailBody::SystemRuntime(model) => paginate_fields!(model, rows),
        DetailBody::SystemData(model) => paginate_fields!(model, rows),
        DetailBody::SystemLenses(model) => paginate_fields!(model, rows),
        DetailBody::SystemReview(model) => paginate_fields!(model, rows),
        DetailBody::Timeline(model) => paginate_fields!(model, runs, events),
        DetailBody::Burnup(model) => paginate_fields!(model, events),
        DetailBody::UsageSeries(model) => paginate_fields!(model, days),
        _ => ensure!(offset == 0, "invalid: unexpected detail page token"),
    }
    ensure!(
        offset == 0 || page.emitted != 0,
        "invalid: detail page is past the end"
    );
    Ok(page.more.then_some(offset + page.emitted))
}

pub(super) struct ArtifactSource<'a> {
    pub media_type: Option<&'a str>,
    /// Whether the content is text, when the caller already knows (a cited section the
    /// engine read as UTF-8 without NULs); otherwise the first page inspects it.
    pub text: Option<bool>,
    pub fingerprint: String,
    pub bytes: u64,
    pub task: String,
    pub key: String,
}

pub(super) fn artifact_page(
    query: &DetailGet,
    seq: u64,
    mut file: impl Read + Seek,
    source: ArtifactSource<'_>,
) -> Result<DetailReply> {
    let ArtifactSource {
        media_type,
        text: known,
        fingerprint,
        bytes: total,
        task,
        key,
    } = source;
    let text = match query.page.as_deref() {
        Some(token) => match token.as_bytes().first() {
            Some(b't') => true,
            Some(b'b') => false,
            _ => bail!("invalid: artifact page encoding"),
        },
        None if known.is_some() => known.unwrap_or_default(),
        None if media_type.is_some_and(|media| {
            (media.starts_with("image/") && media != "image/svg+xml") || media == "application/pdf"
        }) =>
        {
            false
        }
        // Only the head is inspected: a large log's first page must not wait for a read to EOF.
        None => is_text(&mut (&mut file).take(INSPECT_BYTES), total > INSPECT_BYTES)?,
    };
    let etag = format!("{}{}", if text { 't' } else { 'b' }, fingerprint);
    let at = offset(query.page.as_deref(), &etag)? as u64;
    ensure!(
        at == 0 || at < total,
        "invalid: artifact page is past the end"
    );
    let take = (total - at).min(if text { TEXT_BYTES } else { BINARY_BYTES } as u64);
    file.seek(SeekFrom::Start(at))
        .context("seek to artifact page")?;
    let mut bytes = vec![0; take as usize];
    file.read_exact(&mut bytes).context("read artifact page")?;
    // Past the inspected head a text stream may hold bytes that are not UTF-8: that page
    // travels as bytes, so every page still carries the artifact's exact bytes.
    let text_size = match std::str::from_utf8(&bytes) {
        _ if !text => None,
        Ok(_) => Some(bytes.len()),
        Err(error)
            if error.error_len().is_none() && at + take < total && error.valid_up_to() > 0 =>
        {
            Some(error.valid_up_to())
        }
        Err(_) => None,
    };
    let (encoding, content, size) = if let Some(size) = text_size {
        bytes.truncate(size);
        (ArtifactEncoding::Text, String::from_utf8(bytes)?, size)
    } else {
        (
            ArtifactEncoding::Base64,
            STANDARD.encode(&bytes),
            bytes.len(),
        )
    };
    let next_at = at + size as u64;
    let truncated = next_at < total;
    ensure!(
        !truncated || size != 0,
        "invalid: artifact page did not advance"
    );
    let next = truncated.then(|| format!("{etag}:{next_at}"));
    let body = ArtifactPage {
        task,
        key,
        media_type: media_type.map(str::to_owned),
        encoding,
        content,
        truncated,
    };
    reply(query, seq, etag, DetailBody::Artifact(body), next)
}

/// Whether `reader` (the content's head) is UTF-8 without NULs; `more` says content follows it,
/// so a character cut at its end still counts.
fn is_text(reader: &mut impl Read, more: bool) -> Result<bool> {
    let mut buf = [0_u8; 8 * 1024];
    let mut carry = 0;
    loop {
        let read = reader
            .read(&mut buf[carry..])
            .context("inspect artifact encoding")?;
        if read == 0 {
            return Ok(carry == 0 || more);
        }
        let len = carry + read;
        if buf[..len].contains(&0) {
            return Ok(false);
        }
        carry = match std::str::from_utf8(&buf[..len]) {
            Ok(_) => 0,
            Err(error) if error.error_len().is_none() => {
                let end = error.valid_up_to();
                buf.copy_within(end..len, 0);
                len - end
            }
            Err(_) => return Ok(false),
        };
    }
}

pub(super) fn media_type(title: &str) -> Option<&'static str> {
    Some(
        match title.rsplit_once('.')?.1.to_ascii_lowercase().as_str() {
            "md" => "text/markdown",
            "html" | "htm" => "text/html",
            "txt" | "log" | "patch" | "diff" => "text/plain",
            "json" => "application/json",
            "csv" => "text/csv",
            "svg" => "image/svg+xml",
            "pdf" => "application/pdf",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            _ => return None,
        },
    )
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, io::Cursor, rc::Rc};

    use super::*;

    /// A blob that counts the bytes read from it.
    struct Counted<'a> {
        inner: Cursor<&'a [u8]>,
        read: Rc<Cell<u64>>,
    }

    impl Read for Counted<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let read = self.inner.read(buf)?;
            self.read.set(self.read.get() + read as u64);
            Ok(read)
        }
    }

    impl Seek for Counted<'_> {
        fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(to)
        }
    }

    /// A large UTF-8 log must serve its head without being read to EOF, and a byte that is not
    /// UTF-8 past the inspected head must still reach the reader exactly.
    #[test]
    fn a_large_log_serves_its_head_without_reading_to_eof_and_every_byte_after() {
        let mut blob = "a line of the log\n".repeat(30_000).into_bytes();
        blob[300_000] = 0xff;
        let page = |token: Option<String>| {
            let read = Rc::new(Cell::new(0));
            let file = Counted {
                inner: Cursor::new(blob.as_slice()),
                read: read.clone(),
            };
            let query = DetailGet {
                target: DetailRef::Artifact {
                    reference: "T-1:1".into(),
                },
                etag: None,
                page: token,
            };
            let source = ArtifactSource {
                media_type: None,
                text: None,
                fingerprint: "f".into(),
                bytes: blob.len() as u64,
                task: "T-1".into(),
                key: "1".into(),
            };
            let reply = artifact_page(&query, 1, file, source).unwrap();
            (reply, read.get())
        };
        let (first, read) = page(None);
        assert!(
            read <= INSPECT_BYTES + TEXT_BYTES as u64,
            "read {read} bytes"
        );
        let mut got = Vec::new();
        let mut next = Some(first);
        while let Some(reply) = next.take() {
            let Some(DetailBody::Artifact(part)) = reply.body else {
                panic!("expected an artifact page");
            };
            match part.encoding {
                ArtifactEncoding::Text => got.extend(part.content.into_bytes()),
                _ => got.extend(STANDARD.decode(part.content).unwrap()),
            }
            next = reply.next.map(|token| page(Some(token)).0);
        }
        assert_eq!(got, blob);
    }
}
