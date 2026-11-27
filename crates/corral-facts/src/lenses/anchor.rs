use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

use corral_types::lenses::EvidenceAnchor;

use sha2::{Digest, Sha256};

use super::LensCoverage;
use crate::{
    EntryDetector, EntryKind, FileFact, TreeFacts,
    git::TreeFile,
    syntax::{RustEnum, rust_declarations},
};

pub(super) struct Source {
    pub blob: Option<String>,
    pub binary: bool,
    pub limited: bool,
    pub text: Option<Text>,
    symbols: BTreeMap<String, Option<usize>>,
    pub enums: BTreeMap<String, RustEnum>,
}

/// Bound retained anchor text and indexes across all lenses at a revision.
/// Individual blob reads also have the Git scanner's independent limit.
pub(super) struct SourceBudget(usize);

impl Default for SourceBudget {
    fn default() -> Self {
        Self(16 * 1024 * 1024)
    }
}

impl SourceBudget {
    pub fn read(&mut self, blob: &TreeFile<'_>, names: &BTreeSet<&str>) -> Source {
        let mut source = Source {
            blob: blob.regular_blob.map(str::to_owned),
            binary: blob.regular_blob.is_some() && blob.text.is_none() && blob.lines == 0,
            limited: blob.text.is_none() && blob.lines > 0,
            text: None,
            enums: BTreeMap::new(),
            symbols: BTreeMap::new(),
        };
        if let Some(text) = blob.text {
            let bytes = text.len() + (blob.lines as usize + 1) * size_of::<usize>();
            if bytes <= self.0 {
                let declarations = if blob.path.ends_with(".rs") {
                    rust_declarations(text, names)
                } else {
                    Default::default()
                };
                let enum_bytes: usize = declarations
                    .enums
                    .iter()
                    .map(|(name, item)| name.len() + 128 + item.variant_bytes)
                    .sum::<usize>()
                    + declarations
                        .symbols
                        .keys()
                        .map(|key| key.len() + 64)
                        .sum::<usize>();
                if let Some(remaining) = self.0.checked_sub(bytes + enum_bytes) {
                    self.0 = remaining;
                    source.text = Some(Text::new(text, blob.lines));
                    source.enums = declarations.enums;
                    source.symbols = declarations.symbols;
                }
            }
            source.limited = source.text.is_none();
        }
        source
    }
}

pub(super) struct Text {
    source: String,
    starts: Vec<usize>,
    pub lines: u64,
}

impl Text {
    fn new(text: &str, lines: u64) -> Self {
        let mut starts = Vec::with_capacity(lines as usize + 1);
        starts.push(0);
        starts.extend(text.match_indices('\n').map(|(offset, _)| offset + 1));
        Self {
            source: text.to_owned(),
            starts,
            lines,
        }
    }

    fn line_of(&self, offset: usize) -> u64 {
        self.starts.partition_point(|&start| start <= offset) as u64
    }

    pub(super) fn digest(&self, span: Range<u64>, budget: &mut usize) -> Option<String> {
        if span.start >= span.end || span.end - 1 > self.lines {
            return None;
        }
        let start = *self.starts.get(span.start.checked_sub(1)? as usize)?;
        let end = self
            .starts
            .get((span.end - 1) as usize)
            .copied()
            .unwrap_or(self.source.len());
        let bytes = self.source.as_bytes().get(start..end)?;
        *budget = budget.checked_sub(bytes.len())?;
        Some(format!("{:x}", Sha256::digest(bytes)))
    }

    fn unique(&self, needle: &str, path: &str) -> Result<Range<u64>, Issue> {
        let start = self.source.find(needle).ok_or_else(|| {
            Issue::Failed(format!("anchor text {needle:?} not found in {path:?}"))
        })?;
        // Advance by just one scalar: overlapping occurrences are ambiguous too.
        let next = start
            + needle
                .chars()
                .next()
                .ok_or_else(|| Issue::Failed("empty text anchor".into()))?
                .len_utf8();
        if self.source[next..].contains(needle) {
            return Err(Issue::Failed(format!("ambiguous anchor text in {path:?}")));
        }
        Ok(self.line_of(start)..self.line_of(start + needle.len() - 1) + 1)
    }
}

fn file_fact<'a>(tree: &'a TreeFacts, path: &str) -> Option<&'a FileFact> {
    let index = tree
        .files
        .binary_search_by(|file| file.path.as_str().cmp(path))
        .ok()?;
    tree.files.get(index)
}

pub(super) enum Issue {
    Failed(String),
    Unchecked(String),
}

