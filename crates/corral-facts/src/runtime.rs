//! Literal declarations and links, using only the tree already read by the map.

mod config;
mod packages;
mod yaml;

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound::{Included, Unbounded};

use crate::model::*;
use crate::structure::owner;

#[derive(Default)]
struct Declarations {
    units: Vec<RuntimeUnit>,
    runs: Vec<(RuntimeUnitId, RuntimeSite, String)>,
    depends_on: Vec<(RuntimeUnitId, RuntimeSite, String)>,
    builds: Vec<(RuntimeUnitId, RuntimeSite, String, Option<String>)>,
    entries: BTreeMap<String, Vec<RuntimeUnitId>>,
}

pub(crate) fn is_config(path: &str) -> bool {
    config::recognizes(path)
}

pub(crate) struct Configs<'a> {
    pub(crate) parsed: BTreeSet<&'a str>,
    declarations: Declarations,
}

impl<'a> Configs<'a> {
    pub(crate) fn read(texts: &'a BTreeMap<String, String>, containers: &BTreeSet<&str>) -> Self {
        let mut declarations = Declarations::default();
        let parsed = texts
            .iter()
            .filter_map(|(path, text)| {
                config::read(path, text, owner(path, containers), &mut declarations)
                    .then_some(path.as_str())
            })
            .collect();
        Self {
            parsed,
            declarations,
        }
    }
}

pub(crate) struct Runtime {
    facts: RuntimeFacts,
    binaries: BTreeMap<String, Vec<RuntimeUnitId>>,
    entries: BTreeMap<String, Vec<RuntimeUnitId>>,
}

impl Runtime {
    pub(crate) fn collect(
        manifests: &BTreeMap<String, String>,
        configs: Configs<'_>,
        files: &BTreeMap<&str, &FileFact>,
        containers: &BTreeSet<&str>,
    ) -> Self {
        let mut output = configs.declarations;
        packages::read(manifests, files, containers, &mut output);
        output.units.sort_by(|a, b| a.id.cmp(&b.id));
        let mut binaries = BTreeMap::<_, Vec<_>>::new();
        let mut units = BTreeSet::new();
        for unit in &output.units {
            units.insert(&unit.id);
            if unit.id.kind == RuntimeUnitKind::Binary {
                binaries
                    .entry(unit.id.name.clone())
                    .or_default()
                    .push(unit.id.clone());
            }
        }
        let mut edges = Vec::new();
        // These are literal name matches, not a prediction of which executable PATH selects.
        for (from, site, program) in output.runs {
            if let Some(targets) = binaries.get(&program) {
                let from = unit_source(from);
                edges.extend(targets.iter().map(|to| {
                    declared(
                        RuntimeEdgeKind::Runs,
                        from.clone(),
                        to.clone(),
                        site.clone(),
                    )
                }));
            }
        }
        for (from, site, name) in output.depends_on {
            let to = unit_id(RuntimeUnitKind::Service, &name, &from.path);
            if units.contains(&to) {
                edges.push(declared(
                    RuntimeEdgeKind::DependsOn,
                    unit_source(from),
                    to,
                    site,
                ));
            }
        }
        for (to, site, context, dockerfile) in output.builds {
            let Some(context) = local_path(dirname(&site.path), &context) else {
                continue;
            };
            let dockerfile = local_path(&context, dockerfile.as_deref().unwrap_or("Dockerfile"));
            let from = dockerfile
                .as_deref()
                .filter(|path| files.contains_key(path))
                .map(|path| {
                    let image = unit_id(RuntimeUnitKind::Image, basename(path), path);
                    if units.contains(&image) {
                        unit_source(image)
                    } else {
                        container_source(owner(path, containers))
                    }
                })
                .or_else(|| {
                    tree_directory(files, &context)
                        .then(|| container_source(owner(&format!("{context}/."), containers)))
                });
            if let Some(from) = from {
                edges.push(declared(RuntimeEdgeKind::Builds, from, to, site));
            }
        }
        Self {
            facts: RuntimeFacts {
                units: output.units,
                edges,
            },
            binaries,
            entries: output.entries,
        }
    }

    /// Returns false when no binary declaration matches (an external process).
    pub(crate) fn spawn(&mut self, from: &FileFact, program: &str, line: u64) -> bool {
        let Some(targets) = self.binaries.get(program) else {
            return false;
        };
        let source = match self.entries.get(&from.path).map(Vec::as_slice) {
            Some([id]) => unit_source(id.clone()),
            _ => container_source(&from.container),
        };
        let site = RuntimeSite {
            path: from.path.clone(),
            line,
        };
        self.facts
            .edges
            .extend(targets.iter().map(|to| RuntimeEdge {
                kind: RuntimeEdgeKind::Starts,
                from: source.clone(),
                to: to.clone(),
                basis: RuntimeBasis::Syntax,
                site: site.clone(),
            }));
        true
    }

    pub(crate) fn finish(mut self) -> RuntimeFacts {
        self.facts.edges.sort_unstable();
        self.facts
    }
}

fn declared(
    kind: RuntimeEdgeKind,
    from: RuntimeSource,
    to: RuntimeUnitId,
    site: RuntimeSite,
) -> RuntimeEdge {
    RuntimeEdge {
        kind,
        from,
        to,
        site,
        basis: RuntimeBasis::Declared,
    }
}

fn unit_source(id: RuntimeUnitId) -> RuntimeSource {
    RuntimeSource::Unit { id }
}

fn container_source(path: &str) -> RuntimeSource {
    RuntimeSource::Container {
        path: path.to_owned(),
    }
}

fn unit_id(kind: RuntimeUnitKind, name: &str, path: &str) -> RuntimeUnitId {
    RuntimeUnitId {
        kind,
        name: name.to_owned(),
        path: path.to_owned(),
    }
}

fn unit(kind: RuntimeUnitKind, name: &str, path: &str, line: u64, container: &str) -> RuntimeUnit {
    RuntimeUnit {
        id: unit_id(kind, name, path),
        line,
        container: container.to_owned(),
        command: None,
        entry_point: None,
        image: None,
        build_context: None,
        dockerfile: None,
        ports: Vec::new(),
        depends_on: Vec::new(),
    }
}

fn tree_directory(files: &BTreeMap<&str, &FileFact>, path: &str) -> bool {
    if path.is_empty() {
        return true;
    }
    let prefix = format!("{path}/");
    files
        .range::<str, _>((Included(prefix.as_str()), Unbounded))
        .next()
        .is_some_and(|(file, _)| file.starts_with(&prefix))
}

fn dirname(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// A config path may refer to something outside Git or be interpolated at runtime.
fn local_path(base: &str, path: &str) -> Option<String> {
    if path.starts_with('~') || path.contains([':', '\\', '$']) {
        return None;
    }
    literal_path(base, path)
}

/// Normalize an uninterpolated path within the committed tree without following links.
fn literal_path(base: &str, path: &str) -> Option<String> {
    if path.is_empty() || path.starts_with('/') {
        return None;
    }
    let mut parts: Vec<_> = base.split('/').filter(|part| !part.is_empty()).collect();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}
