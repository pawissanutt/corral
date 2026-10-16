//! Bounded streaming matching also covers documents too large for the extracted-text cache.
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};

use anyhow::{Context, Result};
use corral_core::Task;
use corral_core::query::{ContentContext, ContentField, Query, SearchContext, content_fields};

use crate::Ctx;
use crate::content_index::{Document, Record, Source};

pub(crate) const SNIPPET_CHARS: usize = 1200;
const CHUNK_BYTES: u64 = 64 * 1024;

pub(crate) struct Match {
    pub start: usize,
    pub end: usize,
    pub snippet: String,
    pub score: u64,
}

/// Terminal-safe snippets never include escape sequences or unbounded source lines.
fn clipped(text: &str, anchor: Option<&str>) -> String {
    let start = anchor
        .and_then(|s| text.to_lowercase().find(s))
        .unwrap_or(0);
    // Case folding can change byte lengths (for example, Kelvin sign -> "k").
    // Translate the folded offset back to a source offset before taking a snippet.
    let mut folded = 0;
    let source_start = text
        .char_indices()
        .find_map(|(offset, c)| {
            if folded >= start {
                Some(offset)
            } else {
                folded += c.to_lowercase().map(char::len_utf8).sum::<usize>();
                None
            }
        })
        .unwrap_or(text.len());
    let mut from = source_start.saturating_sub(120);
    while !text.is_char_boundary(from) {
        from = from.saturating_sub(1);
    }
    text[from..]
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .take(SNIPPET_CHARS)
        .collect()
}

fn field_matches(which: Option<ContentField>, fields: [bool; 3]) -> bool {
    match which {
        None => true,
        Some(ContentField::Test) => fields[0],
        Some(ContentField::Error) => fields[1],
        Some(ContentField::Verdict) => fields[2],
    }
}

fn weight(line: &SourceLine) -> u64 {
    if line.heading == Some(true) {
        8
    } else if line.fields[2] {
        9
    } else if line.fields[0] {
        7
    } else if line.fields[1] {
        6
    } else {
        2
    }
}

/// A line may be arbitrarily large. Retain only marker flags, a bounded word
/// recognizer and one actual-match excerpt per query term until its newline.
struct SourceLine {
    fields: [bool; 3],
    word: String,
    long_word: bool,
    excerpts: Vec<Option<String>>,
    sample: Option<String>,
    heading: Option<bool>,
}

impl SourceLine {
    fn new(terms: usize) -> Self {
        Self {
            fields: [false; 3],
            word: String::new(),
            long_word: false,
            excerpts: vec![None; terms],
            sample: None,
            heading: None,
        }
    }

    fn finish_word(&mut self) {
        if !self.long_word && !self.word.is_empty() {
            let (tests, errors, verdicts) = content_fields(&self.word);
            for (flag, text) in self.fields.iter_mut().zip([tests, errors, verdicts]) {
                *flag |= !text.is_empty();
            }
        }
        self.word.clear();
        self.long_word = false;
    }

    fn observe(
        &mut self,
        fragment: &str,
        window: &str,
        terms: &[(&str, Option<ContentField>, bool)],
    ) {
        self.sample.get_or_insert_with(|| clipped(window, None));
        if self.heading.is_none() {
            self.heading = fragment
                .chars()
                .find(|c| !c.is_whitespace())
                .map(|c| c == '#');
        }
        for c in fragment.chars() {
            if !c.is_alphanumeric() {
                self.finish_word();
            } else if !self.long_word {
                // The shared classifier's longest markers are "failures" and
                // "verdicts" (8 ASCII bytes). A longer/non-ASCII word cannot match.
                if self.word.len() < 8 && c.is_ascii() {
                    self.word.push(c.to_ascii_lowercase());
                } else {
                    self.long_word = true;
                    self.word.clear();
                }
            }
        }
        let lower = window.to_lowercase();
        for (excerpt, &(term, _, _)) in self.excerpts.iter_mut().zip(terms) {
            if excerpt.is_none() && lower.contains(term) {
                *excerpt = Some(clipped(window, Some(term)));
            }
        }
    }

