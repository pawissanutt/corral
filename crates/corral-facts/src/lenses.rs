//! Behaviour lens facts from immutable Git objects, independent of the checkout.

mod anchor;
mod review;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use anyhow::{Context, Result, ensure};
use corral_types::lenses::{Lens, LensAnchor, LensDiagnostic, LensResult, LensStatus};
use heck::ToSnakeCase;

use crate::{EntryDetector, EntryKind, EntryPoints, ProviderCoverage, Revision, TreeFacts, git};
use anchor::{Source, SourceBudget};
pub use review::impact;

const LENS_FILE_COUNT: usize = 1024;
const LENS_PATH_BYTES: usize = 1024 * 1024;
const LENS_FILE_BYTES: usize = 256 * 1024;
const LENS_TOTAL_BYTES: usize = 4 * 1024 * 1024;

/// Committed lens files and bounded source blobs relevant to their anchors.
pub struct LensSources {
    tree: String,
    lenses: BTreeMap<String, Result<Lens, String>>,
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
        |path| path.starts_with(&prefix) && path.ends_with(".toml"),
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
                let text = blob.text.context("expected a regular UTF-8 TOML file")?;
                remaining -= text.len();
                validate(toml::from_str::<Lens>(text).context("invalid lens TOML")?)
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
        if let Lens::State { states_from, .. } = lens {
            requests
                .entry(&states_from.path)
                .or_default()
                .insert(&states_from.name);
        }
        for element in elements(lens) {
            let names = requests.entry(&element.anchor.path).or_default();
            if let Some(name) = &element.anchor.name {
                // Rust syntax supplies enum and enum-variant locations, not general definitions.
                names.insert(name.split("::").next().unwrap_or(name));
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

fn nonblank(s: &str) -> bool {
    !s.trim().is_empty()
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && !id.chars().any(|c| c.is_whitespace() || c.is_control())
}

fn validate(lens: Lens) -> Result<Lens> {
    ensure!(valid_id(lens.id()), "invalid lens ID {:?}", lens.id());
    ensure!(nonblank(lens.title()), "empty lens title");
    git::check_oid(lens.read_at()).context("invalid read_at commit")?;
    let nodes = match &lens {
        Lens::State {
            states_from,
            states,
            ..
        } => {
            ensure!(
                valid_path(&states_from.path),
                "invalid states_from path {:?}",
                states_from.path
            );
            ensure!(nonblank(&states_from.name), "empty states_from name");
            states
        }
        Lens::Sequence {
            entry,
            participants,
            ..
        } => {
            ensure!(
                entry.as_ref().is_none_or(|entry| nonblank(&entry.name)),
                "empty entry name"
            );
            participants
        }
    };
    let mut ids = BTreeSet::new();
    for node in nodes {
        ensure!(valid_id(&node.id), "invalid element ID {:?}", node.id);
        ensure!(
            ids.insert(node.id.as_str()),
            "duplicate element ID {:?}",
            node.id
        );
        ensure!(nonblank(&node.label), "element {:?}: empty label", node.id);
    }
    let mut edges = BTreeSet::new();
    for element in elements(&lens) {
        let Element {
            key,
            name,
            anchor,
            qualifier,
        } = element;
        if let ElementKey::Edge(kind, from, to, label) = key {
            ensure!(nonblank(label), "{name}: empty label");
            ensure!(ids.contains(from), "{name}: unknown from endpoint {from:?}");
            ensure!(ids.contains(to), "{name}: unknown to endpoint {to:?}");
            // A sequence can intentionally send the same message more than once.
            if kind == "transition" {
                ensure!(
                    edges.insert((key, qualifier, anchor)),
                    "{name}: duplicate transition"
                );
            }
        }
        ensure!(
            valid_path(&anchor.path),
            "{name}: invalid anchor path {:?}",
            anchor.path
        );
        ensure!(
            anchor.name.as_deref().is_none_or(nonblank),
            "{name}: empty anchor name"
        );
        ensure!(
            anchor.text.as_deref().is_none_or(nonblank),
            "{name}: empty anchor text"
        );
    }
    Ok(lens)
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ElementKey<'a> {
    Node(&'static str, &'a str),
    Edge(&'static str, &'a str, &'a str, &'a str),
}

struct Element<'a> {
    key: ElementKey<'a>,
    name: String,
    anchor: &'a LensAnchor,
    qualifier: Option<&'a str>,
}

fn elements<'a>(lens: &'a Lens) -> Vec<Element<'a>> {
    let mut result = Vec::new();
    let (kind, nodes) = match lens {
        Lens::State { states, .. } => ("state", states),
        Lens::Sequence { participants, .. } => ("participant", participants),
    };
    result.extend(nodes.iter().map(|node| Element {
        key: ElementKey::Node(kind, &node.id),
        name: format!("{kind} {}", node.id),
        anchor: &node.anchor,
        qualifier: None,
    }));
    let mut counts = BTreeMap::<ElementKey<'_>, usize>::new();
    let mut edge = |kind,
                    from: &'a str,
                    to: &'a str,
                    label: &'a str,
                    anchor: &'a LensAnchor,
                    qualifier: Option<&'a str>| {
        let key = ElementKey::Edge(kind, from, to, label);
        let count = counts.entry(key).or_default();
        *count += 1;
        let mut name = format!("{kind} {from:?} -> {to:?}: {label:?}");
        if *count > 1 {
            name.push_str(&format!(" (#{count})"));
        }
        result.push(Element {
            key,
            name,
            anchor,
            qualifier,
        });
    };
    match lens {
        Lens::State { transitions, .. } => {
            for row in transitions {
                edge(
                    "transition",
                    &row.from,
                    &row.to,
                    &row.label,
                    &row.anchor,
                    row.guard.as_deref(),
                );
            }
        }
        Lens::Sequence { messages, .. } => {
            for row in messages {
                edge(
                    "message",
                    &row.from,
                    &row.to,
                    &row.label,
                    &row.anchor,
                    row.note.as_deref(),
                );
            }
        }
    }
    result
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
        self.provider
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
    sources
        .lenses
        .iter()
        .map(|(file, lens)| {
            let mut result = LensResult {
                file: file.clone(),
                lens: lens.as_ref().ok().cloned(),
                status: LensStatus::Malformed,
                failures: Vec::new(),
                not_checked: Vec::new(),
                locations: Vec::new(),
                missing_states: Vec::new(),
                extra_states: Vec::new(),
            };
            match lens {
                Ok(lens) => {
                    for element in elements(lens) {
                        anchor::check(&mut result, &element, sources, tree, coverage);
                    }
                    match lens {
                        Lens::State {
                            states_from,
                            states,
                            ..
                        } => {
                            if let Some(source) = sources.sources.get(&states_from.path) {
                                if let Some(found) = source.enums.get(&states_from.name) {
                                    if let Some(remaining) = variant_budget.checked_sub(found.variant_bytes) {
                                        variant_budget = remaining;
                                        let wanted = found.variants.keys().map(String::as_str);
                                        let actual = states.iter().map(|node| node.id.as_str());
                                        compare_variants(&mut result, wanted, actual);
                                    } else {
                                        note(&mut result.not_checked, "states_from", "enum variants not checked: result budget exhausted");
                                    }
                                } else {
                                    note(
                                        &mut result.not_checked,
                                        "states_from",
                                        if source.limited {
                                            "enum variants not checked: source exceeded the lens scan budget"
                                        } else {
                                            "enum variants not available"
                                        },
                                    );
                                }
                                if let Some(line) = source.syntax_line(&states_from.name) {
                                    anchor::locate(
                                        &mut result,
                                        "states_from",
                                        &states_from.path,
                                        line,
                                    );
                                }
                            } else {
                                note(
                                    &mut result.failures,
                                    "states_from",
                                    format!("path {:?} does not exist", states_from.path),
                                );
                            }
                        }
                        Lens::Sequence {
                            entry: Some(entry), ..
                        } => {
                            check_entry(&mut result, entry.kind, &entry.name, &tree.entry_points, coverage);
                        }
                        Lens::Sequence { entry: None, .. } => {}
                    }
                    result.status = if !result.failures.is_empty() {
                        LensStatus::Stale
                    } else if !result.missing_states.is_empty() || !result.extra_states.is_empty() {
                        LensStatus::Incomplete
                    } else {
                        LensStatus::Current
                    };
                }
                Err(error) => note(&mut result.failures, "file", error),
            }
            result
        })
        .collect()
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

