//! Structural decoding of setup input that reports every missing, unknown, mistyped or
//! duplicated field at once, with the JSON path of each.
//!
//! After each serde error the offending node is repaired with a placeholder (or its
//! containing element removed) purely so decoding can continue. Repaired nodes are recorded
//! as tainted: semantic verdicts that rely on them are discarded, and a draft with any
//! structural problem is never accepted or persisted.
use std::collections::BTreeSet;
use std::fmt;

use anyhow::Result;
use corral_core::{JsonPath, PathProblem, PathSeg};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

use crate::setup::SetupProposalInput;
use crate::setup_validation::{SetupValidationError, SetupValidationErrors};

/// Decoded (possibly repaired) setup input plus every structural problem found in it.
#[derive(Debug, Clone)]
pub struct SetupInputDraft {
    pub(crate) input: SetupProposalInput,
    pub(crate) notes: Notes,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Notes {
    pub(crate) problems: Vec<(JsonPath, String)>,
    tainted: Vec<JsonPath>,
    removed: Vec<Vec<PathSeg>>,
}

impl SetupInputDraft {
    /// Submitted-input location of a path in the repaired value.
    pub(crate) fn original(&self, path: &JsonPath) -> JsonPath {
        JsonPath(original_path(&path.0, &self.notes.removed))
    }

