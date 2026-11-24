//! Data facts use declared schemas and source syntax, not compiler or serializer output.
mod json;
mod rust;
mod sql;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    DataEdge, DataEdgeKind, DataKind, DataSource, DataType, DataTypeId, ReadStatus, RuntimeFacts,
};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataFacts {
    pub types: Vec<DataType>,
    pub edges: Vec<DataEdge>,
    /// Languages observed in the tree. An empty set means no data source supplied facts.
    pub coverage: BTreeMap<String, BTreeSet<DataSource>>,
    sources: BTreeMap<String, DataSource>,
    links: Vec<Link>,
    schema_anchors: Vec<SchemaAnchor>,
    pub(crate) contracts: BTreeSet<DataTypeId>,
    pub(crate) unreadable: BTreeSet<String>,
}

pub(crate) struct Builder {
    facts: DataFacts,
    remaining: usize,
    rust_modules: BTreeMap<String, Vec<String>>,
    layout_remaining: usize,
    layout_truncated: bool,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            facts: DataFacts::default(),
            remaining: TREE_BUDGET,
            rust_modules: BTreeMap::new(),
            layout_remaining: SOURCE_BUDGET,
            layout_truncated: false,
        }
    }
}

pub(super) struct Declaration {
    fact: DataType,
    links: Vec<(String, Target)>,
    schema_pointer: Option<String>,
}

#[derive(Default)]
struct Source {
    types: Vec<Declaration>,
    external_modules: Option<Vec<String>>,
    layout_truncated: bool,
}

impl From<Vec<Declaration>> for Source {
    fn from(types: Vec<Declaration>) -> Self {
        Self {
            types,
            ..Self::default()
        }
    }
}

const MAX_PATH_BYTES: usize = 1024;
const MAX_NAME_BYTES: usize = 256;
const MAX_TYPES: usize = 4096;
const MAX_FIELDS: usize = 1024;
const MAX_LINKS: usize = 4096;
const MAX_TYPE_BYTES: usize = 16 * 1024;
const SOURCE_BUDGET: usize = 8 * 1024 * 1024;
const TREE_BUDGET: usize = 64 * 1024 * 1024;
pub(crate) const REPLY_BUDGET: usize = 32 * 1024 * 1024;
const LINK_COST: usize = 2 * (MAX_PATH_BYTES + MAX_NAME_BYTES) + MAX_NAME_BYTES + 128;

// A name/path is repeated in every link and field delta. Bound the expanded
// shape, not only the input blob/reply, before materializing those rows.
fn shape_cost(path: &str, name: &str, fields: &[crate::DataField], links: usize) -> Option<usize> {
    if path.len() > MAX_PATH_BYTES
        || name.len() > MAX_NAME_BYTES
        || fields.len() > MAX_FIELDS
        || links > MAX_LINKS
    {
        return None;
    }
    let mut bytes = (path.len() + name.len() + 128) * (1 + fields.len()) + LINK_COST * links;
    for field in fields {
        if field.name.len() > MAX_NAME_BYTES || field.ty.len() > MAX_TYPE_BYTES {
            return None;
        }
        bytes += 2 * (field.name.len() + field.ty.len());
    }
    Some(bytes)
}

pub(crate) fn extension_shape_cost(
    path: &str,
    name: &str,
    fields: &[crate::DataField],
) -> Option<usize> {
    // Reject oversized fields before scanning their type text for link names.
    shape_cost(path, name, fields, 0)?;
    let links = fields
        .iter()
        .flat_map(|field| extension_names(&field.ty))
        .take(MAX_LINKS + 1)
        .count();
    shape_cost(path, name, fields, links)
}

