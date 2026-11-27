//! Declarative diagrams checked into a project. Only the caller supplies the authorized
//! repository and revision; the recipe cannot redirect the read or request a tool.
use crate::facts::EntryKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const REPO_RECIPE_SCHEMA: &str = "corral.repo-diagram";
pub const MAX_REPO_RECIPE_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoRecipe {
    pub schema: String,
    pub version: u8,
    pub id: String,
    pub title: String,
    pub layout: RecipeLayout,
    #[serde(default)]
    pub groups: Vec<RecipeGroup>,
    pub nodes: Vec<RecipeNode>,
    /// Array order is significant in ordered lanes.
    #[serde(default)]
    pub edges: Vec<RecipeEdge>,
    #[serde(default)]
    pub evidence: Vec<RecipeEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage: Option<EnumCoverage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipeLayout {
    Graph,
    OrderedLanes,
}

impl RecipeLayout {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Graph => "graph",
            Self::OrderedLanes => "ordered_lanes",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeGroup {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeNode {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeEdge {
    pub id: String,
    pub from: String,
    pub to: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeEvidence {
    pub element: String,
    pub anchor: EvidenceAnchor,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EvidenceAnchor {
    Path { path: String },
    Symbol { path: String, name: String },
    Text { path: String, text: String },
    Entry { entry_kind: EntryKind, name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnumCoverage {
    pub path: String,
    pub name: String,
    /// Selected stable node IDs, never display labels.
    pub nodes: Vec<String>,
}

impl RepoRecipe {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Parse the original document directly: duplicate fields must not be normalized away.
    pub fn from_json(json: &str) -> Result<Self, String> {
        if json.len() > MAX_REPO_RECIPE_BYTES {
            return Err("recipe exceeds 32768 bytes".into());
        }
        let recipe: Self = serde_json::from_str(json).map_err(|e| {
            format!(
                "invalid recipe JSON: {}",
                e.to_string().chars().take(256).collect::<String>()
            )
        })?;
        recipe.validate()?;
        Ok(recipe)
    }

    /// Check references, bounded primitives and binding shape without resolving evidence.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != REPO_RECIPE_SCHEMA || self.version != 1 {
            return Err("unsupported recipe schema/version".into());
        }
        if !valid_id(&self.id) || !plain(&self.title, 160) {
            return Err("invalid recipe ID or title".into());
        }
        if self.groups.len() > 32
            || !(1..=80).contains(&self.nodes.len())
            || self.edges.len() > 160
            || self.evidence.len() > 256
        {
            return Err(
                "expected 1–80 nodes, at most 32 groups, 160 edges and 256 bindings".into(),
            );
        }
        let mut ids = BTreeSet::new();
        let mut groups = BTreeSet::new();
        let mut nodes = BTreeSet::new();
        let mut register = |id, label| {
            if !valid_id(id) || !plain(label, 160) {
                return Err("invalid element ID or label".to_string());
            }
            if !ids.insert(id) {
                return Err(format!("duplicate element ID {id}"));
            }
            Ok(())
        };
        for group in &self.groups {
            register(&group.id, &group.label)?;
            groups.insert(group.id.as_str());
        }
        for n in &self.nodes {
            register(&n.id, &n.label)?;
            if n.group
                .as_ref()
                .is_some_and(|id| !groups.contains(id.as_str()))
            {
                return Err(format!("{}: unknown group", n.id));
            }
            nodes.insert(n.id.as_str());
        }
        for e in &self.edges {
            register(&e.id, &e.label)?;
            if !nodes.contains(e.from.as_str()) || !nodes.contains(e.to.as_str()) {
                return Err(format!("{}: unknown endpoint", e.id));
            }
            for value in [&e.from_label, &e.to_label, &e.note].into_iter().flatten() {
                if !plain(value, 160) {
                    return Err(format!("{}: invalid endpoint label or note", e.id));
                }
            }
        }
        let mut bindings = BTreeSet::new();
        for b in &self.evidence {
            if !ids.contains(b.element.as_str()) {
                return Err("unknown evidence element".into());
            }
            if !bindings.insert(b) {
                return Err(format!("{}: duplicate evidence binding", b.element));
            }
            let valid = match &b.anchor {
                EvidenceAnchor::Path { path } => valid_path(path),
                EvidenceAnchor::Symbol { path, name } => valid_path(path) && plain(name, 256),
                EvidenceAnchor::Text { path, text } => {
                    valid_path(path)
                        && text.len() <= 2048
                        && !text.trim().is_empty()
                        && !text
                            .chars()
                            .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
                }
                EvidenceAnchor::Entry { name, .. } => plain(name, 256),
            };
            if !valid {
                return Err(format!("{}: invalid evidence anchor", b.element));
            }
        }
        if let Some(c) = &self.coverage {
            if !valid_path(&c.path)
                || !plain(&c.name, 256)
                || c.nodes.is_empty()
                || c.nodes.len() > nodes.len()
            {
                return Err("invalid enum coverage".into());
            }
            let mut selected = BTreeSet::new();
            if c.nodes
                .iter()
                .any(|n| !nodes.contains(n.as_str()) || !selected.insert(n))
            {
                return Err("enum coverage needs distinct, known node IDs".into());
            }
        }
        Ok(())
    }
}

fn valid_id(s: &str) -> bool {
    s.len() <= 64
        && s.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn plain(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.trim() == s
        && !s
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
}

fn valid_path(s: &str) -> bool {
    plain(s, 512)
        && !s.contains(['\\', ':'])
        && s.split('/')
            .all(|p| !matches!(p, "" | "." | "..") && !p.eq_ignore_ascii_case(".git"))
}