    /// Whether a semantic verdict relied on a repaired node.
    pub(crate) fn relies_on_repair(&self, problem: &PathProblem) -> bool {
        std::iter::once(&problem.path)
            .chain(&problem.subjects)
            .map(|path| self.original(path))
            .any(|path| self.notes.tainted.iter().any(|taint| taint.related(&path)))
    }
}

const PROFILE_FIELDS: [&str; 7] = [
    "metadata",
    "harness",
    "model",
    "vendor",
    "escalate_to",
    "effort",
    "args",
];
const METADATA_FIELDS: [&str; 9] = [
    "display_name",
    "purpose",
    "guidance",
    "instructions",
    "eligibility",
    "revision",
    "evidence",
    "candidates",
    "fallback",
];
const DUPLICATE_PROBE: &str = "\u{1}corral-duplicate-probe";

pub fn parse_setup_input(bytes: &[u8]) -> Result<SetupInputDraft> {
    let mut value: Value = serde_json::from_slice(bytes).map_err(|err| {
        SetupValidationErrors(vec![SetupValidationError {
            path: JsonPath::default().to_string(),
            message: format!("invalid JSON: {err}"),
        }])
    })?;
    let typed_error = match serde_path_to_error::deserialize::<_, SetupProposalInput>(
        &mut serde_json::Deserializer::from_slice(bytes),
    ) {
        Ok(input) => {
            let mut notes = Notes::default();
            misplaced_profile_fields(&value, &mut notes.problems);
            return Ok(SetupInputDraft { input, notes });
        }
        Err(err) => (JsonPath(segments(err.path())), err.inner().to_string()),
    };
    let duplicates = duplicate_keys(bytes);
    let mut notes = Notes::default();
    misplaced_profile_fields(&value, &mut notes.problems);
    // Every iteration repairs or removes at least one submitted node, so the input size
    // bounds the loop; it is not a cap on how many problems are reported.
    for _ in 0..=bytes.len() {
        let problem = match decode(&value) {
            Ok(input) => {
                duplicate_problems(&value, &duplicates, &mut notes);
                // The typed decoder is the authority on rejection; never accept what it refused.
                if notes.problems.is_empty() {
                    notes.tainted.push(typed_error.0.clone());
                    notes.problems.push(typed_error);
                }
                return Ok(SetupInputDraft { input, notes });
            }
            Err(problem) => problem,
        };
        let original = JsonPath(original_path(&problem.path, &notes.removed));
        notes.problems.push((original, problem.message.clone()));
        if repair(&mut value, &problem, 0) {
            let target = JsonPath(original_path(&problem.target(), &notes.removed));
            notes.tainted.push(target);
        } else if let Some(removed) = discard(&mut value, &problem, &notes.removed) {
            if matches!(removed.last(), Some(PathSeg::Index(_))) {
                notes.removed.push(removed.clone());
            }
            notes.tainted.push(JsonPath(removed));
        } else {
            break;
        }
    }
    let mut errors: Vec<_> = notes
        .problems
        .into_iter()
        .map(|(path, message)| SetupValidationError {
            path: path.to_string(),
            message,
        })
        .collect();
    errors.push(SetupValidationError {
        path: JsonPath::default().to_string(),
        message:
            "input structure cannot be checked further; fix the problems above and validate again"
                .into(),
    });
    Err(SetupValidationErrors(errors).into())
}

/// Profiles ignore unknown keys in configuration files, so a metadata field written beside
/// `metadata` would vanish silently. Report it where the profile already lacks metadata.
fn misplaced_profile_fields(value: &Value, problems: &mut Vec<(JsonPath, String)>) {
    let Some(profiles) = value
        .pointer("/proposal/profiles")
        .and_then(Value::as_object)
    else {
        return;
    };
    let base = JsonPath::default().key("proposal").key("profiles");
    for (key, profile) in profiles {
        let Some(profile) = profile.as_object() else {
            continue;
        };
        if profile.contains_key("metadata") {
            continue;
        }
        for field in profile.keys() {
            if PROFILE_FIELDS.contains(&field.as_str()) {
                continue;
            }
            let message = if METADATA_FIELDS.contains(&field.as_str()) {
                format!("`{field}` belongs inside `metadata`")
            } else {
                format!("unknown profile field `{field}`")
            };
            problems.push((base.key(key).key(field), message));
        }
    }
}

/// Typed fields reject duplicates; JSON maps keep the last value. A parsed `Value` keeps
/// only the last value either way, so each duplicate is replayed against the typed decoder.
fn duplicate_problems(value: &Value, duplicates: &[(Vec<PathSeg>, String)], notes: &mut Notes) {
    for (object, key) in duplicates {
        let Some(current) = current_path(object, &notes.removed) else {
            continue;
        };
        let Some(field) = node(value, &current)
            .and_then(Value::as_object)
            .and_then(|object| object.get(key))
        else {
            continue;
        };
        let mut probe = value.clone();
        if !set(
            &mut probe,
            &[current.clone(), vec![PathSeg::Key(DUPLICATE_PROBE.into())]].concat(),
            field.clone(),
        ) {
            continue;
        }
        let Ok(text) = serde_json::to_string(&probe) else {
            continue;
        };
        let (Ok(sentinel), Ok(name)) = (
            serde_json::to_string(DUPLICATE_PROBE),
            serde_json::to_string(key),
        ) else {
            continue;
        };
        let text = text.replacen(&sentinel, &name, 1);
        let message = format!("duplicate field `{key}`");
        let rejected = serde_json::from_str::<SetupProposalInput>(&text)
            .is_err_and(|err| err.to_string().starts_with(&message));
        if rejected {
            let path = JsonPath(object.clone()).key(key);
            notes.problems.push((path.clone(), message));
            notes.tainted.push(path);
        }
    }
}

fn duplicate_keys(bytes: &[u8]) -> Vec<(Vec<PathSeg>, String)> {
    let mut found = Vec::new();
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let scan = Scan {
        path: Vec::new(),
        found: &mut found,
    };
    // The bytes already parsed as JSON; a scan failure only means no duplicates are known.
    if scan.deserialize(&mut deserializer).is_err() {
        return Vec::new();
    }
    found
}

struct Scan<'a> {
    path: Vec<PathSeg>,
    found: &'a mut Vec<(Vec<PathSeg>, String)>,
}