    fn reset(&mut self) {
        self.fields = [false; 3];
        self.word.clear();
        self.long_word = false;
        self.excerpts.fill(None);
        self.sample = None;
        self.heading = None;
    }
}

struct Scan<'a> {
    terms: Vec<(&'a str, Option<ContentField>, bool)>,
    text: String,
    tests: String,
    errors: String,
    verdicts: String,
    seen: Vec<bool>,
    frequencies: Vec<u64>,
    previous: VecDeque<(usize, String)>,
    selected: Vec<(usize, String)>,
    best: u64,
    anchor: usize,
    context: usize,
}

impl<'a> Scan<'a> {
    fn new(query: &'a Query, context: usize) -> Self {
        let terms = query.content_terms();
        let count = terms.len();
        Self {
            terms,
            text: String::new(),
            tests: String::new(),
            errors: String::new(),
            verdicts: String::new(),
            seen: vec![false; count],
            frequencies: vec![0; count],
            previous: VecDeque::new(),
            selected: Vec::new(),
            best: 0,
            anchor: 0,
            context,
        }
    }

    fn visit(&mut self, line: usize, source: &SourceLine) {
        let mut importance = 0;
        let mut excerpt = source.sample.as_deref().unwrap_or("").to_string();
        let mut anchored = false;
        for (i, &(value, which, negative)) in self.terms.iter().enumerate() {
            if let Some(found) = &source.excerpts[i] {
                if !self.seen[i] {
                    self.text.push_str(value);
                    self.text.push('\n');
                    self.seen[i] = true;
                }
                // Field membership is independent of the term's query qualifier.
                for (classified, dest) in [
                    (source.fields[0], &mut self.tests),
                    (source.fields[1], &mut self.errors),
                    (source.fields[2], &mut self.verdicts),
                ] {
                    if classified && !dest.contains(value) {
                        dest.push_str(value);
                        dest.push('\n');
                    }
                }
                if !negative && field_matches(which, source.fields) {
                    let score = weight(source);
                    self.frequencies[i] = self.frequencies[i].saturating_add(score).min(128);
                    importance += score;
                    if !anchored {
                        excerpt.clone_from(found);
                        anchored = true;
                    }
                }
            }
        }
        if self.selected.is_empty() || importance > self.best {
            self.best = importance;
            self.anchor = line;
            self.selected = self
                .previous
                .iter()
                .rev()
                .filter(|(number, _)| *number < line)
                .take(self.context)
                .cloned()
                .collect();
            self.selected.reverse();
            self.selected.push((line, excerpt.clone()));
        } else if line > self.anchor
            && line <= self.anchor.saturating_add(self.context)
            && self
                .selected
                .last()
                .is_none_or(|(number, _)| *number != line)
        {
            self.selected.push((line, excerpt.clone()));
        }
        if self.previous.back().is_none_or(|(n, _)| *n != line) {
            self.previous.push_back((line, excerpt));
        }
        while self.previous.len() > self.context + 1 {
            self.previous.pop_front();
        }
    }
}

pub(crate) fn search_record(
    ctx: &Ctx,
    record: &Record,
    task: &Task,
    query: &Query,
    metadata: SearchContext<'_>,
    context: usize,
    idfs: &[u64],
) -> Result<Option<Match>> {
    let doc = &record.document;
    let mut scan = Scan::new(query, context);
    if !doc.binary {
        let reader: Box<dyn Read + '_> = if doc.complete {
            Box::new(doc.text.as_bytes())
        } else {
            match &record.source {
                Source::Text(text) => Box::new(text.as_bytes()),
                Source::Artifact(artifact) => {
                    crate::artifact_storage::artifact_reader(ctx, artifact)?
                }
            }
        };
        scan_reader(reader, &mut scan)?;
    }
    if !query.matches_content(
        task,
        metadata,
        ContentContext {
            title: &doc.title,
            text: &scan.text,
            tests: &scan.tests,
            errors: &scan.errors,
            verdicts: &scan.verdicts,
            kind: &doc.kind,
            updated: doc.updated,
        },
    ) {
        return Ok(None);
    }
    Ok(Some(finish(doc, scan, idfs)))
}

