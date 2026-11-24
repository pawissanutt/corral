//! JSON Schema (and OpenRPC component schemas), without fetching or re-basing URIs.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use super::{
    Declaration, LINK_COST, MAX_FIELDS, MAX_LINKS, MAX_NAME_BYTES, MAX_TYPE_BYTES, MAX_TYPES,
    SOURCE_BUDGET, Target, shape_cost,
};
use crate::{DataField, DataKind, DataSource, DataType};

const MAX_BYTES: usize = 4 * 1024 * 1024;

// Distinguish ordinary JSON from a likely but unreadable schema for delta coverage.
pub(super) fn is_candidate(path: &str, text: &str) -> bool {
    likely(path, text)
        && (text.len() > MAX_BYTES
            || serde_json::from_str::<Value>(text).map_or(true, |root| recognized(path, &root)))
}

fn likely(path: &str, text: &str) -> bool {
    path.ends_with(".json")
        && (path.ends_with(".schema.json")
            || [
                "$schema",
                "$defs",
                "definitions",
                "properties",
                "openrpc",
                "schemas",
            ]
            .iter()
            .any(|key| text.contains(key)))
}

fn recognized(path: &str, root: &Value) -> bool {
    path.ends_with(".schema.json")
        || root
            .get("$schema")
            .and_then(Value::as_str)
            .is_some_and(|uri| {
                ["http://json-schema.org/", "https://json-schema.org/"]
                    .iter()
                    .any(|prefix| uri.starts_with(prefix))
            })
        || ["$defs", "definitions", "properties"]
            .iter()
            .any(|key| root[key].is_object())
        || component_schemas(root).is_some()
}

fn component_schemas(root: &Value) -> Option<&Map<String, Value>> {
    root.get("components")?.get("schemas")?.as_object()
}

pub(super) fn read(path: &str, text: &str) -> Option<Vec<Declaration>> {
    if text.len() > MAX_BYTES || !likely(path, text) {
        return None;
    }
    // serde_json validates the entire input with its default nesting limit. The
    // targeted second pass finds locations without a parallel full-size AST.
    let root: Value = serde_json::from_str(text).ok()?;
    if !recognized(path, &root) {
        return None;
    }
    let doc = Document::new(text);
    let root_offset = doc.whitespace(0);
    let locations = doc.members(root_offset);
    let mut declarations = Vec::new();
    let root_name = if has_shape(&root) {
        root.get("title")
            .and_then(Value::as_str)
            .filter(|title| !title.is_empty())
            .or_else(|| path.rsplit('/').next()?.strip_suffix(".schema.json"))
            .or_else(|| path.rsplit('/').next()?.strip_suffix(".json"))
    } else {
        None
    };
    let mut reader = Reader {
        path,
        doc,
        declarations: &mut declarations,
        remaining: SOURCE_BUDGET,
    };
    if let Some(name) = root_name {
        reader.declaration(name, String::new(), &root, root_offset, root_offset)?;
    }
    for key in ["$defs", "definitions"] {
        if let (Some(schemas), Some(location)) = (root[key].as_object(), locations.get(key)) {
            reader.schemas(key, schemas, location.value)?;
        }
    }
    if let (Some(schemas), Some(components)) =
        (component_schemas(&root), locations.get("components"))
        && let Some(location) = reader.doc.members(components.value).get("schemas")
    {
        reader.schemas("components/schemas", schemas, location.value)?;
    }
    disambiguate(&mut declarations);
    Some(declarations)
}

fn has_shape(schema: &Value) -> bool {
    [
        "type",
        "properties",
        "enum",
        "$ref",
        "oneOf",
        "anyOf",
        "allOf",
    ]
    .iter()
    .any(|key| schema.get(key).is_some())
}

struct Reader<'a, 'b> {
    path: &'a str,
    doc: Document<'a>,
    declarations: &'b mut Vec<Declaration>,
    remaining: usize,
}

