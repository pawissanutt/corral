//! Conventional source layout only: follow committed `mod` declarations from
//! `src/lib.rs` and `src/main.rs`. Never resolve a qualified path by leaf name.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{DataTypeId, RuntimeFacts, RuntimeUnitKind, manifests};

use super::qualified;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum Anchor {
    Crate,
    Relative,
    Super(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct ModulePath {
    pub anchor: Anchor,
    pub name: String,
}

#[derive(Default)]
pub(crate) struct Layout<'a> {
    // A physical file can be declared by both the library and binary roots.
    scopes: BTreeMap<&'a str, Vec<(&'a str, String)>>,
    mounted: BTreeMap<&'a str, BTreeMap<String, &'a str>>,
}

impl<'a> Layout<'a> {
    pub(crate) fn collect(
        files: impl Iterator<Item = &'a str>,
        modules: &BTreeMap<String, Vec<String>>,
        manifests: &BTreeMap<String, String>,
        runtime: &RuntimeFacts,
    ) -> Self {
        let mut known_cargo = BTreeSet::new();
        let files: BTreeSet<_> = files
            .filter(|path| {
                if path.rsplit('/').next() == Some("Cargo.toml") {
                    known_cargo.insert(*path);
                }
                path.ends_with(".rs") && path.len() <= super::super::MAX_PATH_BYTES
            })
            .collect();
        let candidates: Vec<_> = files.iter().copied().filter_map(crate_root).collect();
        let candidate_dirs: BTreeSet<_> = candidates.iter().map(|&(_, dir)| dir).collect();
        let cargo_bins = CargoBins::new(runtime);
        let roots: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|&(path, dir)| {
                let package = dir.strip_suffix("/src").unwrap_or("");
                let manifest = if package.is_empty() {
                    "Cargo.toml".to_owned()
                } else {
                    format!("{package}/Cargo.toml")
                };
                if known_cargo.contains(manifest.as_str()) {
                    if path.ends_with("/lib.rs") {
                        manifests
                            .get(&manifest)
                            .is_some_and(|text| conventional_lib(path, package, text))
                    } else {
                        cargo_bins.implicit.contains(path)
                            || cargo_bins.declared.contains(&(manifest.as_str(), path))
                    }
                } else {
                    // A nested `src/lib.rs` can be just another ordinary module.
                    // Infer a new crate inside a source tree only with its own manifest.
                    owning_root(dir, &candidate_dirs).is_none()
                }
            })
            .collect();
        let root_dirs: BTreeSet<_> = roots.iter().map(|&(_, dir)| dir).collect();
        let mut layout = Self::default();
        for (root, dir) in roots {
            let mounted = layout.mounted.entry(root).or_default();
            let mut pending = vec![(String::new(), root)];
            while let Some((prefix, path)) = pending.pop() {
                if mounted.contains_key(&prefix) {
                    continue;
                }
                mounted.insert(prefix.clone(), path);
                layout
                    .scopes
                    .entry(path)
                    .or_default()
                    .push((root, prefix.clone()));
                for child in modules.get(path).into_iter().flatten() {
                    let name = qualified(&prefix, child);
                    let location = format!("{dir}/{}", name.replace("::", "/"));
                    let flat = files.get(format!("{location}.rs").as_str());
                    let nested = files.get(format!("{location}/mod.rs").as_str());
                    if let (Some(&file), None) | (None, Some(&file)) = (flat, nested)
                        && owning_root(file, &root_dirs) == Some(dir)
                    {
                        pending.push((name, file));
                    }
                }
            }
        }
        layout
    }

    pub(crate) fn resolve(&self, from: &DataTypeId, path: &ModulePath) -> Option<DataTypeId> {
        let inline = from.name.rsplit_once("::").map_or("", |(module, _)| module);
        let Some(scopes) = self.scopes.get(from.path.as_str()) else {
            // We can still resolve relative paths within a detached file's
            // inline modules, but we cannot infer its crate root.
            return Some(DataTypeId {
                path: from.path.clone(),
                name: scoped_name(inline, path, false)?,
            });
        };
        let mut scopes = scopes.iter();
        let (root, prefix) = scopes.next()?;
        let target = self.in_crate(root, prefix, inline, path)?;
        scopes
            .all(|(root, prefix)| {
                self.in_crate(root, prefix, inline, path).as_ref() == Some(&target)
            })
            .then_some(target)
    }

    fn in_crate(
        &self,
        root: &str,
        prefix: &str,
        inline: &str,
        path: &ModulePath,
    ) -> Option<DataTypeId> {
        let scope = if inline.is_empty() {
            prefix.to_owned()
        } else {
            qualified(prefix, inline)
        };
        let name = scoped_name(&scope, path, true)?;
        let mounted = self.mounted.get(root)?;
        let mut current = name.as_str();
        while let Some((parent, _)) = current.rsplit_once("::") {
            if let Some(&file) = mounted.get(parent) {
                return Some(DataTypeId {
                    path: file.to_owned(),
                    name: name[parent.len() + 2..].to_owned(),
                });
            }
            current = parent;
        }
        Some(DataTypeId {
            path: root.to_owned(),
            name,
        })
    }
}

