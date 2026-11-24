//! Task-text citations of artifacts (`T-12:3`, `T-12:3#goal`), resolved by the engine's rules.
//! Resolving one sends no content to the client: the catalog row, the blob's presence and, for
//! a section, where it lies. A section's range is found by the engine once and then held, as a
//! catalog row and its blob never change, so answering again or paging the section (from its
//! window of the blob) does not read the source again. Whatever the engine would not resolve
//! is `not_found`.

use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    ops::Range,
    sync::{LazyLock, Mutex, PoisonError},
};

use anyhow::{Context, Result};
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

/// What the engine decided about a section, by artifact reference, blob digest and slug:
/// where it lies, or that it is not there (no such heading, not Markdown).
type Found = HashMap<(String, String, String), Option<Range<u64>>>;

/// How many decided sections are held before they are let go together.
const HELD: usize = 512;

const NO_SECTION: &str = "artifact section not found";

static FOUND: LazyLock<Mutex<Found>> = LazyLock::new(Mutex::default);

/// Where `section` lies in the artifact's stored bytes, as the engine finds it: decided once
/// per artifact and section while held. Only the engine's own answer is ever used; a read
/// that failed (not a refusal) is not held.
fn section_range(engine: &Engine, artifact: &Artifact, section: &str) -> Result<Range<u64>> {
    let key = (
        artifact.reference(),
        artifact.sha256.clone(),
        section.to_owned(),
    );
    let held = || FOUND.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(decided) = held().get(&key) {
        return decided.clone().ok_or_else(|| NotFound(NO_SECTION).into());
    }
    let decided = match engine.artifact_section_range(artifact, section) {
        Ok(range) => Some(range.start as u64..range.end as u64),
        Err(error) if error.is::<Refusal>() => None,
        Err(error) => return Err(error),
    };
    let mut found = held();
    if found.len() >= HELD {
        found.clear();
    }
    found.insert(key, decided.clone());
    decided.ok_or_else(|| NotFound(NO_SECTION).into())
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
            text: None,
            fingerprint: fingerprint(&(&query.target, &artifact))?,
            bytes: artifact.bytes,
            task,
            key,
        };
        return artifact_page(query, seq, file, content);
    };
    let range = section_range(engine, &artifact, section)?;
    let content = ArtifactSource {
        media_type: Some("text/markdown"),
        // The engine read the source as UTF-8 without NULs to find the section.
        text: Some(true),
        fingerprint: fingerprint(&(&query.target, &artifact, range.start, range.end))?,
        bytes: range.end - range.start,
        task,
        key,
    };
    artifact_page(query, seq, Window::new(file, range)?, content)
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