impl Reader<'_, '_> {
    fn schemas(
        &mut self,
        section: &str,
        schemas: &Map<String, Value>,
        offset: usize,
    ) -> Option<()> {
        if self.declarations.len() + schemas.len() > MAX_TYPES {
            return None;
        }
        let locations = self.doc.members(offset);
        for (name, schema) in schemas {
            if (schema.is_object() || schema.is_boolean())
                && let Some(location) = locations.get(name)
            {
                let pointer = format!("/{section}/{}", name.replace('~', "~0").replace('/', "~1"));
                self.declaration(name, pointer, schema, location.key, location.value)?;
            }
        }
        Some(())
    }

    fn declaration(
        &mut self,
        name: &str,
        pointer: String,
        schema: &Value,
        key: usize,
        value: usize,
    ) -> Option<()> {
        if name.len() > MAX_NAME_BYTES {
            return None;
        }
        let mut links = Vec::new();
        let mut fields = Vec::new();
        let entries = self.doc.members(value);
        let kind = if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            if properties.len() > MAX_FIELDS {
                return None;
            }
            let required: BTreeSet<&str> = schema
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if let Some(property_location) = entries.get("properties") {
                let locations = self.doc.members(property_location.value);
                let header = shape_cost(self.path, name, &[], 0)?;
                let mut available = self.remaining.checked_sub(header)?;
                for (field, property) in properties {
                    if let Some(location) = locations.get(field) {
                        if field.len() > MAX_NAME_BYTES {
                            return None;
                        }
                        // Reserve the worst-case expanded link cost before building
                        // targets and repeated field names.
                        let remaining_links = (available / LINK_COST).min(MAX_LINKS - links.len());
                        let (ty, targets) = self.property_type(property, remaining_links)?;
                        let data_field = DataField {
                            name: field.clone(),
                            ty,
                            line: self.doc.line(location.key),
                            optional: !required.contains(field.as_str()),
                        };
                        let cost = shape_cost(
                            self.path,
                            name,
                            std::slice::from_ref(&data_field),
                            targets.len(),
                        )? - header;
                        available = available.checked_sub(cost)?;
                        fields.push(data_field);
                        links.extend(targets.into_iter().map(|target| (field.clone(), target)));
                    }
                }
            }
            DataKind::Record
        } else if let Some(variants) = schema.get("enum").and_then(Value::as_array) {
            if variants.len() > MAX_FIELDS {
                return None;
            }
            if let Some(enum_location) = entries.get("enum") {
                let offsets = self.doc.array_offsets(enum_location.value);
                fields.extend(variants.iter().zip(offsets).map(|(variant, offset)| {
                    DataField {
                        name: variant
                            .as_str()
                            .map_or_else(|| variant.to_string(), str::to_owned),
                        ty: value_type(variant).to_owned(),
                        line: self.doc.line(offset),
                        optional: false,
                    }
                }));
            }
            DataKind::Enum
        } else {
            DataKind::Record
        };
        self.remaining =
            self.remaining
                .checked_sub(shape_cost(self.path, name, &fields, links.len())?)?;
        fields.sort_by_key(|field| field.line);
        self.declarations.push(Declaration {
            fact: DataType {
                path: self.path.to_owned(),
                name: name.to_owned(),
                line: self.doc.line(key),
                kind,
                source: DataSource::JsonSchema,
                fields,
            },
            links,
            schema_pointer: Some(pointer),
        });
        Some(())
    }

    fn property_type(&self, schema: &Value, max_links: usize) -> Option<(String, Vec<Target>)> {
        let mut display = String::new();
        let mut targets = Vec::new();
        render_type(
            self.path,
            schema,
            &mut display,
            &mut targets,
            max_links,
            false,
        )?;
        Some((display, targets))
    }
}

