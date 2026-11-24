use std::{borrow::Cow, io::Read as _, ops::Range, path::Path};

use anyhow::{Context, Result};
use corral_types::{Artifact, Reference, references_in, section_slug};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use rusqlite::params;
use serde::Serialize;

use crate::{ArtifactKind, Engine, Store, refuse, store::encode};

/// A citation from task text and the immutable catalog row it names, if resolvable.
#[derive(Debug, Serialize)]
pub struct ArtifactCitation {
    pub reference: String,
    pub artifact: Option<Artifact>,
}

/// File-sourced artifacts carry a format hint in their catalog document, not
/// in their blob or API title. Older and byte-sourced rows have no such hint.
#[derive(Clone, Copy)]
pub(crate) enum ArtifactSource {
    Markdown,
    Other,
    Unknown,
}

impl ArtifactSource {
    pub(crate) fn from_path(path: &Path) -> Self {
        match path.extension() {
            Some(ext) if ext.to_str().is_some_and(is_markdown_extension) => Self::Markdown,
            Some(_) => Self::Other,
            None => Self::Unknown,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Other => "other",
            Self::Unknown => "unknown",
        }
    }
}

pub(crate) fn encode_artifact_record(
    artifact: &Artifact,
    source: Option<ArtifactSource>,
) -> Result<String> {
    #[derive(Serialize)]
    struct Record<'a> {
        #[serde(flatten)]
        artifact: &'a Artifact,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_format: Option<&'static str>,
    }
    let artifact = if artifact.replaced_by.is_some() {
        Cow::Owned(Artifact {
            replaced_by: None,
            ..artifact.clone()
        })
    } else {
        Cow::Borrowed(artifact)
    };
    encode(&Record {
        artifact: &artifact,
        source_format: source.map(ArtifactSource::as_str),
    })
}

impl Store {
    fn artifact_is_markdown(&self, artifact: &Artifact) -> Result<Option<bool>> {
        let mut lookup = self.connection.prepare_cached(
            "SELECT json_extract(document, '$.source_format') FROM artifact_catalog WHERE owner=?1 AND ordinal=?2",
        ).context("prepare artifact source lookup")?;
        let format: Option<String> = lookup
            .query_row(params![artifact.task, artifact.id], |row| row.get(0))
            .context("look up artifact source format")?;
        Ok(match format.as_deref() {
            Some("markdown") => Some(true),
            Some("other") => Some(false),
            Some(_) => None,
            None => markdown_title(&artifact.title),
        })
    }
}

impl Engine {
    pub fn artifact_references(&self, text: &str) -> Vec<ArtifactCitation> {
        self.artifact_references_in(&self.store, text)
    }

    pub(crate) fn artifact_references_in(
        &self,
        store: &Store,
        text: &str,
    ) -> Vec<ArtifactCitation> {
        references_in(text)
            .into_iter()
            .filter(|reference| reference.artifact().is_some())
            .map(|reference| ArtifactCitation {
                reference: reference.as_str().to_owned(),
                artifact: self
                    .resolve_artifact_citation(store, reference)
                    .ok()
                    .flatten(),
            })
            .collect()
    }

    /// Advisory only: a missing task, artifact, or section must not stop a write.
    pub fn unresolved_references(&self, text: &str) -> Vec<String> {
        references_in(text)
            .into_iter()
            .filter(|&reference| {
                if reference.artifact().is_some() {
                    self.resolve_artifact_citation(&self.store, reference)
                        .ok()
                        .flatten()
                        .is_none()
                } else {
                    self.store
                        .get_task(reference.task())
                        .ok()
                        .flatten()
                        .is_none()
                }
            })
            .map(|reference| reference.as_str().to_owned())
            .collect()
    }

    fn resolve_artifact_citation(
        &self,
        store: &Store,
        reference: Reference<'_>,
    ) -> Result<Option<Artifact>> {
        let Some(address) = reference.artifact() else {
            return Ok(None);
        };
        let Some(artifact) = store.get_artifact(address)? else {
            return Ok(None);
        };
        if let Some(section) = reference.section() {
            self.artifact_section(store, &artifact, section)?;
        } else {
            self.open_artifact_blob(&artifact)?;
        }
        Ok(Some(artifact))
    }

    /// Like `require_artifact`, but accepts and checks an optional task artifact section.
    pub fn require_artifact_citation(&self, reference: &str) -> Result<Artifact> {
        let (address, section) = artifact_selector(reference);
        let artifact = self.require_artifact(address)?;
        if let Some(section) = section {
            self.artifact_section(&self.store, &artifact, section)?;
        }
        Ok(artifact)
    }

