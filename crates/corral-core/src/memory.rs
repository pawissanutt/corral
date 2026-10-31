//! Working-tree Markdown memory. Authorization belongs to the service layer;
//! callers must supply only authorized scopes and authenticated author/time.
//! No method invokes Git, changes the index, or treats memory as authority.
use anyhow::{Context, Result, bail};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Store;

mod io;
mod projection;
pub use projection::MemoryProjection;

pub const ENTRY_BYTES: usize = 16 * 1024;
pub const SUMMARY_BYTES: usize = 512;
pub const PAGE_ENTRIES: usize = 20;
pub const READ_BYTES: usize = 4096;
pub const PROJECTION_BYTES: usize = 12288;
/// Keep metadata indivisible and leave room for paginated text and receipt envelopes.
pub const METADATA_JSON_BYTES: usize = 4096;
const PAGE_JSON_BYTES: usize = crate::agent::RESULT_MAX_BYTES - 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "scope",
    content = "key",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum MemoryScope {
    Workspace,
    Project(String),
    Role(MemoryRole),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRole {
    Pm,
    Assistant,
    Worker,
}
impl MemoryRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pm => "pm",
            Self::Assistant => "assistant",
            Self::Worker => "worker",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySelector {
    pub scope: MemoryScope,
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Preference,
    Decision,
    Agreement,
    Guidance,
    Note,
    Person,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceKind {
    OwnerQuote,
    AgentInference,
    ImportedReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryProvenance {
    pub kind: ProvenanceKind,
    pub sources: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_quote: Option<String>,
}

/// Version-1 frontmatter; body is Markdown after the closing delimiter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryMetadata {
    pub version: u32,
    pub id: String,
    pub scope: MemoryScope,
    pub title: String,
    pub kind: MemoryKind,
    pub summary: String,
    #[serde(default)]
    pub project_into_prompt: bool,
    pub provenance: MemoryProvenance,
    pub updated_at: Timestamp,
    pub author: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryEntry {
    pub metadata: MemoryMetadata,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryItem {
    pub metadata: MemoryMetadata,
    /// SHA-256 of exact working-tree Markdown bytes, not reserialized metadata.
    pub hash: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryPage {
    pub entries: Vec<MemoryItem>,
    pub next_offset: Option<usize>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRead {
    pub item: MemoryItem,
    /// A UTF-8-safe body chunk. Offset and next_offset are byte offsets.
    pub body: String,
    pub next_offset: Option<usize>,
}

pub(super) fn key(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    {
        bail!("invalid memory key {value:?}: use 1-128 ASCII letters, digits, _ or -");
    }
    Ok(())
}

fn validate(entry: &MemoryEntry, selector: &MemorySelector) -> Result<()> {
    key(&selector.id)?;
    if let MemoryScope::Project(project) = &selector.scope {
        key(project)?;
    }
    let m = &entry.metadata;
    if serde_json::to_vec(m)
        .context("encoding memory metadata")?
        .len()
        > METADATA_JSON_BYTES
    {
        bail!(
            "memory metadata exceeds {METADATA_JSON_BYTES} JSON bytes; move long detail into the paginated body"
        );
    }
    if m.version != 1 || m.id != selector.id || m.scope != selector.scope {
        bail!("memory path and version-1 metadata must agree");
    }
    if m.summary.len() > SUMMARY_BYTES {
        bail!("memory summary exceeds {SUMMARY_BYTES} UTF-8 bytes");
    }
    if m.title.trim().is_empty() || m.author.trim().is_empty() {
        bail!("memory title and authenticated author must not be empty");
    }
    if m.provenance.sources.is_empty() || m.provenance.sources.iter().any(|s| s.trim().is_empty()) {
        bail!("memory provenance requires nonempty source references");
    }
    if m.provenance.kind == ProvenanceKind::OwnerQuote
        && m.provenance
            .exact_quote
            .as_ref()
            .is_none_or(|s| s.trim().is_empty())
    {
        bail!("owner_quote requires an exact quote, distinct from interpretation");
    }
    Ok(())
}

fn encode(entry: &MemoryEntry) -> Result<String> {
    let yaml =
        serde_yaml_ng::to_string(&entry.metadata).context("serializing memory frontmatter")?;
    let text = format!("---\n{yaml}---\n{}", entry.body);
    if text.len() > ENTRY_BYTES {
        bail!("memory entry exceeds {ENTRY_BYTES} UTF-8 bytes");
    }
    Ok(text)
}

fn decode(text: &str, selector: &MemorySelector) -> Result<MemoryEntry> {
    let rest = text
        .strip_prefix("---\n")
        .context("memory requires YAML frontmatter")?;
    let (yaml, body) = rest
        .split_once("\n---\n")
        .context("memory requires closing frontmatter delimiter")?;
    let entry = MemoryEntry {
        metadata: serde_yaml_ng::from_str(yaml).context("parsing memory frontmatter")?,
        body: body.to_owned(),
    };
    validate(&entry, selector)?;
    Ok(entry)
}

fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

impl Store {
    /// Create requires `None` (explicit absence); replacement requires the current byte hash.
    /// The service must bind author and time from its authenticated operation context.
    pub fn memory_put(
        &self,
        selector: &MemorySelector,
        mut entry: MemoryEntry,
        expected_hash: Option<&str>,
        author: &str,
        updated_at: Timestamp,
    ) -> Result<MemoryItem> {
        self.check_runtime()?;
        entry.metadata.author = author.to_owned();
        entry.metadata.updated_at = updated_at;
        validate(&entry, selector)?;
        let text = encode(&entry)?;
        let _lock = self.lock("memory")?;
        let path = io::entry_path(self, selector, true)?;
        io::check_hash(&path, expected_hash)?;
        crate::store::atomic_write(&path, &text)?;
        Ok(MemoryItem {
            metadata: entry.metadata,
            hash: hash(&text),
        })
    }

    pub fn memory_remove(&self, selector: &MemorySelector, expected_hash: &str) -> Result<()> {
        self.check_runtime()?;
        let _lock = self.lock("memory")?;
        let path = io::entry_path(self, selector, false)?;
        io::check_hash(&path, Some(expected_hash))?;
        std::fs::remove_file(&path).with_context(|| format!("removing memory {}", path.display()))
    }

    pub fn memory_read(
        &self,
        selector: &MemorySelector,
        offset: usize,
        limit: usize,
    ) -> Result<Option<MemoryRead>> {
        if limit == 0 || limit > READ_BYTES {
            bail!("memory read limit must be 1..={READ_BYTES}");
        }
        let path = io::entry_path(self, selector, false)?;
        let Some(text) = io::read(&path)? else {
            return Ok(None);
        };
        let entry = decode(&text, selector)
            .with_context(|| format!("invalid memory {}", path.display()))?;
        if offset > entry.body.len() || !entry.body.is_char_boundary(offset) {
            bail!("memory offset must be a UTF-8 boundary in the body");
        }
        let mut end = offset.saturating_add(limit).min(entry.body.len());
        while !entry.body.is_char_boundary(end) {
            end -= 1;
        }
        if end == offset && offset < entry.body.len() {
            bail!("memory read limit cannot fit the next UTF-8 character");
        }
        let mut result = MemoryRead {
            item: MemoryItem {
                metadata: entry.metadata,
                hash: hash(&text),
            },
            body: entry.body[offset..end].to_owned(),
            next_offset: (end < entry.body.len()).then_some(end),
        };
        // JSON escaping can expand a 4 KiB chunk considerably. Shrink by whole
        // characters, including metadata/continuation overhead in the wire bound.
        while serde_json::to_vec(&result)
            .context("encoding memory read page")?
            .len()
            > PAGE_JSON_BYTES
        {
            result.body.pop();
            result.next_offset = Some(offset + result.body.len());
        }
        Ok(Some(result))
    }

    /// Stable ID ordering; offset is positional, not a snapshot across owner edits.
    pub fn memory_list(
        &self,
        scope: &MemoryScope,
        offset: usize,
        limit: usize,
    ) -> Result<MemoryPage> {
        if limit == 0 || limit > PAGE_ENTRIES {
            bail!("memory list limit must be 1..={PAGE_ENTRIES}");
        }
        let ids = io::ids(self, scope)?;
        let mut entries = Vec::new();
        for id in ids.iter().skip(offset).take(limit) {
            let selector = MemorySelector {
                scope: scope.clone(),
                id: id.clone(),
            };
            let path = io::entry_path(self, &selector, false)?;
            let text = io::read(&path)?
                .with_context(|| format!("memory disappeared: {}", path.display()))?;
            let entry = decode(&text, &selector)
                .with_context(|| format!("invalid memory {}", path.display()))?;
            entries.push(MemoryItem {
                metadata: entry.metadata,
                hash: hash(&text),
            });
            // Reserve the page wrapper and largest possible offset, too.
            if serde_json::to_vec(&entries)
                .context("encoding memory list page")?
                .len()
                + 128
                > PAGE_JSON_BYTES
            {
                entries.pop();
                break;
            }
        }
        let end = offset.saturating_add(entries.len());
        Ok(MemoryPage {
            entries,
            next_offset: (end < ids.len()).then_some(end),
        })
    }
}
