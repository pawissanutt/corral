//! App-safe, typed read models over canonical operations and the durable Store.
//!
//! Detail tokens are selector/content-bound and never authorize a filesystem path.
//! Pages repeat scalar metadata; clients append collections recursively, merging
//! canonical `id`/`day` parents and replacing fixed task rails. Patch text appends;
//! artifact base64 chunks decode independently before byte concatenation. A changed
//! source rejects continuation. Topic owners cache rows by feed head, not loop ticks.
mod agents;
mod artifacts;
mod attachments;
mod changes;
mod paging;
mod plans;
mod questions;
mod review;
mod settings;
mod tasks;
mod timeline;
mod usage;

#[cfg(test)]
mod tests;

use anyhow::{Result, bail};
use corral_protocol::detail::{DetailGet, DetailRef, DetailReply};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::Ctx;

pub use changes::changes;

/// The durable change-feed checkpoint, not a task/question owner revision.
pub fn sequence(ctx: &Ctx) -> Result<u64> {
    Ok(ctx.store.view()?.seq().0)
}

macro_rules! rows {
    ($name:ident, $module:ident, $row:ident) => {
        /// Read a typed topic snapshot on a short, coherent Store view.
        pub fn $name(ctx: &Ctx) -> Result<Vec<corral_protocol::rows::$row>> {
            ctx.read_step(|ctx| $module::$name(ctx))
        }
    };
}
rows!(inbox_rows, questions, InboxRow);
rows!(plans_rows, plans, PlanRow);
rows!(agents_rows, agents, AgentRow);
rows!(assistant_rows, agents, AssistantRow);
rows!(usage_rows, usage, UsageRow);
rows!(settings_rows, settings, SettingsSnapshot);

/// Resolve one bounded typed detail page. `body == None` means not modified.
/// Unknown selectors, missing canonical IDs and invalid/stale tokens fail closed.
/// The gateway supplies authenticated admission; no caller-controlled host paths,
/// environment or executable identity are accepted here.
pub fn read(ctx: &Ctx, request: &DetailGet) -> Result<DetailReply> {
    validate(request)?;
    ctx.read_step(|ctx| read_snapshot(ctx, request))
}

fn read_snapshot(ctx: &Ctx, request: &DetailGet) -> Result<DetailReply> {
    let seq = sequence(ctx)?;
    let selector = digest(&request.target)?;
    let cursor = paging::decode(request.page.as_deref(), &selector)?;
    let offset = cursor.as_ref().map_or(0, |page| page.offset);
    let as_of = cursor.as_ref().map_or_else(now, |page| page.as_of.clone());
    if let DetailRef::Attachment { entry, id } = &request.target {
        if cursor.as_ref().is_some_and(|page| page.source.is_some()) {
            bail!("invalid attachment page token");
        }
        let attachment = attachments::select(ctx, entry, id)?;
        let etag = attachment
            .sha256
            .clone()
            .ok_or_else(|| anyhow::anyhow!("not_found: attachment has no bytes"))?;
        paging::check(cursor.as_ref(), &etag)?;
        if cursor.is_none() && request.etag.as_deref() == Some(etag.as_str()) {
            return Ok(DetailReply {
                etag,
                as_of,
                seq,
                next: None,
                body: None,
            });
        }
        let (body, next) = attachments::page(ctx, entry, id, &attachment, offset)?;
        return paging::reply(body, next, etag, selector, as_of, seq);
    }
    if let DetailRef::Artifact { task, key } = &request.target {
        if cursor.as_ref().is_some_and(|page| page.source.is_some()) {
            bail!("invalid artifact page token");
        }
        let artifact = artifacts::select(ctx, task, key)?;
        let etag = digest(&(&request.target, artifacts::identity(&artifact)?))?;
        paging::check(cursor.as_ref(), &etag)?;
        if cursor.is_none() && request.etag.as_deref() == Some(etag.as_str()) {
            return Ok(DetailReply {
                etag,
                as_of,
                seq,
                next: None,
                body: None,
            });
        }
        let (body, next) = artifacts::page(ctx, task, key, &artifact, offset)?;
        return paging::reply(body, next, etag, selector, as_of, seq);
    }
    let patch = matches!(
        &request.target,
        DetailRef::PlanDiff { .. } | DetailRef::ReviewFile { .. }
    );
    if patch {
        if cursor.as_ref().is_some_and(|page| page.source.is_some()) {
            bail!("invalid patch page token");
        }
        let candidate = match &request.target {
            DetailRef::ReviewFile { task, .. } => Some(review::patch_revision(ctx, task)?),
            _ => None,
        };
        let etag = digest(&(seq, &request.target, &candidate))?;
        paging::check(cursor.as_ref(), &etag)?;
        if cursor.is_none() && request.etag.as_deref() == Some(etag.as_str()) {
            return Ok(DetailReply {
                etag,
                as_of,
                seq,
                next: None,
                body: None,
            });
        }
        let (body, next) = match &request.target {
            DetailRef::PlanDiff { id, from, to } => plans::plan_diff(ctx, id, from, to, offset)?,
            DetailRef::ReviewFile { task, path } => review::review_file(ctx, task, path, offset)?,
            _ => unreachable!(),
        };
        if let (DetailRef::ReviewFile { task, .. }, Some(candidate)) = (&request.target, candidate)
            && review::patch_revision(ctx, task)? != candidate
        {
            bail!("app review candidate changed while reading the patch");
        }
        return paging::reply(body, next, etag, selector, as_of, seq);
    }
    if let DetailRef::Session { id } = &request.target {
        if cursor.as_ref().is_some_and(|page| page.source.is_none()) {
            bail!("invalid session page token");
        }
        let etag = digest(&(seq, &request.target))?;
        paging::check(cursor.as_ref(), &etag)?;
        if cursor.is_none() && request.etag.as_deref() == Some(etag.as_str()) {
            return Ok(DetailReply {
                etag,
                as_of,
                seq,
                next: None,
                body: None,
            });
        }
        let source = cursor.as_ref().and_then(|page| page.source.as_deref());
        let (body, next) = agents::session(ctx, id, source)?;
        return paging::source_reply(body, next, etag, selector, as_of, seq);
    }
    if cursor.as_ref().is_some_and(|page| page.source.is_some()) {
        bail!("invalid detail page token");
    }
    let body = match &request.target {
        DetailRef::Question { key } => questions::question(ctx, key)?,
        DetailRef::Task { id } => tasks::task(ctx, id)?,
        DetailRef::Plan { id } => plans::plan(ctx, id)?,
        DetailRef::Review { task } => review::review(ctx, task)?,
        DetailRef::Session { .. } => unreachable!(),
        DetailRef::Terminal { id } => agents::terminal(ctx, id)?,
        DetailRef::Timeline { from, to, by } => timeline::timeline(ctx, from, to, *by)?,
        DetailRef::Burnup { scope } => timeline::burnup(ctx, scope)?,
        DetailRef::UsageSeries { period } => usage::usage_series(ctx, period)?,
        DetailRef::Filings { before } => agents::filings(ctx, *before)?,
        DetailRef::Artifact { .. } | DetailRef::Attachment { .. } => unreachable!(),
        DetailRef::Other => bail!("unsupported app detail selector"),
        DetailRef::PlanDiff { .. } | DetailRef::ReviewFile { .. } => unreachable!(),
    };
    let etag = digest(&(&request.target, &body))?;
    paging::check(cursor.as_ref(), &etag)?;
    if cursor.is_none() && request.etag.as_deref() == Some(etag.as_str()) {
        return Ok(DetailReply {
            etag,
            as_of,
            seq,
            next: None,
            body: None,
        });
    }
    let (body, next) = paging::body_page(body, offset)?;
    paging::reply(body, next, etag, selector, as_of, seq)
}