pub(super) fn resolve<'a>(
    anchor: &EvidenceAnchor,
    tree: &TreeFacts,
    coverage: Option<LensCoverage<'_>>,
    source: impl Fn(&str) -> Option<&'a Source>,
) -> Result<(String, Range<u64>), Issue> {
    use Issue::{Failed, Unchecked};
    let path = match anchor {
        EvidenceAnchor::Path { path }
        | EvidenceAnchor::Text { path, .. }
        | EvidenceAnchor::Symbol { path, .. } => path,
        EvidenceAnchor::Entry { entry_kind, name } => {
            let points = &tree.entry_points;
            let mut entries = points
                .entries
                .iter()
                .filter(|entry| entry.kind == *entry_kind && entry.name == *name);
            let partial = coverage.is_some_and(|c| {
                c.provider.provides("entries") && (c.has_omissions() || !c.unread_files.is_empty())
            });
            if let Some(entry) = entries.next() {
                if entries.any(|other| other.path != entry.path || other.line != entry.line) {
                    return Err(Failed(format!("ambiguous entry {name:?}")));
                }
                if partial {
                    return Err(Unchecked(format!(
                        "cannot establish a unique entry {name:?}: partial provider answer"
                    )));
                }
                if entry.line == 0
                    || file_fact(tree, &entry.path).is_none_or(|f| f.lines < entry.line)
                {
                    return Err(Unchecked("entry source range unavailable".into()));
                }
                return Ok((entry.path.clone(), entry.line..entry.line + 1));
            }
            let covered = points.entries.iter().any(|e| e.kind == *entry_kind)
                || points.coverage.values().flatten().any(|d| match d {
                    EntryDetector::Extension => false,
                    EntryDetector::Clap | EntryDetector::NpmBin => {
                        *entry_kind == EntryKind::Command
                    }
                    EntryDetector::Axum | EntryDetector::Express => *entry_kind == EntryKind::Route,
                    EntryDetector::OperationEnum => *entry_kind == EntryKind::Operation,
                });
            return Err(if covered && !partial {
                Failed(format!("unknown {entry_kind:?} entry point {name:?}"))
            } else {
                Unchecked(format!("entry {name:?} not covered"))
            });
        }
    };
    let s = source(path).ok_or_else(|| Failed(format!("anchor path {path:?} does not exist")))?;
    if s.blob.is_none() {
        return Err(Failed(format!(
            "anchor path {path:?} is not a regular file"
        )));
    }
    let text = s.text.as_ref().ok_or_else(|| {
        Unchecked(format!(
            "source unavailable in {path:?}: unreadable or beyond scan budget"
        ))
    })?;
    let span = match anchor {
        EvidenceAnchor::Path { .. } if text.lines > 0 => 1..text.lines + 1,
        EvidenceAnchor::Path { .. } => return Err(Unchecked(format!("empty source at {path:?}"))),
        EvidenceAnchor::Text { text: needle, .. } => text.unique(needle, path)?,
        EvidenceAnchor::Symbol { name, .. } => {
            let mut defs = file_fact(tree, path)
                .into_iter()
                .flat_map(|f| &f.definitions)
                .filter(|d| d.name == *name);
            let first = defs.next();
            if defs.next().is_some() || s.symbols.get(name) == Some(&None) {
                return Err(Failed(format!("ambiguous symbol {name:?} in {path:?}")));
            }
            let offset = s.symbols.get(name).copied().flatten().or_else(|| {
                let (item, variant) = name.split_once("::")?;
                s.enums.get(item)?.variants.get(variant).copied()
            });
            let partial =
                coverage.is_some_and(|c| c.has_omissions() || c.unread_files.contains(path));
            if first.is_some() && partial && offset.is_none() {
                return Err(Unchecked(format!(
                    "cannot establish a unique symbol {name:?} in {path:?}: partial provider answer"
                )));
            }
            if let Some(def) = first {
                if def.line == 0 || def.line > def.end_line || def.end_line > text.lines {
                    return Err(Unchecked("definition range unavailable".into()));
                }
                if offset.is_some_and(|o| !(def.line..=def.end_line).contains(&text.line_of(o))) {
                    return Err(Failed(format!("conflicting symbol locations for {name:?}")));
                }
                def.line..def.end_line + 1
            } else if let Some(offset) = offset {
                let line = text.line_of(offset);
                line..line + 1
            } else if coverage.is_some_and(|c| {
                c.provider.provides("definitions")
                    && !c.has_omissions()
                    && !c.unread_files.contains(path)
            }) {
                return Err(Failed(format!("unknown symbol {name:?} in {path:?}")));
            } else {
                return Err(Unchecked(format!(
                    "symbol {name:?} not located by built-in syntax; definitions not provided completely"
                )));
            }
        }
        EvidenceAnchor::Entry { .. } => unreachable!(),
    };
    Ok((path.to_owned(), span))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_uses_complete_bound_lines_only() {
        let digest = |s: &str, mut bytes: usize| Text::new(s, 4).digest(2..4, &mut bytes);
        let a = digest("a\nb\nc\nd", 4);
        assert!(a.is_some());
        assert_eq!(a, digest("x\nb\nc\ny", 4));
        assert_ne!(a, digest("a\nb\nx\nd", 4));
        assert_eq!(None, digest("a\nb\nc\nd", 3));
    }

    #[test]
    fn lens_source_budget_counts_retained_line_indexes() {
        let blob = TreeFile {
            path: "source.txt",
            text: Some("a\nb\n"),
            lines: 2,
            regular_blob: Some("object"),
        };
        let mut budget = SourceBudget(4 + 3 * size_of::<usize>());
        let names = BTreeSet::new();
        let first = budget.read(&blob, &names);
        assert!(first.text.is_some());
        let next = budget.read(&blob, &names);
        assert!(next.limited);
        assert!(next.text.is_none());
        assert_eq!(next.blob.as_deref(), blob.regular_blob);
    }
}
