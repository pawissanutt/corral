//! Package runtime targets and their declaration anchors.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound::{Included, Unbounded};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;
use toml_edit::{Document, Item};

use crate::model::*;

use super::{Declarations, basename, dirname, local_path, unit};

pub(super) fn read(
    path: &str,
    text: &str,
    files: &BTreeMap<&str, &FileFact>,
    container: &str,
    output: &mut Declarations,
) {
    match basename(path) {
        "Cargo.toml" => cargo(path, text, files, container, output),
        "package.json" => npm(path, text, container, output),
        "pyproject.toml" => python(path, text, container, output),
        _ => {}
    }
}

fn cargo(
    path: &str,
    text: &str,
    files: &BTreeMap<&str, &FileFact>,
    container: &str,
    output: &mut Declarations,
) {
    let Ok(doc) = Document::parse(text) else {
        return;
    };
    let Some(package) = doc.get("package").and_then(Item::as_table_like) else {
        return;
    };
    let Some(package_name) = package.get("name").and_then(Item::as_str) else {
        return;
    };
    let dir = dirname(path);
    let main = join(dir, "src/main.rs");
    let mut candidates = BTreeMap::new();
    if files.contains_key(main.as_str()) {
        candidates.insert(package_name, main);
    }
    let bin_dir = join(dir, "src/bin/");
    for (&file, _) in files.range::<str, _>((Included(bin_dir.as_str()), Unbounded)) {
        let Some(rel) = file.strip_prefix(&bin_dir) else {
            break;
        };
        let Some(name) = rel
            .strip_suffix("/main.rs")
            .or_else(|| rel.strip_suffix(".rs"))
        else {
            continue;
        };
        if !name.is_empty() && !name.contains('/') {
            candidates.entry(name).or_insert_with(|| file.to_owned());
        }
    }
    let lines = Lines::new(text);
    let mut declared_names = BTreeSet::new();
    let mut declared_paths = BTreeSet::new();
    if let Some(bins) = doc.get("bin").and_then(Item::as_array_of_tables) {
        for bin in bins {
            let Some(name) = bin.get("name").and_then(Item::as_str) else {
                continue;
            };
            if !declared_names.insert(name) {
                continue;
            }
            let Some(span) = bin.span() else { continue };
            let entry = match bin.get("path").and_then(Item::as_str) {
                Some(p) => local_path(dir, p),
                None => candidates.get(name).cloned(),
            };
            if let Some(entry) = &entry {
                declared_paths.insert(entry.clone());
            }
            let mut fact = unit(
                RuntimeUnitKind::Binary,
                name,
                path,
                lines.at(span.start),
                container,
            );
            fact.entry_point = entry.clone();
            binary(fact, entry, output);
        }
    }
    // Edition 2015 (including the implicit edition) disables auto-discovery
    // for kinds that already have an explicit target. Later editions keep it on.
    let legacy = package
        .get("edition")
        .is_none_or(|edition| edition.as_str() == Some("2015"));
    let autobins = package
        .get("autobins")
        .and_then(Item::as_bool)
        .unwrap_or(!legacy || declared_names.is_empty());
    if autobins {
        for (name, entry) in candidates {
            if !declared_names.contains(name) && !declared_paths.contains(&entry) {
                binary(
                    unit(RuntimeUnitKind::Binary, name, &entry, 1, container),
                    Some(entry),
                    output,
                );
            }
        }
    }
}

fn npm(path: &str, text: &str, container: &str, output: &mut Declarations) {
    let Ok(doc) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let keys = json_keys(text);
    let get_line = |group, name: &str| keys.get(&group)?.get(name).copied();
    if let Some(target) = doc.get("bin").and_then(Value::as_str)
        && let Some(name) = doc.get("name").and_then(Value::as_str)
        && let Some(line) = get_line(None, "bin")
    {
        npm_binary(path, basename(name), target, line, container, output);
    }
    for (group, kind) in [
        ("bin", RuntimeUnitKind::Binary),
        ("scripts", RuntimeUnitKind::Script),
    ] {
        let Some(entries) = doc.get(group).and_then(Value::as_object) else {
            continue;
        };
        for (name, value) in entries {
            let Some(value) = value.as_str() else {
                continue;
            };
            let Some(line) = get_line(Some(group), name) else {
                continue;
            };
            if kind == RuntimeUnitKind::Binary {
                npm_binary(path, name, value, line, container, output);
            } else {
                let mut script = unit(kind, name, path, line, container);
                script.command = Some(value.to_owned());
                output.units.push(script);
            }
        }
    }
}