impl<'de> DeserializeSeed<'de> for Scan<'_> {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Scan<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_bool<E>(self, _: bool) -> Result<(), E> {
        Ok(())
    }

    fn visit_i64<E>(self, _: i64) -> Result<(), E> {
        Ok(())
    }

    fn visit_u64<E>(self, _: u64) -> Result<(), E> {
        Ok(())
    }

    fn visit_f64<E>(self, _: f64) -> Result<(), E> {
        Ok(())
    }

    fn visit_str<E>(self, _: &str) -> Result<(), E> {
        Ok(())
    }

    fn visit_unit<E>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let Scan { path, found } = self;
        let mut index = 0;
        loop {
            let child = Scan {
                path: [path.clone(), vec![PathSeg::Index(index)]].concat(),
                found: &mut *found,
            };
            if seq.next_element_seed(child)?.is_none() {
                return Ok(());
            }
            index += 1;
        }
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let Scan { path, found } = self;
        let mut seen = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                found.push((path.clone(), key.clone()));
            }
            map.next_value_seed(Scan {
                path: [path.clone(), vec![PathSeg::Key(key)]].concat(),
                found: &mut *found,
            })?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProblemKind {
    Unknown(String),
    Missing(String),
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Problem {
    path: Vec<PathSeg>,
    kind: ProblemKind,
    message: String,
}

impl Problem {
    fn target(&self) -> Vec<PathSeg> {
        let mut target = self.path.clone();
        if let ProblemKind::Missing(field) = &self.kind {
            target.push(PathSeg::Key(field.clone()));
        }
        target
    }
}

fn decode(value: &Value) -> std::result::Result<SetupProposalInput, Problem> {
    serde_path_to_error::deserialize(value).map_err(|err| {
        let mut path = segments(err.path());
        let message = err.inner().to_string();
        let kind = if let Some(field) = quoted_after(&message, "unknown field `") {
            // Point at the stray field itself rather than its containing object.
            if node(value, &path)
                .and_then(Value::as_object)
                .is_some_and(|object| object.contains_key(&field))
            {
                path.push(PathSeg::Key(field.clone()));
            }
            ProblemKind::Unknown(field)
        } else if let Some(field) = quoted_after(&message, "missing field `") {
            ProblemKind::Missing(field)
        } else {
            ProblemKind::Invalid
        };
        Problem {
            path,
            kind,
            message,
        }
    })
}

fn segments(path: &serde_path_to_error::Path) -> Vec<PathSeg> {
    let mut segments = Vec::new();
    for seg in path.iter() {
        match seg {
            serde_path_to_error::Segment::Seq { index } => segments.push(PathSeg::Index(*index)),
            serde_path_to_error::Segment::Map { key } => segments.push(PathSeg::Key(key.clone())),
            serde_path_to_error::Segment::Enum { variant } => {
                segments.push(PathSeg::Key(variant.clone()));
            }
            serde_path_to_error::Segment::Unknown => break,
        }
    }
    segments
}

fn quoted_after(message: &str, prefix: &str) -> Option<String> {
    let rest = message.strip_prefix(prefix)?;
    rest.find('`').map(|end| rest[..end].to_owned())
}

fn node<'a>(value: &'a Value, path: &[PathSeg]) -> Option<&'a Value> {
    path.iter().try_fold(value, |value, seg| match seg {
        PathSeg::Key(key) => value.as_object()?.get(key),
        PathSeg::Index(index) => value.as_array()?.get(*index),
    })
}

fn node_mut<'a>(value: &'a mut Value, path: &[PathSeg]) -> Option<&'a mut Value> {
    path.iter().try_fold(value, |value, seg| match seg {
        PathSeg::Key(key) => value.as_object_mut()?.get_mut(key),
        PathSeg::Index(index) => value.as_array_mut()?.get_mut(*index),
    })
}

fn set(value: &mut Value, path: &[PathSeg], replacement: Value) -> bool {
    let Some((last, parent)) = path.split_last() else {
        return false;
    };
    match (node_mut(value, parent), last) {
        (Some(Value::Object(object)), PathSeg::Key(key)) => {
            object.insert(key.clone(), replacement);
            true
        }
        (Some(Value::Array(items)), PathSeg::Index(index)) if *index < items.len() => {
            items[*index] = replacement;
            true
        }
        _ => false,
    }
}

fn remove(value: &mut Value, path: &[PathSeg]) -> bool {
    let Some((last, parent)) = path.split_last() else {
        return false;
    };
    match (node_mut(value, parent), last) {
        (Some(Value::Object(object)), PathSeg::Key(key)) => object.remove(key).is_some(),
        (Some(Value::Array(items)), PathSeg::Index(index)) if *index < items.len() => {
            items.remove(*index);
            true
        }
        _ => false,
    }
}

