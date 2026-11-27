//! Behaviour lens facts from immutable Git objects, independent of the checkout.

mod anchor;
mod review;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use anyhow::{Context, Result, ensure};
use corral_types::lenses::{
    BoundSpan, EvidenceAnchor, InboundReference, LensDiagnostic, LensLocation, LensResult,
    LensStatus, RepoRecipe,
};
use heck::ToSnakeCase;

use crate::{ProviderCoverage, Revision, TreeFacts, git};
use anchor::{Source, SourceBudget};
pub use review::impact;

const LENS_FILE_COUNT: usize = 1024;
const LENS_PATH_BYTES: usize = 1024 * 1024;
const LENS_FILE_BYTES: usize = 256 * 1024;
const LENS_TOTAL_BYTES: usize = 4 * 1024 * 1024;

/// Committed lens files and bounded source blobs relevant to their anchors.
pub struct LensSources {
    tree: String,
    lenses: BTreeMap<String, Result<RepoRecipe, String>>,
    requested: BTreeSet<String>,
    sources: BTreeMap<String, Source>,
    files_limited: bool,
}

impl LensSources {
    pub fn is_empty(&self) -> bool {
        self.lenses.is_empty()
    }
}

/// Require a portable, canonical repository-relative path, not a Git pathspec.
pub fn valid_path(path: &str) -> bool {
    !path.contains(['\\', ':'])
        && !path.chars().any(char::is_control)
        && path.split('/').all(|p| !matches!(p, "" | "." | ".."))
}

/// Read lens files and their source blobs at the supplied commit. File format
/// errors are retained by path so one malformed file cannot hide its siblings.
pub fn load(repo: &Path, rev: &Revision, dir: &str) -> Result<LensSources> {
    ensure!(
        valid_path(dir),
        "invalid repository-relative lens directory {dir:?}"
    );
    let prefix = format!("{dir}/");
    let mut lenses = BTreeMap::new();
    let mut remaining = LENS_TOTAL_BYTES;
    let mut path_bytes = 0;
    let mut files_limited = false;
    git::tree_selected(
        repo,
        rev.tree(),
        |path| {
            path.strip_prefix(&prefix)
                .is_some_and(|file| !file.contains('/') && file.ends_with(".json"))
        },
        |blob| {
            path_bytes += blob.path.len();
            ensure!(
                lenses.len() < LENS_FILE_COUNT && path_bytes <= LENS_PATH_BYTES,
                "lens directory {dir:?} exceeded the file count or path budget"
            );
            let lens = (|| {
                let over_budget = blob.text.map_or(blob.lines > 0, |text| {
                    text.len() > LENS_FILE_BYTES || text.len() > remaining
                });
                if over_budget {
                    files_limited = true;
                    anyhow::bail!("lens file exceeded the lens scan budget");
                }
                let text = blob.text.context("expected a regular UTF-8 JSON file")?;
                remaining -= text.len();
                RepoRecipe::from_json(text).map_err(anyhow::Error::msg)
            })()
            .map_err(|error| format!("{error:#}"));
            lenses.insert(blob.path.to_owned(), lens);
            Ok(())
        },
    )?;
    let mut ids = BTreeMap::<String, usize>::new();
    for lens in lenses.values().filter_map(|row| row.as_ref().ok()) {
        *ids.entry(lens.id().to_owned()).or_default() += 1;
    }
    for row in lenses.values_mut() {
        if let Ok(lens) = row
            && ids.get(lens.id()).is_some_and(|&count| count > 1)
        {
            *row = Err(format!("duplicate lens ID {:?}", lens.id()));
        }
    }
    let mut requests = BTreeMap::<&str, BTreeSet<&str>>::new();
    for lens in lenses.values().filter_map(|row| row.as_ref().ok()) {
        if let Some(check) = &lens.coverage {
            requests.entry(&check.path).or_default().insert(&check.name);
        }
        for binding in &lens.evidence {
            match &binding.anchor {
                EvidenceAnchor::Symbol { path, name } => {
                    let names = requests.entry(path).or_default();
                    names.insert(name);
                    names.insert(name.split("::").next().unwrap_or(name));
                }
                EvidenceAnchor::Path { path } | EvidenceAnchor::Text { path, .. } => {
                    requests.entry(path).or_default();
                }
                EvidenceAnchor::Entry { .. } => {}
            }
        }
    }
    let mut sources = BTreeMap::new();
    if !requests.is_empty() {
        let mut budget = SourceBudget::default();
        git::tree_selected(
            repo,
            rev.tree(),
            |path| requests.contains_key(path),
            |blob| {
                sources.insert(
                    blob.path.to_owned(),
                    budget.read(&blob, &requests[blob.path]),
                );
                Ok(())
            },
        )?;
    }
    let requested = requests.into_keys().map(str::to_owned).collect();
    Ok(LensSources {
        tree: rev.tree().to_owned(),
        lenses,
        requested,
        sources,
        files_limited,
    })
}

