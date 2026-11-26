//! Artifact workflow data, not executable workflow definitions or verified evidence.
use std::collections::{HashMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const RECIPE_SCHEMA: &str = "corral.workflow-diagram";
pub const RECIPE_VERSION: u64 = 1;
pub const MAX_RECIPE_BYTES: usize = 32 * 1024;
const MAX_NODES: usize = 80;
const MAX_EDGES: usize = 160;
const MAX_ID: usize = 64;
const MAX_LABEL: usize = 160;
const MAX_PATH: usize = 512;
const MAX_LINE: u32 = 1_000_000;
const MAX_SPAN: u32 = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRecipe {
    pub schema: String,
    pub version: u64,
    pub subject: DiagramSubject,
    pub start: String,
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiagramSubject {
    /// Exact configured project key. Never a filesystem path or remote.
    pub project: String,
    /// Full lowercase Git object ID. The resolver must verify that it is a local commit.
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowNode {
    pub id: String,
    pub kind: NodeKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bindings: Option<DiagramBindings>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Step,
    Decision,
    End,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowEdge {
    pub id: String,
    pub kind: EdgeKind,
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bindings: Option<DiagramBindings>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Next,
    Branch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiagramBindings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<EntityRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceRequest {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<SourceSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceSpan {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EntityRequest {
    File { path: String },
    Definition { path: String, name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DiagramDiagnostic {
    pub code: DiagnosticCode,
    /// JSON Pointer; empty for a document-level error.
    pub at: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    TooLarge,
    InvalidJson,
    UnsupportedSchema,
    UnsupportedVersion,
    InvalidShape,
    InvalidSubject,
    InvalidId,
    DuplicateId,
    LimitExceeded,
    InvalidLabel,
    InvalidBinding,
    DanglingEdge,
    InvalidStart,
    InvalidBranch,
    InvalidFlow,
    UnreachableNode,
}

impl DiagnosticCode {
    fn at(self, at: impl Into<String>, message: impl Into<String>) -> DiagramDiagnostic {
        DiagramDiagnostic {
            code: self,
            at: at.into(),
            message: message.into(),
        }
    }
}

/// Probe complete, bounded artifact bytes for the explicit root schema, including future versions.
/// False only means there was no recognizable bounded JSON document; keep the raw artifact path.
pub fn declares_schema(bytes: &[u8]) -> bool {
    bytes.len() <= MAX_RECIPE_BYTES
        && serde_json::from_slice::<Value>(bytes).is_ok_and(|value| matches_schema(&value))
}

fn matches_schema(value: &Value) -> bool {
    value.get("schema").and_then(Value::as_str) == Some(RECIPE_SCHEMA)
}

/// Returns a structurally renderable recipe or the first diagnostic. This never resolves bindings,
/// checks project ownership, looks up Git objects or promises that a renderer is installed.
pub fn validate(bytes: &[u8]) -> Result<WorkflowRecipe, DiagramDiagnostic> {
    use DiagnosticCode::*;
    if bytes.len() > MAX_RECIPE_BYTES {
        return Err(TooLarge.at("", "recipe exceeds 32768 bytes"));
    }
    let header: Value =
        serde_json::from_slice(bytes).map_err(|error| decode_error(InvalidJson, &error))?;
    if !matches_schema(&header) {
        return Err(UnsupportedSchema.at("/schema", "expected corral.workflow-diagram"));
    }
    if header.get("version").and_then(Value::as_u64) != Some(RECIPE_VERSION) {
        return Err(UnsupportedVersion.at("/version", "expected recipe version 1"));
    }
    // Decode the original bytes, not Value: serde's typed decode rejects duplicate known
    // fields and unknown fields rather than silently normalizing them.
    let recipe: WorkflowRecipe =
        serde_json::from_slice(bytes).map_err(|error| decode_error(InvalidShape, &error))?;
    validate_recipe(&recipe)?;
    Ok(recipe)
}

fn decode_error(code: DiagnosticCode, error: &serde_json::Error) -> DiagramDiagnostic {
    // Unknown field names and unexpected values in serde's message can come from the artifact.
    let mut message = error.to_string();
    if let Some((end, _)) = message.char_indices().nth(240) {
        message.truncate(end);
        message.push_str(&format!(
            "… at line {} column {}",
            error.line(),
            error.column()
        ));
    }
    code.at("", message)
}

fn validate_recipe(recipe: &WorkflowRecipe) -> Result<(), DiagramDiagnostic> {
    use DiagnosticCode::*;
    let DiagramSubject { project, commit } = &recipe.subject;
    if !plain_text(project, 128) || project.contains(['/', '\\', ':']) {
        return Err(InvalidSubject.at(
            "/subject/project",
            "expected a bounded project key, not a path or URL",
        ));
    }
    if !matches!(commit.len(), 40 | 64)
        || !commit
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(InvalidSubject.at(
            "/subject/commit",
            "expected a full lowercase Git commit ID (40 or 64 hex digits)",
        ));
    }
    if recipe.nodes.len() > MAX_NODES {
        return Err(LimitExceeded.at("/nodes", "at most 80 nodes are allowed"));
    }
    if recipe.edges.len() > MAX_EDGES {
        return Err(LimitExceeded.at("/edges", "at most 160 edges are allowed"));
    }

    let mut ids = HashSet::new();
    let mut nodes = HashMap::new();
    for (index, node) in recipe.nodes.iter().enumerate() {
        let at = format!("/nodes/{index}");
        check_id(&mut ids, &node.id, &at)?;
        check_label(&node.label, &at)?;
        check_bindings(node.bindings.as_ref(), &at)?;
        nodes.insert(node.id.as_str(), index);
    }
    let start = nodes
        .get(recipe.start.as_str())
        .copied()
        .filter(|&index| recipe.nodes[index].kind != NodeKind::End)
        .ok_or_else(|| InvalidStart.at("/start", "start must name a step or decision node"))?;

    let mut outgoing = vec![Vec::new(); recipe.nodes.len()];
    let mut branches = HashSet::new();
    for (index, edge) in recipe.edges.iter().enumerate() {
        let at = format!("/edges/{index}");
        check_id(&mut ids, &edge.id, &at)?;
        let from = nodes
            .get(edge.from.as_str())
            .copied()
            .ok_or_else(|| DanglingEdge.at(format!("{at}/from"), "source node does not exist"))?;
        let to = nodes
            .get(edge.to.as_str())
            .copied()
            .ok_or_else(|| DanglingEdge.at(format!("{at}/to"), "target node does not exist"))?;
        if let Some(label) = &edge.label {
            check_label(label, &at)?;
        }
        check_bindings(edge.bindings.as_ref(), &at)?;
        match (recipe.nodes[from].kind, edge.kind, edge.label.as_deref()) {
            (NodeKind::Step, EdgeKind::Next, _) => {},
            (NodeKind::Decision, EdgeKind::Branch, Some(label)) => {
                if !branches.insert((from, label)) {
                    return Err(InvalidBranch.at(format!("{at}/label"), "branch labels must be distinct within a decision"));
                }
            },
            _ => return Err(InvalidBranch.at(at, "steps use next edges, decisions use labeled branch edges, and ends have no outgoing edges")),
        }
        outgoing[from].push(to);
    }
    for (index, node) in recipe.nodes.iter().enumerate() {
        match node.kind {
            NodeKind::Step if outgoing[index].len() != 1 => {
                return Err(InvalidFlow.at(
                    format!("/nodes/{index}"),
                    "a step requires exactly one next edge",
                ));
            }
            NodeKind::Decision if outgoing[index].len() < 2 => {
                return Err(InvalidBranch.at(
                    format!("/nodes/{index}"),
                    "a decision requires at least two branches",
                ));
            }
            _ => {}
        }
    }
    let mut pending = vec![start];
    let mut reachable = vec![false; recipe.nodes.len()];
    while let Some(index) = pending.pop() {
        if !reachable[index] {
            reachable[index] = true;
            pending.extend(&outgoing[index]);
        }
    }
    if let Some(index) = reachable.iter().position(|seen| !seen) {
        return Err(UnreachableNode.at(
            format!("/nodes/{index}"),
            "node is not reachable from start",
        ));
    }
    if !recipe.nodes.iter().any(|node| node.kind == NodeKind::End) {
        return Err(InvalidFlow.at("/nodes", "at least one end node must be reachable"));
    }
    Ok(())
}

fn check_id<'a>(
    ids: &mut HashSet<&'a str>,
    id: &'a str,
    at: &str,
) -> Result<(), DiagramDiagnostic> {
    use DiagnosticCode::*;
    if id.is_empty()
        || id.len() > MAX_ID
        || !id.as_bytes()[0].is_ascii_alphanumeric()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(InvalidId.at(format!("{at}/id"), "expected 1–64 ASCII letters, digits, underscores or hyphens, starting with a letter or digit"));
    }
    if !ids.insert(id) {
        return Err(DuplicateId.at(
            format!("{at}/id"),
            "element ID already used by a node or edge",
        ));
    }
    Ok(())
}

fn plain_text(text: &str, max_bytes: usize) -> bool {
    !text.is_empty()
        && text.len() <= max_bytes
        && text.trim() == text
        && !text
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
}

fn check_label(label: &str, at: &str) -> Result<(), DiagramDiagnostic> {
    if plain_text(label, MAX_LABEL) {
        Ok(())
    } else {
        Err(DiagnosticCode::InvalidLabel.at(
            format!("{at}/label"),
            "expected 1–160 bytes of single-line text without outer whitespace",
        ))
    }
}

fn check_path(path: &str, at: String) -> Result<(), DiagramDiagnostic> {
    if plain_text(path, MAX_PATH)
        && !path.contains(['\\', ':'])
        && path
            .split('/')
            .all(|part| !matches!(part, "" | "." | "..") && !part.eq_ignore_ascii_case(".git"))
    {
        Ok(())
    } else {
        Err(DiagnosticCode::InvalidBinding.at(
            at,
            "expected a literal repository-relative path (at most 512 bytes)",
        ))
    }
}

fn check_bindings(bindings: Option<&DiagramBindings>, at: &str) -> Result<(), DiagramDiagnostic> {
    let Some(bindings) = bindings else {
        return Ok(());
    };
    if let Some(source) = &bindings.source {
        check_path(&source.path, format!("{at}/bindings/source/path"))?;
        if let Some(span) = &source.span
            && (span.start == 0
                || span.end < span.start
                || span.end > MAX_LINE
                || span.end - span.start >= MAX_SPAN)
        {
            return Err(DiagnosticCode::InvalidBinding.at(
                format!("{at}/bindings/source/span"),
                "expected an inclusive span of at most 200 lines within lines 1–1000000",
            ));
        }
    }
    if let Some(entity) = &bindings.entity {
        let path = match entity {
            EntityRequest::File { path } => path,
            EntityRequest::Definition { path, name } => {
                if !plain_text(name, MAX_LABEL) {
                    return Err(DiagnosticCode::InvalidBinding.at(
                        format!("{at}/bindings/entity/name"),
                        "expected a definition name of 1–160 bytes",
                    ));
                }
                path
            }
        };
        check_path(path, format!("{at}/bindings/entity/path"))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "diagram_tests.rs"]
mod tests;