fn check_entry(
    result: &mut LensResult,
    kind: EntryKind,
    name: &str,
    points: &EntryPoints,
    coverage: Option<LensCoverage<'_>>,
) {
    if let Some(entry) = points
        .entries
        .iter()
        .find(|entry| entry.kind == kind && entry.name == name)
    {
        anchor::locate(result, "entry", &entry.path, entry.line);
        return;
    }
    if coverage
        .is_some_and(|coverage| coverage.provider.provides("entries") && coverage.has_omissions())
    {
        note(
            &mut result.not_checked,
            "entry",
            "entry not checked: provider answer had skipped or omitted facts",
        );
    } else if points.entries.iter().any(|entry| entry.kind == kind)
        || points
            .coverage
            .values()
            .flatten()
            .any(|detector| match detector {
                // An extension row establishes coverage only for its own kind.
                EntryDetector::Extension => false,
                EntryDetector::Clap | EntryDetector::NpmBin => kind == EntryKind::Command,
                EntryDetector::Axum | EntryDetector::Express => kind == EntryKind::Route,
                EntryDetector::OperationEnum => kind == EntryKind::Operation,
            })
    {
        note(
            &mut result.failures,
            "entry",
            format!("unknown {kind:?} entry point {name:?}"),
        );
    } else {
        note(
            &mut result.not_checked,
            "entry",
            "entry points not provided for this kind",
        );
    }
}
