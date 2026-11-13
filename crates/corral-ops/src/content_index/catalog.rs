//! Disk-backed complete-source candidates. The old bounded prefix cache still defines IDF
//! and task-search text; it is not a completeness index for overflow documents.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_core::query::Query;
use serde::{Deserialize, Serialize};

use super::{Cache, Document, INDEX_BYTES, Record, Source, TEXT_BUDGET, VERSION, digest, postings};
use crate::Ctx;

#[path = "catalog_error.rs"]
mod error;
#[path = "catalog_order.rs"]
mod order;
#[path = "catalog_refresh.rs"]
mod refresh;
use error::Corrupt;
pub(crate) use error::{is_corrupt, repair};

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Manifest {
    version: u32,
    after: u64,
    events_after: u64,
    event_bytes: u64,
    events_file: String,
    tasks: BTreeSet<String>,
    assistants: bool,
    #[serde(default)]
    overflow_binary: BTreeSet<String>,
    #[serde(default)]
    prefix_stamp: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Entry {
    pub document: Document,
    source: IndexedSource,
    lexical: String,
    terms: Option<Vec<String>>,
    fallback: Option<Vec<u64>>,
}

#[derive(Serialize, Deserialize)]
enum IndexedSource {
    Artifact(corral_core::Artifact),
    Text(String),
    Event(u64),
}

pub(crate) struct Catalog {
    root: PathBuf,
    manifest: Manifest,
    cache: Cache,
}

impl Catalog {
    /// Caller holds index then storage. Every refresh is based on one frozen store view.
    pub fn open(ctx: &Ctx, reindex: bool) -> Result<Self> {
        match Self::open_once(ctx, reindex) {
            Err(error) if is_corrupt(&error) => {
                error::invalidate_locked(ctx)?;
                Self::open_once(ctx, reindex)
            }
            result => result,
        }
    }

    fn open_once(ctx: &Ctx, reindex: bool) -> Result<Self> {
        let _span = corral_core::timing::span("search.index_open_refresh");
        let root = ctx.store.cache_root()?.join("search-catalog");
        fs::create_dir_all(&root).context("creating search catalog")?;
        let path = root.join("manifest.json");
        let manifest: Manifest = match File::open(&path) {
            Ok(file) => serde_json::from_reader(BufReader::new(file)).unwrap_or_default(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Manifest::default(),
            Err(e) => return Err(e).context("opening search catalog manifest"),
        };
        let (view, seq) = super::one_view(ctx)?;
        let mut previous = match File::open(ctx.store.cache_root()?.join("content-search.json")) {
            Ok(file)
                if file
                    .metadata()
                    .context("reading search index metadata")?
                    .len()
                    <= INDEX_BYTES =>
            {
                super::read_cache(file)
            }
            Ok(_) => Cache::default(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Cache::default(),
            Err(e) => return Err(e).context("opening previous search index"),
        };
        if previous.version != VERSION {
            previous = Cache::default();
        }
        let mut catalog = Self {
            root,
            manifest,
            cache: previous,
        };
        let old_events = catalog.manifest.events_file.clone();
        let changed = refresh::refresh(&view, seq, &mut catalog, reindex)?;
        let prefix_path = ctx.store.cache_root()?.join("content-search.json");
        let stamp = crate::content_refresh::file_stamp(&prefix_path)?;
        if changed || catalog.cache.version != VERSION || stamp != catalog.manifest.prefix_stamp {
            catalog.rebuild_prefixes(&view, reindex)?;
            // Publishing the cursor last makes a crashed refresh replayable.
            let bytes = serde_json::to_vec(&catalog.manifest)
                .context("encoding search catalog manifest")?;
            crate::artifact_storage::replace(&path, &bytes)?;
            if !old_events.is_empty() && old_events != catalog.manifest.events_file {
                let old = catalog.root.join(&old_events);
                if old.is_file() {
                    fs::remove_file(old).context("removing replaced event search catalog")?;
                }
            }
        }
        Ok(catalog)
    }

    fn visit_entries(&self, mut visit: impl FnMut(Entry) -> Result<()>) -> Result<()> {
        for task in &self.manifest.tasks {
            entries(
                File::open(self.root.join(format!("{task}.jsonl")))
                    .context("opening task search catalog")?,
                &mut visit,
            )?;
        }
        if self.manifest.assistants {
            entries(
                File::open(self.root.join("assistants.jsonl"))
                    .context("opening assistant search catalog")?,
                &mut visit,
            )?;
        }
        let file = File::open(self.root.join(&self.manifest.events_file))
            .context("opening event search catalog")?;
        entries(file.take(self.manifest.event_bytes), visit)
    }

    pub fn idfs(&self, query: &Query) -> Vec<u64> {
        query
            .content_terms()
            .iter()
            .map(|(term, _, _)| {
                let matching = self
                    .cache
                    .postings
                    .iter()
                    .filter(|(word, _)| super::words(term).any(|t| word.contains(&t)))
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
                let path = self.root.join(format!("{key}.body"));
                if !document.complete && !document.binary && !path.is_file() {
                    return Err(Corrupt.into());
                }
                Source::File(path)
            }
            IndexedSource::Event(_) if document.complete || document.binary => {
                Source::Text(String::new())
            }
            IndexedSource::Event(seq) => Source::Text(event_text(ctx, *seq)?),
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
        let key = format!("a-{}", digest(&artifact.sha256));
        let Some(indexed) = lexical(&self.root, &key)? else {
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
        self.manifest.tasks.iter()
    }

    pub fn task_text(&self, id: &str) -> Result<String> {
        match fs::read_to_string(self.root.join(format!("{id}.extra"))) {
            Ok(text) => Ok(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(Corrupt.into()),
            Err(error) => Err(error).context("reading task search supplemental text"),
        }
    }

    fn rebuild_prefixes(&mut self, ctx: &Ctx, reindex: bool) -> Result<()> {
        let _span = corral_core::timing::span("search.prefix_refresh");
        let parent = self.root.parent().context("search catalog has no parent")?;
        let mut previous = std::mem::take(&mut self.cache);
        if reindex {
            previous = Cache::default();
        }
        previous.postings.clear();
        let mut descriptors = order::Order::new(&self.root);
        self.visit_entries(|entry| descriptors.push(entry.document, entry.lexical))?;
        let mut descriptors = descriptors.sorted()?;
        let mut budget = TEXT_BUDGET;
        let mut documents = BTreeMap::new();
        let mut extra = BTreeMap::<String, String>::new();
        self.manifest.overflow_binary.clear();
        while let Some(order::Descriptor(mut doc, key)) = descriptors.next()? {
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
                let text = prefix(ctx, &self.root, &key)?;
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
        for id in &self.manifest.tasks {
            let text = extra.remove(id).unwrap_or_default();
            crate::artifact_storage::replace(
                &self.root.join(format!("{id}.extra")),
                text.as_bytes(),
            )?;
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
        // Keep existing board/finder readers on the same extracted-text scoring cache.
        crate::artifact_storage::replace(&parent.join("content-search.json"), &bytes)?;
        self.manifest.prefix_stamp =
            crate::content_refresh::file_stamp(&parent.join("content-search.json"))?;
        // This complete prefix projection belongs to the same frozen view as the catalog.
        // Publish its feed position too, so the existing refresher does not rebuild it.
        crate::content_refresh::write_cursor(
            parent,
            corral_core::backend::ChangeSeq(self.manifest.after),
        )?;
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
        Source::File(_) => anyhow::bail!("catalog refresh requires canonical source text"),
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
                        crate::artifact_storage::artifact_reader(ctx, artifact)?
                    }
                    Source::Text(text) => Box::new(text.as_bytes()),
                    Source::File(_) => {
                        anyhow::bail!("catalog refresh requires canonical source text")
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

fn prefix(ctx: &Ctx, root: &Path, key: &str) -> Result<super::ExtractedText> {
    if let Some(seq) = key.strip_prefix("e-") {
        let text = event_text(ctx, seq.parse().context("invalid indexed event key")?)?;
        return Ok(super::extract_bytes(text.as_bytes()));
    }
    let text = lexical(root, key)?.ok_or(Corrupt)?;
    Ok(super::ExtractedText {
        text: text.prefix,
        complete: text.complete,
        binary: text.binary,
    })
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