fn npm_binary(
    path: &str,
    name: &str,
    target: &str,
    line: u64,
    container: &str,
    output: &mut Declarations,
) {
    let mut fact = unit(RuntimeUnitKind::Binary, name, path, line, container);
    fact.command = Some(target.to_owned());
    binary(fact, local_path(dirname(path), target), output);
}

fn python(path: &str, text: &str, container: &str, output: &mut Declarations) {
    let Ok(doc) = Document::parse(text) else {
        return;
    };
    let Some(scripts) = doc
        .get("project")
        .and_then(Item::as_table_like)
        .and_then(|p| p.get("scripts"))
        .and_then(Item::as_table_like)
    else {
        return;
    };
    let lines = Lines::new(text);
    for (name, value) in scripts.iter() {
        let Some(command) = value.as_str() else {
            continue;
        };
        let Some(span) = scripts.get_key_value(name).and_then(|(key, _)| key.span()) else {
            continue;
        };
        let mut fact = unit(
            RuntimeUnitKind::Binary,
            name,
            path,
            lines.at(span.start),
            container,
        );
        fact.command = Some(command.to_owned());
        output.units.push(fact);
    }
}

fn binary(fact: RuntimeUnit, entry: Option<String>, output: &mut Declarations) {
    if let Some(entry) = entry {
        output
            .entries
            .entry(entry)
            .or_default()
            .push(fact.id.clone());
    }
    output.units.push(fact);
}

fn join(dir: &str, path: &str) -> String {
    if dir.is_empty() {
        path.to_owned()
    } else {
        format!("{dir}/{path}")
    }
}

struct Lines(Vec<usize>);

impl Lines {
    fn new(text: &str) -> Self {
        Self(text.match_indices('\n').map(|(i, _)| i).collect())
    }

    fn at(&self, offset: usize) -> u64 {
        1 + self.0.partition_point(|&i| i < offset) as u64
    }
}

// serde_json already validated this input. Index only root keys and the direct
// children of bin/scripts; braces in strings and unrelated nested keys are opaque.
fn json_keys(text: &str) -> BTreeMap<Option<&'static str>, BTreeMap<String, u64>> {
    static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#""(?:\\.|[^"\\])*"|[{}\[\]:]"#)
            .unwrap_or_else(|error| panic!("invalid package key pattern: {error}"))
    });
    let lines = Lines::new(text);
    let mut tokens = TOKEN.find_iter(text).peekable();
    let mut depth = 0;
    let mut group = None;
    let mut keys = BTreeMap::<_, BTreeMap<_, _>>::new();
    while let Some(token) = tokens.next() {
        match token.as_str() {
            "{" | "[" => depth += 1,
            "}" | "]" => depth -= 1,
            ":" => {}
            raw if (depth == 1 || (depth == 2 && group.is_some()))
                && tokens.peek().is_some_and(|next| next.as_str() == ":") =>
            {
                let Ok(key) = serde_json::from_str::<String>(raw) else {
                    continue;
                };
                let section = if depth == 1 {
                    group = match key.as_str() {
                        "bin" => Some("bin"),
                        "scripts" => Some("scripts"),
                        _ => None,
                    };
                    // Duplicate declarations are last-wins, just like the parsed manifest.
                    if group.is_some() {
                        keys.remove(&group);
                    }
                    None
                } else {
                    group
                };
                keys.entry(section)
                    .or_default()
                    .insert(key, lines.at(token.start()));
            }
            _ => {}
        }
    }
    keys
}