fn scoped_name(mut scope: &str, path: &ModulePath, rooted: bool) -> Option<String> {
    match path.anchor {
        Anchor::Crate => return rooted.then(|| path.name.clone()),
        Anchor::Relative => {}
        Anchor::Super(levels) => {
            for _ in 0..levels {
                if scope.is_empty() {
                    return None;
                }
                scope = scope.rsplit_once("::").map_or("", |(parent, _)| parent);
            }
        }
    }
    Some(qualified(scope, &path.name))
}

fn crate_root(path: &str) -> Option<(&str, &str)> {
    let (dir, basename) = path.rsplit_once('/')?;
    (dir.rsplit('/').next() == Some("src") && matches!(basename, "lib.rs" | "main.rs"))
        .then_some((path, dir))
}

fn conventional_lib(path: &str, package_dir: &str, manifest: &str) -> bool {
    let Ok(table) = manifest.parse::<toml::Table>() else {
        return false;
    };
    let Some(package) = table.get("package").and_then(toml::Value::as_table) else {
        return false;
    };
    if let Some(lib) = table.get("lib") {
        return lib.as_table().is_some_and(|lib| {
            lib.get("path").is_none_or(|value| {
                value
                    .as_str()
                    .and_then(|name| manifests::local_path(package_dir, name))
                    .as_deref()
                    == Some(path)
            })
        });
    }
    package.get("autolib").and_then(toml::Value::as_bool) != Some(false)
}

#[derive(Default)]
struct CargoBins<'a> {
    implicit: BTreeSet<&'a str>,
    declared: BTreeSet<(&'a str, &'a str)>,
}

impl<'a> CargoBins<'a> {
    fn new(runtime: &'a RuntimeFacts) -> Self {
        let mut bins = Self::default();
        for unit in runtime
            .units
            .iter()
            .filter(|unit| unit.id.kind == RuntimeUnitKind::Binary)
        {
            let path = unit.id.path.as_str();
            match path.rsplit('/').next() {
                Some("main.rs") => {
                    bins.implicit.insert(path);
                }
                Some("Cargo.toml") => {
                    if let Some(entry) = unit.entry_point.as_deref() {
                        bins.declared.insert((path, entry));
                    }
                }
                _ => {}
            }
        }
        bins
    }
}

fn owning_root<'a>(mut path: &'a str, dirs: &BTreeSet<&str>) -> Option<&'a str> {
    while let Some((parent, _)) = path.rsplit_once('/') {
        if dirs.contains(parent) {
            return Some(parent);
        }
        path = parent;
    }
    None
}