fn placeholders() -> [Value; 8] {
    [
        Value::Null,
        Value::Bool(false),
        Value::from(0),
        Value::from(""),
        Value::Array(Vec::new()),
        Value::Object(serde_json::Map::new()),
        Value::from("1970-01-01T00:00:00Z"),
        Value::from("1970-01-01"),
    ]
}

/// Make the node at a problem decodable, silently filling nested placeholders.
fn repair(value: &mut Value, problem: &Problem, depth: usize) -> bool {
    match &problem.kind {
        ProblemKind::Unknown(_) => remove(value, &problem.path),
        ProblemKind::Missing(_) | ProblemKind::Invalid => fill(value, &problem.target(), depth),
    }
}

fn fill(value: &mut Value, target: &[PathSeg], depth: usize) -> bool {
    for placeholder in placeholders() {
        let mut trial = value.clone();
        if !set(&mut trial, target, placeholder) {
            return false;
        }
        if settle(&mut trial, target, depth) {
            *value = trial;
            return true;
        }
    }
    false
}

fn settle(trial: &mut Value, target: &[PathSeg], depth: usize) -> bool {
    for _ in 0..16 {
        match decode(trial) {
            Ok(_) => return true,
            Err(next) if next.kind == ProblemKind::Invalid && next.path == target => return false,
            Err(next) if !next.path.starts_with(target) => return true,
            Err(next) => {
                if depth >= 3 || !repair(trial, &next, depth + 1) {
                    return false;
                }
            }
        }
    }
    false
}

/// Drop the nearest removable array element or map entry containing an unrepairable node,
/// returning its submitted-input path.
fn discard(value: &mut Value, problem: &Problem, removed: &[Vec<PathSeg>]) -> Option<Vec<PathSeg>> {
    let target = problem.target();
    for len in (1..=target.len()).rev() {
        let path = &target[..len];
        let mut trial = value.clone();
        if !remove(&mut trial, path) {
            continue;
        }
        let required = match (decode(&trial), &path[len - 1]) {
            (Err(next), PathSeg::Key(key)) => {
                next.path == path[..len - 1] && next.kind == ProblemKind::Missing(key.clone())
            }
            _ => false,
        };
        if required {
            continue;
        }
        *value = trial;
        return Some(original_path(path, removed));
    }
    None
}

/// Map a path in the repaired value back to the submitted input's array indices.
/// `removed` holds submitted-input paths of discarded elements, in discovery order.
fn original_path(path: &[PathSeg], removed: &[Vec<PathSeg>]) -> Vec<PathSeg> {
    let mut original = Vec::with_capacity(path.len());
    for seg in path {
        match seg {
            PathSeg::Key(key) => original.push(PathSeg::Key(key.clone())),
            PathSeg::Index(index) => {
                let mut gone = removed_indices(&original, removed);
                gone.sort_unstable();
                let mut index = *index;
                for at in gone {
                    if at <= index {
                        index += 1;
                    }
                }
                original.push(PathSeg::Index(index));
            }
        }
    }
    original
}

/// Inverse of [`original_path`]; `None` when the path lies inside a discarded element.
fn current_path(path: &[PathSeg], removed: &[Vec<PathSeg>]) -> Option<Vec<PathSeg>> {
    let mut original = Vec::with_capacity(path.len());
    let mut current = Vec::with_capacity(path.len());
    for seg in path {
        match seg {
            PathSeg::Key(key) => current.push(PathSeg::Key(key.clone())),
            PathSeg::Index(index) => {
                let gone = removed_indices(&original, removed);
                if gone.contains(index) {
                    return None;
                }
                let before = gone.iter().filter(|at| **at < *index).count();
                current.push(PathSeg::Index(index - before));
            }
        }
        original.push(seg.clone());
    }
    Some(current)
}

fn removed_indices(array: &[PathSeg], removed: &[Vec<PathSeg>]) -> Vec<usize> {
    removed
        .iter()
        .filter_map(|item| match item.split_last() {
            Some((PathSeg::Index(at), parent)) if parent == array => Some(*at),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
#[path = "setup_structure_tests.rs"]
mod tests;