    /// Return the exact stored bytes, or the original source of one Markdown section.
    pub fn show_artifact(&self, reference: &str) -> Result<Vec<u8>> {
        let (address, section) = artifact_selector(reference);
        let artifact = self.require_artifact(address)?;
        if let Some(section) = section {
            return self.artifact_section(&self.store, &artifact, section);
        }
        self.artifact_bytes(&artifact)
    }

    fn artifact_bytes(&self, artifact: &Artifact) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.open_artifact_blob(artifact)?
            .read_to_end(&mut bytes)
            .context("read artifact blob")?;
        Ok(bytes)
    }

    fn artifact_section(
        &self,
        store: &Store,
        artifact: &Artifact,
        section: &str,
    ) -> Result<Vec<u8>> {
        let (mut bytes, range) = self.section_source(store, artifact, section)?;
        bytes.truncate(range.end);
        Ok(bytes.split_off(range.start))
    }

    /// Where a cited section lies in the artifact's stored bytes, by the rules `show_artifact`
    /// cuts it with. One read of the source, so a reader can then page the section from the
    /// blob without reading the source again.
    pub fn artifact_section_range(
        &self,
        artifact: &Artifact,
        section: &str,
    ) -> Result<Range<usize>> {
        Ok(self.section_source(&self.store, artifact, section)?.1)
    }

    /// The artifact's stored bytes and where `section` lies in them; refused when it is not
    /// Markdown or has no such section.
    fn section_source(
        &self,
        store: &Store,
        artifact: &Artifact,
        section: &str,
    ) -> Result<(Vec<u8>, Range<usize>)> {
        let reference = artifact.reference();
        let not_markdown = || {
            refuse(
                format!(
                    "artifact {reference} is not Markdown; sections are only available in Markdown"
                ),
                vec![],
            )
        };
        let explicit = store.artifact_is_markdown(artifact)?;
        if explicit == Some(false) {
            return Err(not_markdown());
        }
        let bytes = self.artifact_bytes(artifact)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| not_markdown())?;
        if text.contains('\0')
            || (explicit.is_none()
                && (text.starts_with("#!")
                    || matches!(
                        artifact.kind,
                        ArtifactKind::Diff | ArtifactKind::Gate | ArtifactKind::Log
                    )))
        {
            return Err(not_markdown());
        }
        let headings = markdown_headings(text);
        if headings.is_empty() && explicit.is_none() {
            return Err(not_markdown());
        }
        if let Some((i, hit)) = headings
            .iter()
            .enumerate()
            .find(|(_, heading)| heading.slug == section)
        {
            let range = section_range(text, &headings[i + 1..], hit);
            return Ok((bytes, range));
        }
        let mut slugs = headings
            .iter()
            .map(|heading| heading.slug.as_str())
            .filter(|slug| !slug.is_empty())
            .peekable();
        let available = if slugs.peek().is_none() {
            "(none)".into()
        } else {
            slugs.collect::<Vec<_>>().join(", ")
        };
        Err(refuse(
            format!(
                "section {section:?} not found in artifact {reference}; available sections: {available}"
            ),
            vec![],
        ))
    }
}

fn artifact_selector(reference: &str) -> (&str, Option<&str>) {
    if let Some(parsed) = Reference::parse(reference)
        && let (Some(address), Some(section)) = (parsed.artifact(), parsed.section())
    {
        return (address, Some(section));
    }
    (reference, None)
}

fn markdown_title(title: &str) -> Option<bool> {
    let extension = Path::new(title).extension()?.to_str()?;
    // Avoid mistaking a version number or prose after a period for a suffix.
    if !extension
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphabetic)
        || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(is_markdown_extension(extension))
}

fn is_markdown_extension(extension: &str) -> bool {
    extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
}

struct Heading {
    slug: String,
    level: u8,
    start: usize,
}

fn markdown_headings(text: &str) -> Vec<Heading> {
    let mut events = Parser::new(text).into_offset_iter();
    let mut headings = Vec::new();
    while let Some((event, range)) = events.next() {
        let Event::Start(Tag::Heading { level, .. }) = event else {
            continue;
        };
        let mut words = String::new();
        for (event, _) in events.by_ref() {
            match event {
                Event::End(TagEnd::Heading(_)) => break,
                Event::Text(text) | Event::Code(text) => words.push_str(&text),
                Event::SoftBreak | Event::HardBreak => words.push(' '),
                _ => {}
            }
        }
        headings.push(Heading {
            slug: section_slug(&words),
            level: level as u8,
            // Use the physical source line, including any quote or list marker.
            start: text[..range.start].rfind(['\n', '\r']).map_or(0, |i| i + 1),
        });
    }
    headings
}

fn section_range(text: &str, rest: &[Heading], hit: &Heading) -> Range<usize> {
    let end = rest
        .iter()
        .find(|heading| heading.level <= hit.level)
        .map_or(text.len(), |heading| heading.start);
    hit.start..end
}
