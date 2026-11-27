use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    path::Path,
};

use anyhow::{Context, Result, ensure};
use corral_types::lenses::{EvidenceAnchor, LensImpact, RepoRecipe};

use super::{LensSources, Source, SourceBudget, anchor, ids};
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
    // Entries are addressed by kind/name, so their paths are not known at recipe load time.
    let mut entry_paths = BTreeSet::new();
    for (sources, tree) in [(before, before_tree), (after, after_tree)] {
        let wanted: BTreeSet<_> = sources
            .lenses
            .values()
            .filter_map(|r| r.as_ref().ok())
            .flat_map(|r| r.evidence.iter())
            .filter_map(|e| match &e.anchor {
                EvidenceAnchor::Entry { entry_kind, name } => Some((*entry_kind, name.as_str())),
                _ => None,
            })
            .collect();
        entry_paths.extend(
            tree.entry_points
                .entries
                .iter()
                .filter(|e| wanted.contains(&(e.kind, e.name.as_str())))
                .map(|e| e.path.as_str()),
        );
    }
    let mut review = Review {
        repo,
        head,
        before: Snapshot::new(repo, base, before, after, before_tree, &entry_paths)?,
        after: Snapshot::new(repo, head, after, before, after_tree, &entry_paths)?,
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
            touched_base: Vec::new(),
        };
        // A malformed file has unknown contents, not an empty diagram.
        let know_old = left.is_none_or(Result::is_ok);
        let know_new = right.is_none_or(Result::is_ok);
        // Element IDs are local to a recipe. A different recipe replacing the same
        // file can legitimately reuse every spelling without sharing any identity.
        let old = bindings(old);
        let new = bindings(new);
        for key @ (_, id) in old
            .keys()
            .chain(new.keys())
            .copied()
            .collect::<BTreeSet<_>>()
        {
            let previous = old.get(&key);
            let next = new.get(&key);
            if previous.is_none() && know_old {
                row.added.push(id.to_owned());
            }
            if next.is_none() && know_new {
                row.removed.push(id.to_owned());
            }
            let anchors = previous
                .into_iter()
                .chain(next)
                .flatten()
                .copied()
                .collect::<BTreeSet<_>>();
            let mut touched = false;
            let mut base_touched = false;
            for anchor in anchors {
                let was = previous.is_some_and(|set| set.contains(anchor));
                let now = next.is_some_and(|set| set.contains(anchor));
                // A rebind is itself an evidence edit, even with identical source blobs.
                let rebind = previous.is_some() && next.is_some() && was != now;
                let changed = rebind || review.touched(anchor, was, now)?;
                touched |= changed;
                base_touched |= was && changed;
            }
            if touched && next.is_some() {
                row.touched.push(id.to_owned());
            }
            if base_touched {
                row.touched_base.push(id.to_owned());
            }
        }
        if !row.touched.is_empty()
            || !row.added.is_empty()
            || !row.removed.is_empty()
            || !row.touched_base.is_empty()
        {
            result.push(row);
        }
    }
    Ok(result)
}

fn bindings(recipe: Option<&RepoRecipe>) -> BTreeMap<(&str, &str), BTreeSet<&EvidenceAnchor>> {
    let mut by_id = BTreeMap::new();
    if let Some(recipe) = recipe {
        by_id.extend(ids(recipe).map(|id| ((recipe.id(), id), BTreeSet::new())));
        for binding in &recipe.evidence {
            by_id
                .entry((recipe.id(), &binding.element))
                .or_default()
                .insert(&binding.anchor);
        }
    }
    by_id
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
        entry_paths: &BTreeSet<&str>,
    ) -> Result<Self> {
        // A newly added/removed lens may reference a path the other snapshot
        // didn't need for checks. Read those counterparts to compare safely.
        let paths: BTreeSet<_> = other
            .requested
            .difference(&lenses.requested)
            .map(String::as_str)
            .chain(
                entry_paths
                    .iter()
                    .copied()
                    .filter(|p| !lenses.requested.contains(*p)),
            )
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

    fn resolve(&self, anchor: &EvidenceAnchor) -> Option<(String, Range<u64>)> {
        anchor::resolve(anchor, self.tree, None, |path| self.source(path))
            .ok()
            .map(|(path, span, _)| (path, span))
    }
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
    fn touched(&mut self, anchor: &EvidenceAnchor, was: bool, now: bool) -> Result<bool> {
        let old = was.then(|| self.before.resolve(anchor)).flatten();
        let new = now.then(|| self.after.resolve(anchor)).flatten();
        let path = match anchor {
            EvidenceAnchor::Path { path }
            | EvidenceAnchor::Text { path, .. }
            | EvidenceAnchor::Symbol { path, .. } => Some(path),
            EvidenceAnchor::Entry { .. } => old.as_ref().or(new.as_ref()).map(|r| &r.0),
        };
        if let Some(path) = path {
            let left = self.before.source(path);
            let right = self.after.source(path);
            ensure!(
                !(left.is_some_and(|s| s.limited) || right.is_some_and(|s| s.limited))
                    || left.and_then(|s| s.blob.as_ref()) == right.and_then(|s| s.blob.as_ref()),
                "could not diff anchor source {path:?} beyond the scan budget"
            );
        }
        if was && now && old.as_ref().map(|r| &r.0) != new.as_ref().map(|r| &r.0) {
            return Ok(true);
        }
        if let Some((path, span)) = old
            && self.hunks(&path)?.iter().any(|h| {
                overlaps(&span, &h.old)
                    || h.old.is_empty() && span.start < h.old.start && h.old.start < span.end
            })
        {
            return Ok(true);
        }
        if let Some((path, span)) = new
            && self.hunks(&path)?.iter().any(|h| overlaps(&span, &h.new))
        {
            return Ok(true);
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
