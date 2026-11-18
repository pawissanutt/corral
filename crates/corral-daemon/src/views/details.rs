use std::{
    collections::BTreeSet,
    io::{Read, Seek, SeekFrom},
    mem,
};

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use corral_engine::{Engine, EventKind, events::EventFilter, store::TaskFilter};
use corral_protocol::detail::*;
use jiff::Timestamp;
use serde::Serialize;

use super::{REPLY_MAX, fingerprint, json_len, review, task_number, terminal};

const ROW_LIMIT: usize = 100;
const TEXT_BYTES: usize = 8 * 1024; // Worst-case JSON escaping still fits CONTROL.
const BINARY_BYTES: usize = 32 * 1024;

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
            DetailRef::Artifact { reference } => return artifact(engine, &query, seq, reference),
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
        DetailRef::Task { id } => {
            let task = store.require_task(id)?;
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
        DetailRef::Plan { id } => DetailBody::Plan(PlanDetail {
            parent: store.require_task(id)?,
            children: store.list_tasks(&TaskFilter {
                parent: Some(id.clone()),
                ..Default::default()
            })?,
        }),
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
        DetailRef::UsageSeries { by } => DetailBody::UsageSeries(UsageSeries {
            summary: engine.usage_summary(*by)?,
            rows: store.list_usage()?,
            runs: store.all_runs()?,
        }),
        DetailRef::Artifacts { filter } => {
            let mut rows = store.list_artifacts(None)?;
            if let Some(filter) = filter.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                let needle = filter.to_lowercase();
                rows.retain(|row| {
                    row.title.to_lowercase().contains(&needle)
                        || row
                            .kind
                            .as_ref()
                            .is_some_and(|kind| kind.to_lowercase().contains(&needle))
                        || row.reference().to_ascii_lowercase().contains(&needle)
                });
            }
            DetailBody::Artifacts(ArtifactsPage { rows })
        }
        _ => bail!("invalid: unsupported detail"),
    })
}

fn burnup(engine: &Engine, scope: &str) -> Result<Burnup> {
    let store = &engine.store;
    let mut filter = EventFilter {
        kinds: vec![EventKind::Created, EventKind::Landed],
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

fn paginate(body: &mut DetailBody, offset: usize) -> Result<Option<usize>> {
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
        DetailBody::Plan(model) => paginate_fields!(model, children),
        DetailBody::Review(model) => paginate_fields!(model, files),
        DetailBody::Timeline(model) => paginate_fields!(model, runs, events),
        DetailBody::Burnup(model) => paginate_fields!(model, events),
        DetailBody::UsageSeries(model) => paginate_fields!(model, rows, runs),
        DetailBody::Artifacts(model) => paginate_fields!(model, rows),
        _ => ensure!(offset == 0, "invalid: unexpected detail page token"),
    }
    ensure!(
        offset == 0 || page.emitted != 0,
        "invalid: detail page is past the end"
    );
    Ok(page.more.then_some(offset + page.emitted))
}

fn artifact(engine: &Engine, query: &DetailGet, seq: u64, reference: &str) -> Result<DetailReply> {
    let artifact = engine
        .store
        .get_artifact(reference)?
        .context("artifact reference not found")?;
    let media_type = media_type(&artifact.title);
    let mut file = engine.open_artifact_blob(&artifact)?;
    let text = match query.page.as_deref() {
        Some(token) => match token.as_bytes().first() {
            Some(b't') => true,
            Some(b'b') => false,
            _ => bail!("invalid: artifact page encoding"),
        },
        None if media_type.is_some_and(|media| {
            (media.starts_with("image/") && media != "image/svg+xml") || media == "application/pdf"
        }) =>
        {
            false
        }
        None => is_text(&mut file)?,
    };
    let etag = format!(
        "{}{}",
        if text { 't' } else { 'b' },
        fingerprint(&(&query.target, &artifact))?
    );
    let at = offset(query.page.as_deref(), &etag)? as u64;
    ensure!(
        at == 0 || at < artifact.bytes,
        "invalid: artifact page is past the end"
    );
    let take = (artifact.bytes - at).min(if text { TEXT_BYTES } else { BINARY_BYTES } as u64);
    file.seek(SeekFrom::Start(at))
        .context("seek to artifact page")?;
    let mut bytes = vec![0; take as usize];
    file.read_exact(&mut bytes).context("read artifact page")?;
    let (encoding, content, size) = if text {
        let size = match std::str::from_utf8(&bytes) {
            Ok(_) => bytes.len(),
            Err(error) if error.error_len().is_none() && at + take < artifact.bytes => {
                error.valid_up_to()
            }
            Err(error) => return Err(error).context("invalid: artifact text page is not UTF-8"),
        };
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
    let truncated = next_at < artifact.bytes;
    ensure!(
        !truncated || size != 0,
        "invalid: artifact page did not advance"
    );
    let next = truncated.then(|| format!("{etag}:{next_at}"));
    let body = ArtifactPage {
        task: artifact.task,
        key: artifact.id.to_string(),
        media_type: media_type.map(str::to_owned),
        encoding,
        content,
        truncated,
    };
    reply(query, seq, etag, DetailBody::Artifact(body), next)
}

fn is_text(reader: &mut impl Read) -> Result<bool> {
    let mut buf = [0_u8; 8 * 1024];
    let mut carry = 0;
    loop {
        let read = reader
            .read(&mut buf[carry..])
            .context("inspect artifact encoding")?;
        if read == 0 {
            return Ok(carry == 0);
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

fn media_type(title: &str) -> Option<&'static str> {
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
