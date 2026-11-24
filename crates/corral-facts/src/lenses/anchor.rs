use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

use corral_types::lenses::{LensAnchor, LensLocation, LensResult};

use super::{Element, LensCoverage, LensSources, note};
use crate::{
    DefinitionFact, FileFact, TreeFacts,
    git::TreeFile,
    syntax::{RustEnum, rust_enums},
};

pub(super) struct Source {
    pub blob: Option<String>,
    pub binary: bool,
    pub limited: bool,
    pub text: Option<Text>,
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
        };
        if let Some(text) = blob.text {
            let bytes = text.len() + (blob.lines as usize + 1) * size_of::<usize>();
            if bytes <= self.0 {
                let enums = if blob.path.ends_with(".rs") {
                    rust_enums(text, names)
                } else {
                    BTreeMap::new()
                };
                let enum_bytes: usize = enums
                    .iter()
                    .map(|(name, item)| name.len() + 128 + item.variant_bytes)
                    .sum();
                if let Some(remaining) = self.0.checked_sub(bytes + enum_bytes) {
                    self.0 = remaining;
                    source.text = Some(Text::new(text, blob.lines));
                    source.enums = enums;
                }
            }
            source.limited = source.text.is_none();
        }
        source
    }
}

impl Source {
    pub fn syntax_line(&self, name: &str) -> Option<u64> {
        let offset = if let Some((name, variant)) = name.split_once("::") {
            *self.enums.get(name)?.variants.get(variant)?
        } else {
            self.enums.get(name)?.declaration
        };
        Some(self.text.as_ref()?.line_of(offset))
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

    fn in_definition(&self, def: &DefinitionFact) -> Option<Range<usize>> {
        if def.line == 0 || def.end_line < def.line || def.end_line > self.lines {
            return None;
        }
        let start = self.starts[(def.line - 1) as usize];
        let end = self
            .starts
            .get(def.end_line as usize)
            .copied()
            .unwrap_or(self.source.len());
        Some(start..end)
    }

    pub fn occurrence(&self, needle: &str, definitions: &[&DefinitionFact]) -> Option<Range<u64>> {
        definitions
            .iter()
            .filter_map(|def| self.in_definition(def))
            .chain(definitions.is_empty().then_some(0..self.source.len()))
            .filter_map(|scope| {
                let start = scope.start + self.source[scope].find(needle)?;
                let end = start + needle.len() - 1;
                Some(self.line_of(start)..self.line_of(end) + 1)
            })
            .min_by_key(|span| (span.start, span.end))
    }
}

fn file_fact<'a>(tree: &'a TreeFacts, path: &str) -> Option<&'a FileFact> {
    let index = tree
        .files
        .binary_search_by(|file| file.path.as_str().cmp(path))
        .ok()?;
    tree.files.get(index)
}

pub(super) fn definitions<'a>(tree: &'a TreeFacts, anchor: &LensAnchor) -> Vec<&'a DefinitionFact> {
    file_fact(tree, &anchor.path)
        .into_iter()
        .flat_map(|file| &file.definitions)
        .filter(|def| anchor.name.as_ref() == Some(&def.name))
        .collect()
}

fn missing_def_reason(path: &str, coverage: Option<LensCoverage<'_>>) -> Option<&'static str> {
    let Some(coverage) = coverage.filter(|coverage| coverage.provider.provides("definitions"))
    else {
        return Some("definitions not provided");
    };
    if coverage.unread_files.contains(path) {
        return Some("file was not read completely by the provider");
    }
    coverage
        .has_omissions()
        .then_some("provider answer had skipped or omitted facts")
}

pub(super) fn locate(result: &mut LensResult, element: &str, path: &str, line: u64) {
    result.locations.push(LensLocation {
        element: element.to_owned(),
        path: path.to_owned(),
        line,
    });
}

pub(super) fn check(
    result: &mut LensResult,
    element: &Element<'_>,
    sources: &LensSources,
    tree: &TreeFacts,
    coverage: Option<LensCoverage<'_>>,
) {
    let Element {
        name: key, anchor, ..
    } = element;
    let Some(source) = sources.sources.get(&anchor.path) else {
        note(
            &mut result.failures,
            key,
            format!("anchor path {:?} does not exist", anchor.path),
        );
        return;
    };
    let defs = if coverage.is_some_and(|coverage| coverage.provider.provides("definitions")) {
        definitions(tree, anchor)
    } else {
        Vec::new()
    };
    if let Some(name) = &anchor.name
        && defs.is_empty()
    {
        if let Some(reason) = missing_def_reason(&anchor.path, coverage) {
            note(
                &mut result.not_checked,
                key,
                format!("name {name:?} not checked: {reason}"),
            );
        } else {
            note(
                &mut result.failures,
                key,
                format!("unknown definition {name:?} in {:?}", anchor.path),
            );
        }
    }
    let mut line = None;
    if let Some(needle) = &anchor.text {
        match &source.text {
            None => note(
                &mut result.not_checked,
                key,
                if source.limited {
                    "anchor text not checked: source exceeded the lens scan budget"
                } else {
                    "anchor text not checked: source is not a regular text blob"
                },
            ),
            Some(text)
                if !defs.is_empty()
                    && !defs.iter().any(|def| text.in_definition(def).is_some()) =>
            {
                note(
                    &mut result.not_checked,
                    key,
                    "anchor text cannot be checked: definition range unavailable",
                );
            }
            Some(text) => {
                if let Some(span) = text.occurrence(needle, &defs) {
                    line = Some(span.start);
                } else {
                    let scope = if defs.is_empty() {
                        "the file"
                    } else {
                        "the named definition"
                    };
                    note(
                        &mut result.failures,
                        key,
                        format!("anchor text {needle:?} not found in {scope}"),
                    );
                }
            }
        }
    }
    if let Some(line) = line
        .or_else(|| defs.iter().map(|def| def.line).min())
        .or_else(|| {
            anchor
                .name
                .as_deref()
                .and_then(|name| source.syntax_line(name))
        })
        .or_else(|| {
            (anchor.name.is_none() && anchor.text.is_none() && source.text.is_some()).then_some(1)
        })
    {
        locate(result, key, &anchor.path, line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