fn ids(lens: &RepoRecipe) -> impl Iterator<Item = &str> {
    lens.nodes
        .iter()
        .map(|n| n.id.as_str())
        .chain(lens.edges.iter().map(|e| e.id.as_str()))
        .chain(lens.groups.iter().map(|g| g.id.as_str()))
}

/// Coverage accompanying provider-overlaid structural facts.
#[derive(Clone, Copy)]
pub struct LensCoverage<'a> {
    pub provider: &'a ProviderCoverage,
    /// Only explicit `file` rows; a built-in read is not a provider answer.
    pub unread_files: &'a BTreeSet<String>,
}

impl LensCoverage<'_> {
    fn has_omissions(self) -> bool {
        // Counts are reply-wide, not attributed to files or categories.
        self.provider.dropped_unknown_files > 0
            || self
                .provider
                .skipped
                .values()
                .chain(self.provider.omitted.values())
                .any(|&count| count > 0)
    }
}

/// Check all lenses and report separately what failed and what could not run.
/// Pass coverage alongside any merged provider facts so a partial answer does
/// not turn unseen definitions or entry points into stale lenses.
pub fn check(
    sources: &LensSources,
    tree: &TreeFacts,
    coverage: Option<LensCoverage<'_>>,
) -> Vec<LensResult> {
    // Several lenses can share one enum; don't replicate its variants without bound.
    let mut variant_budget = LENS_TOTAL_BYTES;
    let mut inbound = BTreeMap::<&str, Vec<_>>::new();
    for row in &tree.file_references {
        inbound.entry(&row.to).or_default().push(row);
    }
    let mut budget = EvidenceBudget {
        hash_bytes: 16 * 1024 * 1024,
        references: LENS_TOTAL_BYTES,
    };
    sources
        .lenses
        .iter()
        .map(|(file, lens)| {
            let mut result = LensResult {
                file: file.clone(),
                lens: lens.as_ref().ok().cloned(),
                status: LensStatus::Malformed,
                derived: Vec::new(),
                failures: Vec::new(),
                not_checked: Vec::new(),
                locations: Vec::new(),
                missing_states: Vec::new(),
                extra_states: Vec::new(),
            };
            match lens {
                Ok(lens) => {
                    check_evidence(
                        &mut result,
                        lens,
                        sources,
                        tree,
                        coverage,
                        &inbound,
                        &mut budget,
                    );
                    check_coverage(&mut result, lens, sources, &mut variant_budget);
                    result.status = if !result.failures.is_empty() {
                        LensStatus::Stale
                    } else if !result.missing_states.is_empty() || !result.extra_states.is_empty() {
                        LensStatus::Incomplete
                    } else {
                        LensStatus::Current
                    };
                }
                Err(error) => note(&mut result.failures, "$file", error),
            }
            result
        })
        .collect()
}

struct EvidenceBudget {
    hash_bytes: usize,
    references: usize,
}

