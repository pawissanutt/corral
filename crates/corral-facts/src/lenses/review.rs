use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ops::Range,
    path::Path,
};

use anyhow::{Context, Result, ensure};
use corral_types::lenses::{Lens, LensAnchor, LensImpact};

use super::{Element, ElementKey, LensSources, Source, SourceBudget, anchor, elements};
use crate::{
    Revision, TreeFacts,
    git::{self, LineDiff, LineHunk},
};

/// Compare the lens snapshots and the actual old/new edited lines. An insertion
/// above a surviving anchor shifts its location but does not touch it.
pub fn impact(
    repo: &Path,
    base: &Revision,
    head: &Revision,
    before: &LensSources,
    after: &LensSources,
    before_tree: &TreeFacts,
    after_tree: &TreeFacts,
) -> Result<Vec<LensImpact>> {
    ensure!(
        before.tree == base.tree() && after.tree == head.tree(),
        "lens sources do not match review trees"
    );
    if base.tree() == head.tree() {
        return Ok(Vec::new());
    }
    ensure!(
        !before.files_limited && !after.files_limited,
        "could not compare lens files beyond the scan budget"
    );
    let mut review = Review {
        repo,
        head,
        before: Snapshot::new(repo, base, before, after, before_tree)?,
        after: Snapshot::new(repo, head, after, before, after_tree)?,
        diff: None,
        hunks: BTreeMap::new(),
    };
    let paths: BTreeSet<_> = before.lenses.keys().chain(after.lenses.keys()).collect();
    let mut result = Vec::new();
    for path in paths {
        let left = before.lenses.get(path);
        let right = after.lenses.get(path);
        let old = left.and_then(|lens| lens.as_ref().ok());
        let new = right.and_then(|lens| lens.as_ref().ok());
        let Some(lens) = new.or(old) else { continue };
        let mut row = LensImpact {
            path: path.to_owned(),
            id: lens.id().to_owned(),
            title: lens.title().to_owned(),
            touched: Vec::new(),
            added: Vec::new(),
            removed: Vec::new(),
        };
        // A malformed file has unknown contents, not an empty diagram.
        let know_old = left.is_none_or(Result::is_ok);
        let know_new = right.is_none_or(Result::is_ok);
        let mut matches = group(old);
        for (key, mut next) in group(new) {
            let prev = matches.remove(&key).unwrap_or_default();
            let mut unmatched = Vec::new();
            // Match repeats by anchor/qualifier before pairing edited elements.
            for (identity, elements) in prev {
                for element in elements {
                    if let Some(other) = next.get_mut(&identity).and_then(VecDeque::pop_front) {
                        review.element(&mut row, Some(element), Some(other), know_old, know_new)?;
                    } else {
                        unmatched.push(element);
                    }
                }
            }
            let mut prev = unmatched.into_iter();
            let mut next = next.into_values().flatten();
            loop {
                let (left, right) = (prev.next(), next.next());
                if left.is_none() && right.is_none() {
                    break;
                }
                review.element(&mut row, left, right, know_old, know_new)?;
            }
        }
        for element in matches
            .into_values()
            .flat_map(BTreeMap::into_values)
            .flatten()
        {
            review.element(&mut row, Some(element), None, know_old, know_new)?;
        }
        row.touched.sort_unstable();
        row.touched.dedup();
        row.added.sort_unstable();
        row.removed.sort_unstable();
        if !row.touched.is_empty() || !row.added.is_empty() || !row.removed.is_empty() {
            result.push(row);
        }
    }
    Ok(result)
}

type Repeats<'a> = BTreeMap<(Option<&'a str>, &'a LensAnchor), VecDeque<Element<'a>>>;
type Groups<'a> = BTreeMap<ElementKey<'a>, Repeats<'a>>;

fn group(lens: Option<&Lens>) -> Groups<'_> {
    let mut groups = Groups::new();
    for element in lens.into_iter().flat_map(elements) {
        groups
            .entry(element.key)
            .or_default()
            .entry((element.qualifier, element.anchor))
            .or_default()
            .push_back(element);
    }
    groups
}

struct Snapshot<'a> {
    lenses: &'a LensSources,
    tree: &'a TreeFacts,
    extra: BTreeMap<String, Source>,
}

impl<'a> Snapshot<'a> {
    fn new(
        repo: &Path,
        rev: &Revision,
        lenses: &'a LensSources,
        other: &LensSources,
        tree: &'a TreeFacts,
    ) -> Result<Self> {
        // A newly added/removed lens may reference a path the other snapshot
        // didn't need for checks. Read those counterparts to compare safely.
        let paths: BTreeSet<_> = other
            .requested
            .difference(&lenses.requested)
            .map(String::as_str)
            .collect();
        let mut extra = BTreeMap::new();
        if !paths.is_empty() {
            let mut budget = SourceBudget::default();
            git::tree_selected(
                repo,
                rev.tree(),
                |path| paths.contains(path),
                |blob| {
                    extra.insert(blob.path.to_owned(), budget.read(&blob, &BTreeSet::new()));
                    Ok(())
                },
            )?;
        }
        Ok(Self {
            lenses,
            tree,
            extra,
        })
    }

    fn source(&self, path: &str) -> Option<&Source> {
        self.lenses
            .sources
            .get(path)
            .or_else(|| self.extra.get(path))
    }