// Render only the supported syntactic subset, without evaluating schemas. The
// parser's nesting limit bounds recursion; output and link budgets are checked
// before appending/cloning, including for repeated alternatives.
fn render_type(
    path: &str,
    schema: &Value,
    display: &mut String,
    targets: &mut Vec<Target>,
    max_links: usize,
    nested: bool,
) -> Option<()> {
    fn append(out: &mut String, text: &str) -> Option<()> {
        if out.len().checked_add(text.len())? > MAX_TYPE_BYTES {
            return None;
        }
        out.push_str(text);
        Some(())
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        append(display, reference)?;
        if let Some(target) = reference_target(path, reference) {
            if targets.len() == max_links {
                return None;
            }
            targets.push(target);
        }
        return Some(());
    }
    let groups = ["anyOf", "oneOf"].map(|key| schema.get(key).and_then(Value::as_array));
    if groups.iter().any(Option::is_some) {
        let combined = groups.iter().all(Option::is_some);
        if nested && combined {
            append(display, "(")?;
        }
        for (index, alternatives) in groups.into_iter().flatten().enumerate() {
            if index > 0 {
                append(display, " & ")?;
            }
            if nested || combined {
                append(display, "(")?;
            }
            let plain_null_first = alternatives.first().is_some_and(|first| {
                first.get("type").and_then(Value::as_str) == Some("null")
                    && ["$ref", "oneOf", "anyOf"]
                        .into_iter()
                        .all(|key| first.get(key).is_none())
            });
            // Preserve nullable-ref display order without discarding any branch.
            let swap = alternatives.len() == 2
                && plain_null_first
                && alternatives[1]
                    .get("$ref")
                    .and_then(Value::as_str)
                    .is_some();
            if alternatives.is_empty() {
                append(display, "any")?;
            }
            for n in 0..alternatives.len() {
                if n > 0 {
                    append(display, " | ")?;
                }
                let branch = if swap { 1 - n } else { n };
                render_type(
                    path,
                    &alternatives[branch],
                    display,
                    targets,
                    max_links,
                    true,
                )?;
            }
            if nested || combined {
                append(display, ")")?;
            }
        }
        if nested && combined {
            append(display, ")")?;
        }
        return Some(());
    }
    let ty = schema.get("type");
    if ty.and_then(Value::as_str) == Some("array")
        && let Some(items) = schema.get("items").filter(|items| items.is_object())
    {
        append(display, "array<")?;
        render_type(path, items, display, targets, max_links, true)?;
        return append(display, ">");
    }
    match ty {
        Some(Value::String(ty)) => append(display, ty),
        Some(Value::Array(types)) => {
            let mut first = true;
            for ty in types.iter().filter_map(Value::as_str) {
                if !first {
                    append(display, " | ")?;
                }
                append(display, ty)?;
                first = false;
            }
            Some(())
        }
        _ if schema.get("properties").is_some() => append(display, "object"),
        _ if schema.get("enum").is_some() => append(display, "enum"),
        _ => append(display, "any"),
    }
}

fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn reference_target(path: &str, reference: &str) -> Option<Target> {
    let (file, pointer) = reference.split_once('#').unwrap_or((reference, ""));
    let pointer = decode_percent(pointer)?;
    let file = decode_percent(file)?;
    let path = if file.is_empty() {
        path.to_owned()
    } else {
        repo_path(path, &file)?
    };
    Some(Target::Schema { path, pointer })
}

fn disambiguate(rows: &mut [Declaration]) {
    // A title and a named definition can have the same display name, as can
    // definitions in different sections. Keep the first name and qualify any
    // later collision by pointer; linking never depends on the display name.
    let mut reserved: BTreeSet<_> = rows.iter().map(|row| row.fact.name.clone()).collect();
    let mut seen = BTreeSet::new();
    for row in rows {
        if !seen.insert(row.fact.name.clone())
            && let Some(pointer) = &row.schema_pointer
        {
            let mut name = format!("#{pointer}");
            while !reserved.insert(name.clone()) {
                name.push('#');
            }
            row.fact.name = name;
        }
    }
}

