//! External ordering for legacy prefix admission: bounded descriptors, not all events in RAM.
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::content_index::Document;

#[derive(Serialize, Deserialize)]
pub(super) struct Descriptor(pub Document, pub String, pub(super) super::IndexedSource);

impl PartialEq for Descriptor {
    fn eq(&self, other: &Self) -> bool {
        self.0.updated == other.0.updated && self.0.reference == other.0.reference
    }
}
impl Eq for Descriptor {}
impl Ord for Descriptor {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .updated
            .cmp(&other.0.updated)
            .then_with(|| other.0.reference.cmp(&self.0.reference))
    }
}
impl PartialOrd for Descriptor {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub(super) struct Order {
    root: PathBuf,
    chunk: Vec<Descriptor>,
    bytes: usize,
    runs: Vec<tempfile::NamedTempFile>,
}

impl Order {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            chunk: Vec::new(),
            bytes: 0,
            runs: Vec::new(),
        }
    }

    pub fn push(
        &mut self,
        document: Document,
        lexical: String,
        source: super::IndexedSource,
    ) -> Result<()> {
        self.bytes += document.reference.len() + document.title.len() + lexical.len() + 512;
        self.chunk.push(Descriptor(document, lexical, source));
        if self.bytes >= 2 * 1024 * 1024 {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if self.chunk.is_empty() {
            return Ok(());
        }
        self.chunk.sort_by(|a, b| b.cmp(a));
        let mut file = tempfile::NamedTempFile::new_in(&self.root)
            .context("creating sorted search descriptors")?;
        {
            let mut writer = BufWriter::new(file.as_file_mut());
            for descriptor in self.chunk.drain(..) {
                emit(&mut writer, &descriptor)?;
            }
            writer
                .flush()
                .context("flushing sorted search descriptors")?;
        }
        self.bytes = 0;
        self.runs.push(file);
        // Limit simultaneously open files as well as heap descriptors. The temporary
        // merge files are removed by NamedTempFile on both success and failure.
        if self.runs.len() == 32 {
            let mut sorted = Sorted::new(std::mem::take(&mut self.runs))?;
            let mut file = tempfile::NamedTempFile::new_in(&self.root)
                .context("creating merged search descriptors")?;
            {
                let mut writer = BufWriter::new(file.as_file_mut());
                while let Some(descriptor) = sorted.next()? {
                    emit(&mut writer, &descriptor)?;
                }
                writer
                    .flush()
                    .context("flushing merged search descriptors")?;
            }
            self.runs.push(file);
        }
        Ok(())
    }

    pub fn sorted(mut self) -> Result<Sorted> {
        self.flush()?;
        Sorted::new(self.runs)
    }
}

pub(super) struct Sorted {
    _files: Vec<tempfile::NamedTempFile>,
    readers: Vec<BufReader<File>>,
    heap: BinaryHeap<(Descriptor, usize)>,
    line: String,
}

impl Sorted {
    fn new(mut files: Vec<tempfile::NamedTempFile>) -> Result<Self> {
        let mut readers = Vec::new();
        for file in &mut files {
            file.as_file_mut()
                .seek(SeekFrom::Start(0))
                .context("rewinding search descriptors")?;
            readers.push(BufReader::new(
                file.reopen().context("opening sorted search descriptors")?,
            ));
        }
        let mut sorted = Self {
            _files: files,
            readers,
            heap: BinaryHeap::new(),
            line: String::new(),
        };
        for i in 0..sorted.readers.len() {
            sorted.refill(i)?;
        }
        Ok(sorted)
    }

    fn refill(&mut self, i: usize) -> Result<()> {
        self.line.clear();
        if self.readers[i]
            .read_line(&mut self.line)
            .context("reading sorted search descriptor")?
            != 0
        {
            let descriptor =
                serde_json::from_str(&self.line).context("decoding sorted search descriptor")?;
            self.heap.push((descriptor, i));
        }
        Ok(())
    }

    pub fn next(&mut self) -> Result<Option<Descriptor>> {
        let Some((descriptor, i)) = self.heap.pop() else {
            return Ok(None);
        };
        self.refill(i)?;
        Ok(Some(descriptor))
    }
}

fn emit(writer: &mut impl Write, descriptor: &Descriptor) -> Result<()> {
    serde_json::to_writer(&mut *writer, descriptor).context("writing sorted search descriptor")?;
    writer
        .write_all(b"\n")
        .context("terminating sorted search descriptor")
}