    fn resolve(&self, anchor: &LensAnchor) -> Option<Resolved> {
        let source = self.source(&anchor.path)?;
        let text = source.text.as_ref()?;
        let defs = anchor::definitions(self.tree, anchor);
        let line = if let Some(needle) = &anchor.text {
            text.occurrence(needle, &defs)
        } else if let Some(name) = &anchor.name {
            source.syntax_line(name).map(|line| line..line + 1)
        } else {
            (text.lines > 0).then_some(1..2)
        };
        let definitions = defs
            .into_iter()
            .filter(|def| def.line > 0 && def.end_line >= def.line && def.end_line <= text.lines)
            .map(|def| def.line..def.end_line + 1)
            .collect();
        Some(Resolved { line, definitions })
    }
}

struct Resolved {
    line: Option<Range<u64>>,
    definitions: Vec<Range<u64>>,
}

struct Review<'a> {
    repo: &'a Path,
    head: &'a Revision,
    before: Snapshot<'a>,
    after: Snapshot<'a>,
    diff: Option<LineDiff>,
    hunks: BTreeMap<String, Vec<LineHunk>>,
}

impl Review<'_> {
    fn element(
        &mut self,
        row: &mut LensImpact,
        old: Option<Element<'_>>,
        new: Option<Element<'_>>,
        know_old: bool,
        know_new: bool,
    ) -> Result<()> {
        if let Some(old) = &old
            && new.is_none()
            && know_new
        {
            row.removed.push(old.name.clone());
        }
        if let Some(new) = &new
            && old.is_none()
            && know_old
        {
            row.added.push(new.name.clone());
        }
        let old_anchor = old.as_ref().map(|element| element.anchor);
        let new_anchor = new.as_ref().map(|element| element.anchor);
        if self.touched(old_anchor, new_anchor)?
            && let Some(element) = new.or(old)
        {
            row.touched.push(element.name);
        }
        Ok(())
    }

    fn touched(&mut self, old: Option<&LensAnchor>, new: Option<&LensAnchor>) -> Result<bool> {
        // An unreadable changed text blob is not a zero-line diff. Fail this
        // category closed rather than reporting a clean review for that lens.
        for anchor in old.into_iter().chain(new) {
            let left = self.before.source(&anchor.path);
            let right = self.after.source(&anchor.path);
            ensure!(
                !(left.is_some_and(|source| source.limited)
                    || right.is_some_and(|source| source.limited))
                    || left.and_then(|source| source.blob.as_ref())
                        == right.and_then(|source| source.blob.as_ref()),
                "could not diff anchor source {:?} beyond the scan budget",
                anchor.path
            );
        }
        let mut old_line = None;
        if let Some(anchor) = old
            && let Some(location) = self.before.resolve(anchor)
        {
            let hunks = self.hunks(&anchor.path)?;
            for hunk in hunks {
                if location
                    .definitions
                    .iter()
                    .chain(&location.line)
                    .any(|span| overlaps(span, &hunk.old))
                {
                    return Ok(true);
                }
                if old == new
                    && hunk.old.is_empty()
                    && location.line.as_ref().is_some_and(|span| {
                        // Inserting inside a multiline literal edits that anchor, but
                        // inserting at its first line just moves it down.
                        span.start < hunk.old.start && hunk.old.start < span.end
                    })
                {
                    return Ok(true);
                }
            }
            old_line = location.line;
        }
        if let Some(anchor) = new
            && let Some(location) = self.after.resolve(anchor)
        {
            let hunks = self.hunks(&anchor.path)?;
            // For an unchanged anchor whose old location survives, don't hook it
            // onto an unrelated newly inserted occurrence of the same text.
            let line = if old == new && old_line.is_some() {
                None
            } else {
                location.line
            };
            return Ok(hunks.iter().any(|hunk| {
                location
                    .definitions
                    .iter()
                    .chain(&line)
                    .any(|span| overlaps(span, &hunk.new))
            }));
        }
        Ok(false)
    }

    fn hunks(&mut self, path: &str) -> Result<&[LineHunk]> {
        use std::collections::btree_map::Entry;
        let hunks = match self.hunks.entry(path.to_owned()) {
            Entry::Occupied(value) => value.into_mut(),
            Entry::Vacant(entry) => {
                let left = self.before.source(path);
                let right = self.after.source(path);
                // A verified binary has no source lines, like a missing path or
                // a symlink. Oversized UTF-8 is unknown, not an empty file.
                let old = left
                    .filter(|source| !source.binary)
                    .and_then(|source| source.blob.as_deref());
                let new = right
                    .filter(|source| !source.binary)
                    .and_then(|source| source.blob.as_deref());
                let old_text = left.and_then(|source| source.text.as_ref());
                let new_text = right.and_then(|source| source.text.as_ref());
                let hunks = match (old, new) {
                    (Some(old), Some(new))
                        if old != new && old_text.is_some() && new_text.is_some() =>
                    {
                        if self.diff.is_none() {
                            self.diff = Some(LineDiff::new(self.repo, self.head.commit())?);
                        }
                        self.diff
                            .as_ref()
                            .context("missing opened Git diff context")?
                            .hunks(old, new)
                            .with_context(|| format!("diff anchor source {path:?}"))?
                    }
                    (Some(_), None) => old_text
                        .into_iter()
                        .map(|text| LineHunk {
                            old: 1..text.lines + 1,
                            new: 1..1,
                        })
                        .collect(),
                    (None, Some(_)) => new_text
                        .into_iter()
                        .map(|text| LineHunk {
                            old: 1..1,
                            new: 1..text.lines + 1,
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                entry.insert(hunks)
            }
        };
        Ok(hunks)
    }
}

fn overlaps(a: &Range<u64>, b: &Range<u64>) -> bool {
    !a.is_empty() && !b.is_empty() && a.start < b.end && b.start < a.end
}