fn validate(request: &DetailGet) -> Result<()> {
    fn bounded(value: &str, maximum: usize) -> Result<()> {
        if value.is_empty() || value.len() > maximum || value.contains('\0') {
            bail!("invalid app detail selector");
        }
        Ok(())
    }
    match &request.target {
        DetailRef::Question { key } => bounded(key, 512)?,
        DetailRef::Task { id }
        | DetailRef::Plan { id }
        | DetailRef::Session { id }
        | DetailRef::Terminal { id } => bounded(id, 512)?,
        DetailRef::Review { task } => bounded(task, 512)?,
        DetailRef::ReviewFile { task, path } => {
            bounded(task, 512)?;
            bounded(path, 4096)?;
        }
        DetailRef::PlanDiff { id, from, to } => {
            bounded(id, 512)?;
            bounded(from, 128)?;
            bounded(to, 128)?;
        }
        DetailRef::Timeline { from, to, .. } => {
            bounded(from, 64)?;
            bounded(to, 64)?;
        }
        DetailRef::Burnup { scope } => bounded(scope, 512)?,
        DetailRef::UsageSeries { period } => bounded(period, 64)?,
        DetailRef::Artifact { task, key } => {
            bounded(task, 512)?;
            bounded(key, 4096)?;
        }
        DetailRef::Attachment { entry, id } => {
            bounded(entry, 256)?;
            bounded(id, 2)?;
            if !matches!(id.as_str(), "a1" | "a2" | "a3" | "a4") {
                bail!("invalid attachment id");
            }
        }
        DetailRef::Filings { before } => {
            if *before == Some(0) {
                bail!("invalid filings before anchor");
            }
        }
        DetailRef::Other => bail!("unsupported app detail selector"),
    }
    if request.etag.as_ref().is_some_and(|value| value.len() > 128)
        || request
            .page
            .as_ref()
            .is_some_and(|value| value.len() > 1024)
    {
        bail!("app detail cache token exceeds limit");
    }
    Ok(())
}

pub(super) fn now() -> String {
    jiff::Timestamp::now().to_string()
}

/// UTF-8-safe byte bound; callers expose canonical truncation flags where present.
pub(super) fn short(value: &str, maximum: usize) -> String {
    let mut end = value.len().min(maximum);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn digest(value: &impl Serialize) -> Result<String> {
    let mut hash = Sha256::new();
    serde_json::to_writer(HashWriter(&mut hash), value)?;
    Ok(format!("{:x}", hash.finalize()))
}

/// Content revision for rows that have no independent canonical revision counter.
pub(super) fn revision(value: &impl Serialize) -> Result<u64> {
    let mut hash = Sha256::new();
    serde_json::to_writer(HashWriter(&mut hash), value)?;
    let bytes = hash.finalize();
    Ok(u64::from_be_bytes(bytes[..8].try_into()?))
}

struct HashWriter<'a>(&'a mut Sha256);
impl std::io::Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