fn check_evidence(
    result: &mut LensResult,
    lens: &RepoRecipe,
    sources: &LensSources,
    tree: &TreeFacts,
    coverage: Option<LensCoverage<'_>>,
    inbound: &BTreeMap<&str, Vec<&crate::Reference<String>>>,
    budget: &mut EvidenceBudget,
) {
    let mut per_recipe = 8 * 1024;
    let mut bound = BTreeSet::new();
    let mut derived = BTreeSet::new();
    for (index, binding) in lens.evidence.iter().enumerate() {
        let id = &binding.element;
        bound.insert(id.as_str());
        match anchor::resolve(&binding.anchor, tree, coverage, |path| {
            sources.sources.get(path)
        }) {
            Ok((path, span, basis)) => {
                let digest = sources
                    .sources
                    .get(&path)
                    .and_then(|source| match binding.anchor {
                        EvidenceAnchor::Path { .. } => source.blob.clone(),
                        _ => source
                            .text
                            .as_ref()?
                            .digest(span.clone(), &mut budget.hash_bytes),
                    });
                result.locations.push(LensLocation {
                    element: id.clone(),
                    path: path.clone(),
                    line: span.start,
                    span: Some(BoundSpan {
                        binding: index,
                        end_line: span.end - 1,
                        basis,
                        digest,
                    }),
                });
                if derived.insert((id, path.clone()))
                    && let Some(refs) = inbound.get(path.as_str())
                {
                    for &reference in refs {
                        let bytes = reference.from.len() + reference.to.len() + id.len() + 128;
                        if result.derived.len() >= 64
                            || bytes > per_recipe
                            || bytes > budget.references
                        {
                            note(
                                &mut result.not_checked,
                                id,
                                "additional inbound file references omitted: evidence budget",
                            );
                            break;
                        }
                        budget.references -= bytes;
                        per_recipe -= bytes;
                        result.derived.push(InboundReference {
                            element: id.clone(),
                            reference: reference.clone(),
                        });
                    }
                }
            }
            Err(anchor::Issue::Failed(reason)) => note(&mut result.failures, id, reason),
            Err(anchor::Issue::Unchecked(reason)) => note(&mut result.not_checked, id, reason),
        }
    }
    for id in ids(lens).filter(|id| !bound.contains(id)) {
        note(&mut result.not_checked, id, "no evidence binding");
    }
}

fn check_coverage(
    result: &mut LensResult,
    lens: &RepoRecipe,
    sources: &LensSources,
    variant_budget: &mut usize,
) {
    if let Some(c) = &lens.coverage {
        if let Some(found) = sources
            .sources
            .get(&c.path)
            .and_then(|s| s.enums.get(&c.name))
        {
            if let Some(remaining) = variant_budget.checked_sub(found.variant_bytes) {
                *variant_budget = remaining;
                compare_variants(
                    result,
                    found.variants.keys().map(String::as_str),
                    c.nodes.iter().map(String::as_str),
                );
            } else {
                note(
                    &mut result.not_checked,
                    "$coverage",
                    "enum variants exceeded result budget",
                );
            }
        } else {
            note(
                &mut result.not_checked,
                "$coverage",
                "enum variants unavailable or ambiguous at this revision",
            );
        }
    }
}

fn note(issues: &mut Vec<LensDiagnostic>, element: &str, message: impl Into<String>) {
    issues.push(LensDiagnostic {
        element: element.to_owned(),
        message: message.into(),
    });
}

fn compare_variants<'a>(
    result: &mut LensResult,
    variants: impl Iterator<Item = &'a str>,
    states: impl Iterator<Item = &'a str>,
) {
    let mut missing: BTreeSet<_> = variants.collect();
    let unmatched: Vec<_> = states.filter(|state| !missing.remove(state)).collect();
    let mut aliases = BTreeMap::new();
    for &variant in &missing {
        aliases
            .entry(variant.to_snake_case())
            .and_modify(|entry| *entry = None)
            .or_insert(Some(variant));
    }
    for state in unmatched {
        if let Some(Some(variant)) = aliases.get(state) {
            missing.remove(variant);
        } else {
            result.extra_states.push(state.to_owned());
        }
    }
    result.missing_states = missing.into_iter().map(str::to_owned).collect();
    result.extra_states.sort_unstable();
}