fn repo_path(path: &str, reference: &str) -> Option<String> {
    if reference.starts_with("//") || reference.contains([':', '?', '\\', '\0']) {
        return None;
    }
    let mut parts: Vec<&str> = if reference.starts_with('/') {
        Vec::new()
    } else {
        path.rsplit_once('/')
            .map_or_else(Vec::new, |(dir, _)| dir.split('/').collect())
    };
    for part in reference.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    let file = parts.last()?;
    file.ends_with(".json").then(|| parts.join("/"))
}

fn decode_percent(text: &str) -> Option<String> {
    let mut bytes = text.bytes();
    let mut out = Vec::with_capacity(text.len());
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let high = char::from(bytes.next()?).to_digit(16)?;
            let low = char::from(bytes.next()?).to_digit(16)?;
            out.push((high * 16 + low) as u8);
        } else {
            out.push(byte);
        }
    }
    String::from_utf8(out).ok()
}

struct Location {
    key: usize,
    value: usize,
}

/// Locate only object members and array values of interest. All skipped values are
/// already valid JSON; skip quoted strings and balanced containers iteratively.
struct Document<'a> {
    bytes: &'a [u8],
    newlines: Vec<usize>,
}

impl<'a> Document<'a> {
    fn new(text: &'a str) -> Self {
        let bytes = text.as_bytes();
        let newlines = text.match_indices('\n').map(|(offset, _)| offset).collect();
        Self { bytes, newlines }
    }

    fn line(&self, offset: usize) -> u64 {
        self.newlines.partition_point(|newline| *newline < offset) as u64 + 1
    }

    fn members(&self, start: usize) -> BTreeMap<String, Location> {
        let mut members = BTreeMap::new();
        if self.bytes.get(start) != Some(&b'{') {
            return members;
        }
        let mut key = self.whitespace(start + 1);
        while self.bytes.get(key) == Some(&b'"') {
            let end = self.string_end(key);
            let Ok(name) = serde_json::from_slice(&self.bytes[key..end]) else {
                break;
            };
            let value = self.whitespace(self.whitespace(end) + 1);
            members.insert(name, Location { key, value });
            let end = self.whitespace(self.value_end(value));
            if self.bytes.get(end) != Some(&b',') {
                break;
            }
            key = self.whitespace(end + 1);
        }
        members
    }

    fn array_offsets(&self, start: usize) -> impl Iterator<Item = usize> + '_ {
        let mut next = Some(self.whitespace(start + 1));
        std::iter::from_fn(move || {
            let value = next?;
            if self.bytes.get(value)? == &b']' {
                return None;
            }
            let end = self.whitespace(self.value_end(value));
            next = (self.bytes.get(end) == Some(&b',')).then(|| self.whitespace(end + 1));
            Some(value)
        })
    }

    fn whitespace(&self, mut index: usize) -> usize {
        while self.bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        index
    }

    fn string_end(&self, mut index: usize) -> usize {
        index += 1;
        while let Some(byte) = self.bytes.get(index) {
            index += 1;
            match byte {
                b'\\' => index += 1,
                b'"' => break,
                _ => {}
            }
        }
        index
    }

    fn value_end(&self, start: usize) -> usize {
        let mut index = start;
        match self.bytes.get(start) {
            Some(b'"') => self.string_end(start),
            Some(b'{' | b'[') => {
                let mut nesting = 0;
                while let Some(byte) = self.bytes.get(index) {
                    index += 1;
                    match byte {
                        b'{' | b'[' => nesting += 1,
                        b'}' | b']' => {
                            nesting -= 1;
                            if nesting == 0 {
                                break;
                            }
                        }
                        b'"' => index = self.string_end(index - 1),
                        _ => {}
                    }
                }
                index
            }
            _ => {
                while self.bytes.get(index).is_some_and(|byte| {
                    !byte.is_ascii_whitespace() && !matches!(byte, b',' | b'}' | b']')
                }) {
                    index += 1;
                }
                index
            }
        }
    }
}
