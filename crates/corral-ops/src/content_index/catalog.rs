//! Disk-backed complete-source candidates. The old bounded prefix cache still defines IDF
//! and task-search text; it is not a completeness index for overflow documents.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::query::Query;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Cache, Document, INDEX_BYTES, Record, Source, TEXT_BUDGET, VERSION, digest, postings};
use crate::Ctx;

#[path = "catalog_error.rs"]
mod error;
#[path = "catalog_fallback.rs"]
mod fallback;
#[path = "catalog_order.rs"]
mod order;
#[path = "catalog_refresh.rs"]
mod refresh;
use error::Corrupt;
#[cfg(test)]
pub(crate) use refresh::advance_batch;
#[cfg(test)]
pub(crate) use refresh::interrupt_after_batches;
#[path = "catalog_metadata.rs"]
mod metadata;
// Emergency safety cutover: never wait for or depend on catalog construction in production.
// Unit regressions can explicitly advance and exercise complete generations.
pub(super) const BACKGROUND_CATALOG_BUILD_ENABLED: bool = false;
pub(crate) use error::{is_corrupt, repair};

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct Manifest {
    version: u32,
    after: u64,
    events_after: u64,
    tasks: BTreeMap<String, String>,
    events: Vec<String>,
    assistants: Option<String>,
    overflow_binary: BTreeSet<String>,
    prefix_file: String,
    extras: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Entry {
    pub document: Document,
    source: IndexedSource,
    lexical: String,
    terms: Option<Vec<String>>,
    fallback: Option<Vec<u64>>,
}

#[derive(Clone, Serialize, Deserialize)]
enum IndexedSource {
    Artifact(corral_core::Artifact),
    Text(String),
    Event(u64),
    Slice { file: String, offset: u64, len: u64 },
}

pub(crate) struct Catalog {
    root: PathBuf,
    objects: PathBuf,
    manifest: Manifest,
    cache: Cache,
    temporary: Option<tempfile::TempDir>,
}

impl Catalog {
    /// Never waits for a build. Only complete, current immutable generations serve hits.
    pub fn open(ctx: &Ctx, reindex: bool) -> Result<Option<Self>> {
        let _span = corral_core::timing::span("search.index_open_refresh");
        if !BACKGROUND_CATALOG_BUILD_ENABLED && !cfg!(test) {
            return Ok(None);
        }
        if reindex {
            refresh::request_reindex(ctx)?;
            refresh::schedule(ctx, true)?;
            return Ok(None);
        }
        let ready = refresh::published(ctx)?;
        if ready.is_none() {
            refresh::schedule(ctx, true)?;
        }
        Ok(ready)
    }

    pub fn fallback(ctx: &Ctx, reindex: bool) -> Result<Self> {
        fallback::build(ctx, reindex)
    }

    /// The daemon and CLI schedule a worker, never a full synchronous source scan.
    pub fn maintain(ctx: &Ctx) -> Result<()> {
        if !BACKGROUND_CATALOG_BUILD_ENABLED {
            return Ok(());
        }
        refresh::schedule(ctx, false)
    }

    fn visit_entries(&self, mut visit: impl FnMut(Entry) -> Result<()>) -> Result<()> {
        for path in self.manifest.tasks.values() {
            entries(
                File::open(self.root.join(path)).context("opening task search catalog")?,
                &mut visit,
            )?;
        }
        if let Some(path) = &self.manifest.assistants {
            entries(
                File::open(self.root.join(path)).context("opening assistant search catalog")?,
                &mut visit,
            )?;
        }
        for path in &self.manifest.events {
            entries(
                File::open(self.root.join(path)).context("opening event search catalog")?,
                &mut visit,
            )?;
        }
        Ok(())
    }

    pub fn idfs(&self, query: &Query) -> Vec<u64> {
        query
            .content_terms()
            .iter()
            .map(|(term, _, _)| {
                let tokens: Vec<_> = super::words(term).collect();
                let matching = self
                    .cache
                    .postings
                    .iter()
                    .filter(|(word, _)| tokens.iter().any(|token| word.contains(token)))
                    .flat_map(|(_, ids)| ids.iter())
                    .collect::<BTreeSet<_>>()
                    .len();
                1 + 1000 / (1 + u64::try_from(matching).unwrap_or(u64::MAX - 1))
            })
            .collect()
    }

    /// Candidate selection reads only catalog terms, not task histories or source bodies.
    pub fn visit(&self, query: &Query, mut visit: impl FnMut(&Entry) -> Result<()>) -> Result<()> {
        let _span = corral_core::timing::span("search.candidate_selection");
        let positive: Vec<_> = query
            .content_terms()
            .into_iter()
            .filter(|(_, _, negative)| !negative)
            .flat_map(|(term, _, _)| super::words(term))
            .collect();
        self.visit_entries(|entry| {
            if entry.terms.is_none() && entry.fallback.is_none() {
                return visit(&entry);
            }
            let matches = entry.terms.as_ref().map_or_else(
                || {
                    positive.iter().all(|word| {
                        super::words(&entry.document.title).any(|token| token.contains(word))
                            || entry
                                .fallback
                                .as_ref()
                                .is_none_or(|bits| super::grams::may_contain(bits, word))
                    })
                },
                |terms| {
                    positive.iter().all(|word| {
                        super::words(&entry.document.title).any(|token| token.contains(word))
                            || terms.iter().any(|token| token.contains(word))
                    })
                },
            );
            if matches {
                visit(&entry)?;
            }
            Ok(())
        })
    }

    pub fn record(&self, ctx: &Ctx, entry: &Entry) -> Result<Record> {
        let mut document = entry.document.clone();
        if let Some(cached) = self.cache.documents.get(&document.reference) {
            document = cached.clone();
        } else {
            document.complete = false;
            document.binary = self.manifest.overflow_binary.contains(&document.reference);
        }
        let source = match &entry.source {
            IndexedSource::Artifact(artifact) => Source::Artifact(artifact.clone()),
            IndexedSource::Text(key) => {
                let path = self.objects.join(format!("{key}.body"));
                if !document.complete && !document.binary && !path.is_file() {
                    return Err(Corrupt.into());
                }
                Source::File(path)
            }
            IndexedSource::Event(_) if document.complete || document.binary => {
                Source::Text(String::new())
            }
            IndexedSource::Event(seq) => Source::Text(event_text(ctx, *seq)?),
            IndexedSource::Slice { file, offset, len } => Source::FileRange {
                path: self.root.join(file),
                offset: *offset,
                len: *len,
            },
        };
        Ok(Record { document, source })
    }

    pub fn artifact_records(&self, ctx: &Ctx) -> Result<Vec<Record>> {
        let mut records = Vec::new();
        self.visit_entries(|entry| {
            if entry.document.scope == "artifacts" {
                records.push(self.record(ctx, &entry)?);
            }
            Ok(())
        })?;
        Ok(records)
    }

    pub fn artifact_may_match(
        &self,
        artifact: &corral_core::Artifact,
        terms: &[&str],
    ) -> Result<bool> {
        if self.temporary.is_some() {
            return Ok(true);
        }
        let key = format!("a-{}", digest(&artifact.sha256));
        let Some(indexed) = lexical(&self.objects, &key)? else {
            return Ok(true);
        };
        Ok(terms
            .iter()
            .flat_map(|term| super::words(term))
            .all(|word| {
                indexed.terms.as_ref().map_or_else(
                    || {
                        indexed
                            .fallback
                            .as_ref()
                            .is_none_or(|bits| super::grams::may_contain(bits, &word))
                    },
                    |terms| terms.iter().any(|token| token.contains(&word)),
                )
            }))
    }

    pub fn task_ids(&self) -> impl Iterator<Item = &String> {
        self.manifest.tasks.keys()
    }

    pub fn task_text(&self, id: &str) -> Result<String> {
        let path = self.manifest.extras.get(id).ok_or(Corrupt)?;
        match fs::read_to_string(self.root.join(path)) {
            Ok(text) => Ok(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(Corrupt.into()),
            Err(error) => Err(error).context("reading task search supplemental text"),
        }
    }

    fn rebuild_prefixes(&mut self, ctx: &Ctx, reindex: bool, publish: bool) -> Result<()> {
        let _span = corral_core::timing::span("search.prefix_refresh");
        let mut previous = std::mem::take(&mut self.cache);
        if reindex {
            previous = Cache::default();
        }
        previous.postings.clear();
        let mut descriptors = order::Order::new(&self.root);
        self.visit_entries(|entry| descriptors.push(entry.document, entry.lexical, entry.source))?;
        let mut descriptors = descriptors.sorted()?;
        let mut budget = TEXT_BUDGET;
        let mut documents = BTreeMap::new();
        let mut extra = BTreeMap::<String, String>::new();
        self.manifest.overflow_binary.clear();
        while let Some(order::Descriptor(mut doc, _key, source)) = descriptors.next()? {
            doc.complete = false;
            doc.binary = false;
            if let Some(cached) = previous.documents.get(&doc.reference).filter(|cached| {
                previous.version == VERSION && cached.fingerprint == doc.fingerprint
            }) {
                doc.text.clone_from(&cached.text);
                doc.complete = cached.complete;
                doc.binary = cached.binary;
                doc.file_stamp.clone_from(&cached.file_stamp);
            } else if budget > 0 && documents.len() < 4096 {
                let text = prefix_source(ctx, &self.root, &doc.task, &source)?;
                doc.text = text.text;
                doc.complete = text.complete;
                doc.binary = text.binary;
            }
            if doc.text.len() <= budget && documents.len() < 4096 {
                budget -= doc.text.len();
                documents.insert(doc.reference.clone(), doc.clone());
            } else {
                doc.text.clear();
                doc.complete = false;
                if doc.binary {
                    self.manifest.overflow_binary.insert(doc.reference.clone());
                }
            }
            if !matches!(doc.scope.as_str(), "specs" | "events") {
                let text = extra.entry(doc.task.clone()).or_default();
                text.push('\n');
                text.push_str(&doc.text);
            }
        }
        for id in self.manifest.tasks.keys() {
            let text = extra.remove(id).unwrap_or_default();
            let path = immutable_file(&self.root, text.as_bytes())?;
            self.manifest.extras.insert(id.clone(), path);
        }
        let mut cache = Cache {
            version: VERSION,
            postings: postings(&documents),
            documents,
            events_after: Some(self.manifest.events_after),
        };
        let bytes = loop {
            let bytes = serde_json::to_vec(&cache).context("encoding search catalog prefixes")?;
            if u64::try_from(bytes.len()).context("search prefix size overflow")? <= INDEX_BYTES {
                break bytes;
            }
            let Some((reference, doc)) = cache.documents.pop_first() else {
                anyhow::bail!("search prefix metadata exceeds size limit");
            };
            if doc.binary {
                self.manifest.overflow_binary.insert(reference);
            }
            cache.postings = postings(&cache.documents);
        };
        self.manifest.prefix_file = immutable_file(&self.root, &bytes)?;
        if publish {
            publish_prefixes(ctx, &bytes, self.manifest.after)?;
        }
        self.cache = cache;
        Ok(())
    }
}

fn entries(reader: impl Read, mut visit: impl FnMut(Entry) -> Result<()>) -> Result<()> {
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    loop {
        line.clear();
        if reader
            .read_line(&mut line)
            .context("reading search catalog")?
            == 0
        {
            break;
        }
        let entry = serde_json::from_str(&line)
            .map_err(|error| anyhow::Error::new(Corrupt).context(error))?;
        visit(entry)?;
    }
    Ok(())
}

fn lexical(root: &Path, key: &str) -> Result<Option<super::lexical::Lexical>> {
    match File::open(root.join(format!("{key}.lex"))) {
        Ok(file) => Ok(serde_json::from_reader(BufReader::new(file)).ok()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context("opening lexical search cache"),
    }
}

fn write_record(
    ctx: &Ctx,
    root: &Path,
    record: Record,
    writer: &mut impl Write,
    reindex: bool,
    cached: Option<&Document>,
) -> Result<()> {
    let (source, key) = match &record.source {
        Source::Artifact(artifact) => (
            IndexedSource::Artifact(artifact.clone()),
            format!("a-{}", digest(&artifact.sha256)),
        ),
        Source::Text(_) if record.document.reference.starts_with("events:") => {
            let seq: u64 = record
                .document
                .reference
                .trim_start_matches("events:")
                .parse()
                .context("invalid event search reference")?;
            (IndexedSource::Event(seq), format!("e-{seq}"))
        }
        Source::Text(text) => {
            let key = format!("t-{}", digest(text));
            let path = root.join(format!("{key}.body"));
            if reindex || !path.is_file() {
                crate::artifact_storage::replace(&path, text.as_bytes())?;
            }
            (IndexedSource::Text(key.clone()), key)
        }
        Source::File(_) => {
            let key = format!("t-{}", record.document.fingerprint);
            (IndexedSource::Text(key.clone()), key)
        }
        Source::FileRange { .. } => anyhow::bail!("packed fallback source cannot be indexed"),
    };
    let event = matches!(&source, IndexedSource::Event(_));
    let indexed = if !reindex && !event {
        lexical(root, &key)?
    } else {
        None
    };
    let indexed = match indexed {
        Some(indexed) => indexed,
        None => {
            #[cfg(test)]
            super::tests::CATALOG_EXTRACTIONS.with(|count| count.set(count.get() + 1));
            let reusable = cached.filter(|doc| {
                !reindex && doc.complete && doc.fingerprint == record.document.fingerprint
            });
            let reader: Box<dyn Read + '_> = if let Some(cached) = reusable {
                Box::new(cached.text.as_bytes())
            } else {
                match &record.source {
                    Source::Artifact(artifact) => {
                        let Some(reader) =
                            super::search_artifact_reader(ctx, &record.document.task, artifact)?
                        else {
                            return Ok(());
                        };
                        reader
                    }
                    Source::Text(text) => Box::new(text.as_bytes()),
                    Source::File(path) => {
                        Box::new(File::open(path).context("opening catalog source text")?)
                    }
                    Source::FileRange { .. } => {
                        anyhow::bail!("packed fallback source cannot be indexed")
                    }
                }
            };
            let mut indexed = super::lexical::extract(reader)?;
            if let Some(cached) = reusable {
                indexed.prefix.clone_from(&cached.text);
                indexed.complete = cached.complete;
                indexed.binary = cached.binary;
                if cached.binary {
                    indexed.terms = None;
                    indexed.fallback = None;
                }
            }
            if !event {
                let bytes =
                    serde_json::to_vec(&indexed).context("encoding complete search terms")?;
                replace_cache(&root.join(format!("{key}.lex")), &bytes)?;
            }
            indexed
        }
    };
    let mut document = record.document;
    document.complete = indexed.complete;
    document.binary = indexed.binary;
    let entry = Entry {
        document,
        source,
        lexical: key,
        terms: indexed.terms,
        fallback: indexed.fallback,
    };
    serde_json::to_writer(&mut *writer, &entry).context("writing search catalog entry")?;
    writer
        .write_all(b"\n")
        .context("terminating search catalog entry")
}

fn event_text(ctx: &Ctx, seq: u64) -> Result<String> {
    use corral_core::backend::{EventQuery, EventSeq, PageReq};
    let view = ctx.store.view()?;
    let query = EventQuery {
        after: Some(EventSeq(seq.saturating_sub(1))),
        ..EventQuery::default()
    };
    let event = view
        .events(&query, PageReq::first(1))
        .context("reading candidate event")?
        .items
        .into_iter()
        .next()
        .filter(|row| row.seq.0 == seq && !row.malformed)
        .context("indexed event no longer exists")?;
    Ok(event.event.detail.to_string())
}

fn prefix_source(
    ctx: &Ctx,
    root: &Path,
    owner: &str,
    source: &IndexedSource,
) -> Result<super::ExtractedText> {
    use std::io::{Seek, SeekFrom};
    let reader: Box<dyn Read> = match source {
        IndexedSource::Artifact(artifact) => {
            super::search_artifact_reader(ctx, owner, artifact)?
                .context("artifact was purged while extracting a prefix")?
        }
        IndexedSource::Text(key) => Box::new(
            File::open(root.join("objects").join(format!("{key}.body")))
                .context("opening catalog prefix source")?,
        ),
        IndexedSource::Event(seq) => {
            return Ok(super::extract_bytes(event_text(ctx, *seq)?.as_bytes()));
        }
        IndexedSource::Slice { file, offset, len } => {
            let mut reader =
                File::open(root.join(file)).context("opening packed catalog prefix")?;
            reader
                .seek(SeekFrom::Start(*offset))
                .context("seeking packed catalog prefix")?;
            Box::new(reader.take(*len))
        }
    };
    let mut bytes = Vec::new();
    reader
        .take((super::DOCUMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .context("extracting catalog prefix")?;
    Ok(super::extract_bytes(&bytes))
}

fn replace_cache(path: &Path, bytes: &[u8]) -> Result<()> {
    // This is disposable derived text, not canonical artifact publication. One cursor
    // publication follows the completed build; do not fsync twice per source document.
    let mut file = tempfile::NamedTempFile::new_in(path.parent().context("cache has no parent")?)
        .context("creating derived search text")?;
    file.write_all(bytes)
        .context("writing derived search text")?;
    file.persist(path)
        .context("publishing derived search text")?;
    Ok(())
}

fn load_prefix_cache(ctx: &Ctx) -> Result<Cache> {
    match File::open(ctx.store.cache_root()?.join("content-search.json")) {
        Ok(file)
            if file
                .metadata()
                .context("reading prefix cache metadata")?
                .len()
                <= INDEX_BYTES =>
        {
            let cache = super::read_cache(file);
            Ok(if cache.version == VERSION {
                cache
            } else {
                Cache::default()
            })
        }
        Ok(_) => Ok(Cache::default()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Cache::default()),
        Err(error) => Err(error).context("opening search prefixes"),
    }
}

fn immutable_file(root: &Path, bytes: &[u8]) -> Result<String> {
    let name = format!("part-{:x}", Sha256::digest(bytes));
    let path = root.join(&name);
    if fs::read(&path).ok().as_deref() != Some(bytes) {
        replace_cache(&path, bytes)?;
    }
    Ok(name)
}

fn publish_prefixes(ctx: &Ctx, bytes: &[u8], after: u64) -> Result<()> {
    let lock = super::index_lock(ctx)?;
    match lock.try_lock() {
        Ok(()) => {
            let root = ctx.store.cache_root()?;
            crate::artifact_storage::replace(&root.join("content-search.json"), bytes)?;
            crate::content_refresh::write_cursor(&root, corral_core::backend::ChangeSeq(after))
        }
        Err(fs::TryLockError::WouldBlock) => Ok(()),
        Err(fs::TryLockError::Error(error)) => Err(error).context("publishing search prefixes"),
    }
}

fn metadata_entry(root: &Path, record: Record) -> Result<Entry> {
    let (source, key) = match record.source {
        Source::Artifact(artifact) => {
            let key = format!("a-{}", digest(&artifact.sha256));
            (IndexedSource::Artifact(artifact), key)
        }
        Source::Text(_) if record.document.reference.starts_with("events:") => {
            let seq = record
                .document
                .reference
                .trim_start_matches("events:")
                .parse()
                .context("invalid metadata event reference")?;
            (IndexedSource::Event(seq), format!("e-{seq}"))
        }
        Source::Text(text) => {
            let key = format!("t-{}", digest(&text));
            let path = root.join("objects").join(format!("{key}.body"));
            if fs::read(&path).ok().as_deref() != Some(text.as_bytes()) {
                replace_cache(&path, text.as_bytes())?;
            }
            (IndexedSource::Text(key.clone()), key)
        }
        Source::File(_) | Source::FileRange { .. } => {
            anyhow::bail!("metadata requires canonical text")
        }
    };
    Ok(Entry {
        document: record.document,
        source,
        lexical: key,
        terms: None,
        fallback: None,
    })
}

fn write_metadata(root: &Path, record: Record, writer: &mut impl Write) -> Result<()> {
    let entry = metadata_entry(root, record)?;
    serde_json::to_writer(&mut *writer, &entry).context("writing source metadata")?;
    writer
        .write_all(b"\n")
        .context("terminating source metadata")
}