fn source_cost(rows: &[Declaration]) -> Option<usize> {
    if rows.len() > MAX_TYPES {
        return None;
    }
    let remaining = rows.iter().try_fold(SOURCE_BUDGET, |left, row| {
        let fact = &row.fact;
        left.checked_sub(shape_cost(
            &fact.path,
            &fact.name,
            &fact.fields,
            row.links.len(),
        )?)
    })?;
    Some(SOURCE_BUDGET - remaining)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SchemaAnchor {
    id: DataTypeId,
    pointer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
enum Target {
    Schema { path: String, pointer: String },
    Name(String),
    Rust(rust::ModulePath),
    Exact(DataTypeId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Link {
    from: DataTypeId,
    field: String,
    to: Target,
}

impl Builder {
    pub(crate) fn read(&mut self, path: &str, text: Option<&str>) {
        let bounded = text.filter(|_| path.len() <= MAX_PATH_BYTES);
        let (source, rows) = match extension(path) {
            Some("rs") => (
                DataSource::RustSerde,
                bounded.and_then(|text| rust::read(path, text)),
            ),
            Some("sql") => (
                DataSource::Sql,
                bounded
                    .and_then(|text| sql::read(path, text))
                    .map(Source::from),
            ),
            Some("json") => (
                DataSource::JsonSchema,
                bounded
                    .and_then(|text| json::read(path, text))
                    .map(Source::from),
            ),
            _ => return,
        };
        let facts = &mut self.facts;
        if let Some(rows) = rows.and_then(|mut rows| {
            // Path layout is optional evidence, independently bounded. Large
            // module graphs must not crowd out otherwise readable type shapes.
            self.layout_truncated |= rows.layout_truncated;
            if let Some(modules) = rows
                .external_modules
                .take()
                .filter(|modules| !modules.is_empty())
            {
                let bytes = modules
                    .iter()
                    .map(|name| name.len() + path.len() + 128)
                    .sum();
                if let Some(remaining) = self.layout_remaining.checked_sub(bytes) {
                    self.layout_remaining = remaining;
                    self.rust_modules.insert(path.to_owned(), modules);
                } else {
                    self.layout_truncated = true;
                }
            }
            self.remaining = self.remaining.checked_sub(source_cost(&rows.types)?)?;
            Some(rows)
        }) {
            facts.sources.insert(path.to_owned(), source);
            for row in rows.types {
                if let Some(pointer) = row.schema_pointer {
                    facts.schema_anchors.push(SchemaAnchor {
                        id: row.fact.id(),
                        pointer,
                    });
                }
                facts.add(row.fact, row.links);
            }
        } else if source != DataSource::JsonSchema
            || text.is_none_or(|text| json::is_candidate(path, text))
        {
            // Ordinary JSON is not a schema. Oversized/non-text blobs, or an
            // unreadable file in a detected language, are not evidence of removal.
            facts.unreadable.insert(path.to_owned());
        }
    }

    pub(crate) fn read_status(&self, path: &str) -> Option<ReadStatus> {
        if self.facts.sources.contains_key(path) {
            Some(ReadStatus::Complete)
        } else if self.facts.unreadable.contains(path) {
            Some(ReadStatus::Failed)
        } else {
            None
        }
    }

    #[cfg(test)]
    pub(crate) fn finish<'a>(
        self,
        files: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> DataFacts {
        self.finish_project(files, &BTreeMap::new(), &RuntimeFacts::default())
    }

    pub(crate) fn finish_project<'a>(
        mut self,
        files: impl IntoIterator<Item = (&'a str, &'a str)>,
        manifests: &BTreeMap<String, String>,
        runtime: &RuntimeFacts,
    ) -> DataFacts {
        let files: Vec<_> = files.into_iter().collect();
        if self
            .facts
            .links
            .iter()
            .any(|link| matches!(link.to, Target::Rust(_)))
        {
            // An incomplete tree could hide a second mount of a shared file.
            // Fall back to file-local inline scopes, never a partial crate graph.
            let layout = if self.layout_truncated {
                rust::Layout::default()
            } else {
                rust::Layout::collect(
                    files.iter().map(|&(path, _)| path),
                    &self.rust_modules,
                    manifests,
                    runtime,
                )
            };
            self.facts.links.retain_mut(|link| {
                if let Target::Rust(path) = &link.to {
                    let Some(target) = layout.resolve(&link.from, path) else {
                        return false;
                    };
                    link.to = Target::Exact(target);
                }
                true
            });
        }
        self.facts.finish(files);
        self.facts
    }
}

impl DataFacts {
    pub(crate) fn overlay<'a>(
        &mut self,
        rows: Vec<DataType>,
        files: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) {
        if rows.is_empty() {
            return;
        }
        let covered: BTreeSet<_> = rows.iter().map(|row| row.path.clone()).collect();
        self.types.retain(|row| !covered.contains(&row.path));
        self.links.retain(|link| !covered.contains(&link.from.path));
        for path in covered {
            self.unreadable.remove(&path);
            self.sources.insert(path, DataSource::Extension);
        }
        for row in rows {
            let links: Vec<_> = row
                .fields
                .iter()
                .flat_map(|field| {
                    extension_names(&field.ty)
                        .map(|name| (field.name.clone(), Target::Name(name.into())))
                })
                .collect();
            self.add(row, links);
        }
        self.finish(files);
    }

    fn add(&mut self, fact: DataType, links: Vec<(String, Target)>) {
        let id = fact.id();
        if fact.source != DataSource::Extension {
            self.contracts.insert(id.clone());
        }
        self.links.extend(links.into_iter().map(|(field, to)| Link {
            from: id.clone(),
            field,
            to,
        }));
        self.types.push(fact);
    }

    fn finish<'a>(&mut self, files: impl IntoIterator<Item = (&'a str, &'a str)>) {
        self.coverage.clear();
        for (path, language) in files {
            let sources = self.coverage.entry(language.to_owned()).or_default();
            sources.extend(self.sources.get(path).copied());
        }
        self.types
            .sort_by(|a, b| (&a.path, &a.name, a.line).cmp(&(&b.path, &b.name, b.line)));
        self.types
            .dedup_by(|a, b| a.path == b.path && a.name == b.name);
        self.edges = edges(&self.types, &self.links, &self.schema_anchors);
    }

    /// At least one side has evidence from a source, and neither side was unreadable.
    /// A file that was absent or was not a schema on one side can still be compared.
    pub fn can_compare_path(&self, other: &Self, path: &str) -> bool {
        !self.unreadable.contains(path)
            && !other.unreadable.contains(path)
            && (self.sources.contains_key(path) || other.sources.contains_key(path))
    }

    pub fn source_at(&self, path: &str) -> Option<DataSource> {
        self.sources.get(path).copied()
    }

    pub fn unreadable_paths(&self) -> &BTreeSet<String> {
        &self.unreadable
    }

    pub(crate) fn schema_pointers(&self) -> BTreeMap<(&str, &str), &str> {
        self.schema_anchors
            .iter()
            .map(|anchor| {
                (
                    (anchor.id.path.as_str(), anchor.id.name.as_str()),
                    anchor.pointer.as_str(),
                )
            })
            .collect()
    }
}

fn edges(types: &[DataType], links: &[Link], anchors: &[SchemaAnchor]) -> Vec<DataEdge> {
    let by_id: BTreeMap<_, _> = types.iter().map(|row| (row.id(), row)).collect();
    let by_pointer: BTreeMap<_, _> = anchors
        .iter()
        .map(|anchor| {
            (
                (anchor.id.path.as_str(), anchor.pointer.as_str()),
                &anchor.id,
            )
        })
        .collect();
    let mut names = BTreeMap::<&str, Vec<&DataType>>::new();
    let mut fields = BTreeMap::new();
    for (id, row) in &by_id {
        names.entry(&row.name).or_default().push(*row);
        let sep = separator(row);
        for (index, _) in row.name.match_indices(sep) {
            names
                .entry(&row.name[index + sep.len()..])
                .or_default()
                .push(*row);
        }
        fields.insert(
            id,
            row.fields
                .iter()
                .map(|field| (field.name.as_str(), field.line))
                .collect::<BTreeMap<_, _>>(),
        );
    }
    links
        .iter()
        .filter_map(|link| {
            let from = by_id.get(&link.from)?;
            let line = *fields.get(&link.from)?.get(link.field.as_str())?;
            let to = match &link.to {
                Target::Schema { path, pointer } => by_pointer
                    .get(&(path.as_str(), pointer.as_str()))
                    .and_then(|id| by_id.get(*id).copied()),
                Target::Name(name) => resolve_name(&names, from, name),
                Target::Exact(id) => by_id.get(id).copied(),
                Target::Rust(_) => None,
            }?;
            Some(DataEdge {
                kind: DataEdgeKind::Has,
                from: link.from.clone(),
                to: to.id(),
                field: link.field.clone(),
                line,
            })
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn resolve_name<'a>(
    names: &BTreeMap<&str, Vec<&'a DataType>>,
    from: &DataType,
    name: &str,
) -> Option<&'a DataType> {
    let from_ext = extension(&from.path);
    let candidates = || {
        names
            .get(name)
            .into_iter()
            .flatten()
            .copied()
            .filter(|row| {
                extension(&row.path) == from_ext
                    && (from.source != DataSource::Sql || row.kind == DataKind::Table)
            })
    };
    let mut same_file = candidates().filter(|row| row.path == from.path).peekable();
    if same_file.peek().is_some() {
        let sep = separator(from);
        let mut scope = from.name.as_str();
        while let Some((module, _)) = scope.rsplit_once(sep) {
            let scoped = format!("{module}{sep}{name}");
            if let Some(row) = candidates().find(|row| row.path == from.path && row.name == scoped)
            {
                return Some(row);
            }
            scope = module;
        }
        if let Some(row) = candidates().find(|row| row.path == from.path && row.name == name) {
            return Some(row);
        }
        let first = same_file.next();
        return first.filter(|_| same_file.next().is_none());
    }
    let mut rows = candidates();
    let first = rows.next();
    first.filter(|_| rows.next().is_none())
}

fn extension(path: &str) -> Option<&str> {
    path.rsplit('/')
        .next()?
        .rsplit_once('.')
        .map(|(_, ext)| ext)
}

fn separator(ty: &DataType) -> &'static str {
    if ty.kind == DataKind::Table {
        "."
    } else {
        "::"
    }
}

// Extensions describe their field types as text; only whole path-like names may
// contribute inferred edges (not substrings of other identifiers).
fn extension_names(ty: &str) -> impl Iterator<Item = &str> {
    ty.split(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | ':' | '.')))
        .map(|part| part.trim_matches(':'))
        .filter(|part| !part.is_empty())
}

#[cfg(test)]
mod golden;
