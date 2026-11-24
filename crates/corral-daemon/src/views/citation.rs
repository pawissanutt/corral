//! Task-text citations of artifacts (`T-12:3`, `T-12:3#goal`), resolved by the engine's rules.
//! Resolving one reads no content for the client: the catalog row, the blob's presence and,
//! for a section, where it lies. A cited section's content pages from a window of the blob
//! whose range the first page found and the page token carries, so a continuation never reads
//! the source again. Whatever the engine would not resolve is `not_found`.

use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    ops::Range,
};

use anyhow::{Context, Result, ensure};
use corral_engine::Engine;
use corral_protocol::detail::{CitationDetail, DetailGet, DetailReply};
use corral_types::{Artifact, Reference, Refusal};

use super::NotFound;
use super::details::{ArtifactSource, artifact_page, media_type};
use super::fingerprint;

/// A refusal of the engine's (an unreadable reference, no such section, not Markdown) as
/// `not_found`; any other failure as it is.
fn missing(error: anyhow::Error, what: &'static str) -> anyhow::Error {
    if error.is::<Refusal>() {
        NotFound(what).into()
    } else {
        error
    }
}

/// The cited artifact and its open blob.
fn cited(engine: &Engine, address: &str) -> Result<(Artifact, File)> {
    let artifact = engine
        .store
        .get_artifact(address)
        .map_err(|error| missing(error, "artifact not found"))?
        .ok_or(NotFound("artifact not found"))?;
    let file = engine.open_artifact_blob(&artifact).map_err(|error| {
        let gone = error
            .root_cause()
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::NotFound);
        if gone {
            NotFound("artifact content not found").into()
        } else {
            error
        }
    })?;
    Ok((artifact, file))
}

/// Where `section` lies in the artifact's stored bytes.
fn section_range(engine: &Engine, artifact: &Artifact, section: &str) -> Result<Range<u64>> {
    let range = engine
        .artifact_section_range(artifact, section)
        .map_err(|error| missing(error, "artifact section not found"))?;
    Ok(range.start as u64..range.end as u64)
}

/// Whether an artifact citation resolves, without its content.
pub(super) fn citation(engine: &Engine, reference: &str) -> Result<CitationDetail> {
    let cited_ref = Reference::parse(reference);
    let address = cited_ref
        .and_then(Reference::artifact)
        .context("invalid: not an artifact citation")?;
    let (artifact, _) = cited(engine, address)?;
    let section_bytes = cited_ref
        .and_then(Reference::section)
        .map(|section| {
            section_range(engine, &artifact, section).map(|range| range.end - range.start)
        })
        .transpose()?;
    Ok(CitationDetail {
        artifact,
        section_bytes,
    })
}

/// An artifact's content, or for a citation naming a section that section's source.
pub(super) fn artifact(
    engine: &Engine,
    query: &DetailGet,
    seq: u64,
    reference: &str,
) -> Result<DetailReply> {
    let cited_ref = Reference::parse(reference);
    let address = cited_ref.and_then(Reference::artifact).unwrap_or(reference);
    let (artifact, file) = cited(engine, address)?;
    let (task, key) = (artifact.task.clone(), artifact.id.to_string());
    let Some(section) = cited_ref.and_then(Reference::section) else {
        let content = ArtifactSource {
            media_type: media_type(&artifact.title),
            fingerprint: fingerprint(&(&query.target, &artifact))?,
            bytes: artifact.bytes,
            task,
            key,
        };
        return artifact_page(query, seq, file, content);
    };
    // The first page finds the range; a continuation carries it as `<start>-<end>~<token>`.
    let (range, page) = match query.page.as_deref() {
        None => (section_range(engine, &artifact, section)?, None),
        Some(token) => {
            let (range, page) = token
                .split_once('~')
                .context("invalid: section page token")?;
            let (start, end) = range
                .split_once('-')
                .context("invalid: section page token")?;
            let range = start
                .parse::<u64>()
                .context("invalid: section page token")?
                ..end.parse::<u64>().context("invalid: section page token")?;
            ensure!(
                range.start <= range.end && range.end <= artifact.bytes,
                "invalid: section page token"
            );
            (range, Some(page.to_owned()))
        }
    };
    let inner = DetailGet {
        target: query.target.clone(),
        etag: query.etag.clone(),
        page,
    };
    let content = ArtifactSource {
        media_type: Some("text/markdown"),
        fingerprint: fingerprint(&(&query.target, &artifact, range.start, range.end))?,
        bytes: range.end - range.start,
        task,
        key,
    };
    let window = Window::new(file, range.clone())?;
    let mut reply = artifact_page(&inner, seq, window, content)?;
    reply.next = reply
        .next
        .map(|next| format!("{}-{}~{next}", range.start, range.end));
    Ok(reply)
}

/// `len` bytes of a blob from `start`, read and sought as a file of their own.
struct Window {
    file: File,
    start: u64,
    len: u64,
    at: u64,
}

impl Window {
    fn new(mut file: File, range: Range<u64>) -> Result<Self> {
        file.seek(SeekFrom::Start(range.start))
            .context("seek to the cited section")?;
        Ok(Self {
            file,
            start: range.start,
            len: range.end - range.start,
            at: 0,
        })
    }
}

impl Read for Window {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = usize::try_from(self.len - self.at).unwrap_or(usize::MAX);
        let take = buf.len().min(left);
        if take == 0 {
            return Ok(0);
        }
        let read = self.file.read(&mut buf[..take])?;
        self.at += read as u64;
        Ok(read)
    }
}

impl Seek for Window {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let SeekFrom::Start(at) = to else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a section window seeks from its start",
            ));
        };
        self.at = at.min(self.len);
        self.file.seek(SeekFrom::Start(self.start + self.at))?;
        Ok(self.at)
    }
}