fn finish(doc: &Document, scan: Scan<'_>, idfs: &[u64]) -> Match {
    let mut score = 0_u64;
    for (i, &(term, field, negative)) in scan.terms.iter().enumerate() {
        if negative {
            continue;
        }
        let title = if field.is_none() && doc.title.to_lowercase().contains(term) {
            16
        } else {
            0
        };
        let frequency = scan.frequencies[i];
        // Saturating term frequency and inverse document frequency, with field boosts.
        score = score
            .saturating_add((title + frequency.saturating_mul(16) / (frequency + 8)) * idfs[i]);
    }
    let start = scan.selected.first().map_or(1, |(n, _)| *n);
    let end = scan.selected.last().map_or(1, |(n, _)| *n);
    let mut chars_left = SNIPPET_CHARS;
    // Preserve the matching line when preceding context alone would exhaust the budget.
    let count = scan.selected.len();
    let per_context = (SNIPPET_CHARS / 2).saturating_sub(count) / count.saturating_sub(1).max(1);
    let mut lines = Vec::new();
    for (number, text) in scan.selected {
        let cap = if count == 1 {
            SNIPPET_CHARS
        } else if number == scan.anchor {
            SNIPPET_CHARS / 2
        } else {
            per_context
        };
        let clipped: String = text.chars().take(cap.min(chars_left)).collect();
        chars_left = chars_left.saturating_sub(clipped.chars().count() + 1);
        lines.push(clipped);
    }
    Match {
        start,
        end,
        snippet: lines.join("\n").chars().take(SNIPPET_CHARS).collect(),
        score,
    }
}

fn scan_reader(reader: Box<dyn Read + '_>, scan: &mut Scan<'_>) -> Result<()> {
    let mut reader = BufReader::new(reader);
    let overlap_limit = scan
        .terms
        .iter()
        .map(|(term, _, _)| term.chars().count().saturating_mul(4))
        .max()
        .unwrap_or(0);
    let mut bytes = Vec::new();
    let mut overlap = String::new();
    let mut utf8_tail = Vec::new();
    let mut source = SourceLine::new(scan.terms.len());
    let mut line = 1_usize;
    loop {
        bytes.clear();
        let size = reader
            .by_ref()
            .take(CHUNK_BYTES)
            .read_until(b'\n', &mut bytes)
            .context("streaming search content")?;
        if size == 0 {
            if !utf8_tail.is_empty() {
                let tail = String::from_utf8_lossy(&utf8_tail);
                overlap.push_str(&tail);
                source.observe(&tail, &overlap, &scan.terms);
            }
            if source.sample.is_some() {
                source.finish_word();
                scan.visit(line, &source);
            }
            break;
        }
        let ended = bytes.last() == Some(&b'\n');
        let fragment = decode_fragment(&bytes, &mut utf8_tail);
        let fragment = if ended {
            fragment.trim_end_matches(['\r', '\n'])
        } else {
            fragment.as_str()
        };
        overlap.push_str(fragment);
        source.observe(fragment, &overlap, &scan.terms);
        if ended {
            source.finish_word();
            scan.visit(line, &source);
            line = line.saturating_add(1);
            source.reset();
            overlap.clear();
        } else {
            // Keep only the largest query term's possible source UTF-8 bytes.
            // Line classification is carried separately, not inferred from this window.
            let mut start = overlap.len().saturating_sub(overlap_limit);
            while !overlap.is_char_boundary(start) {
                start += 1;
            }
            overlap.drain(..start);
        }
    }
    Ok(())
}

/// Decode bounded fragments without inventing word boundaries inside split UTF-8.
fn decode_fragment(bytes: &[u8], tail: &mut Vec<u8>) -> String {
    let mut data = std::mem::take(tail);
    data.extend_from_slice(bytes);
    let mut rest = data.as_slice();
    let mut text = String::new();
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                text.push_str(valid);
                break;
            }
            Err(error) => {
                text.push_str(&String::from_utf8_lossy(&rest[..error.valid_up_to()]));
                match error.error_len() {
                    Some(size) => {
                        text.push('\u{fffd}');
                        rest = &rest[error.valid_up_to() + size..];
                    }
                    None => {
                        tail.extend_from_slice(&rest[error.valid_up_to()..]);
                        break;
                    }
                }
            }
        }
    }
    text
}
