//! Recover a complete original-host lifecycle journal after proof-publication/CAS loss.
//! This only verifies retained facts; it never repeats shutdown or executor work.
use super::{boundary, facts, legacy::SourceEntry};
use anyhow::{Context, Result, ensure};
use corral_core::{Store, assistant::AssistantContainer};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader, Read},
};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fact {
    version: u32,
    assistant: String,
    generation: u32,
    session_id: String,
    admission: String,
    event: facts::Lifecycle,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalProof {
    pub version: u32,
    pub source: String,
    pub header_sha256: String,
    pub evidence: facts::Evidence,
    pub canonical_version: Option<u64>,
    pub entries: Vec<SourceEntry>,
}
impl JournalProof {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1
                && facts::hash(&self.header_sha256)
                && !self.entries.is_empty()
                && self.entries.iter().map(|e| e.bytes.len()).sum::<usize>() <= 512 * 1024
                && self
                    .entries
                    .iter()
                    .all(|e| facts::digest(e.bytes.as_bytes()) == e.sha256),
            "incomplete/corrupt retained lifecycle proof"
        );
        self.evidence.require_terminal()
    }
}

pub(super) fn require_prefix(
    canonical: &facts::Evidence,
    complete: &facts::Evidence,
) -> Result<()> {
    let mut normalized = canonical.clone();
    normalized.instances = complete.instances.clone();
    normalized.sealed = complete.sealed;
    ensure!(
        normalized == *complete,
        "canonical/journal executor source identity contradicts"
    );
    for (token, instance) in &canonical.instances {
        let later = complete
            .instances
            .get(token)
            .context("canonical executor missing from shutdown journal")?;
        let mut earlier = instance.clone();
        if matches!(earlier.executor, facts::Executor::Preparing { .. }) {
            // Preparation may be the last canonical publication, but its scope cannot change.
            let value = serde_json::to_value(&earlier.executor)?;
            let final_value = serde_json::to_value(&later.executor)?;
            ensure!(
                value["resource"] == final_value["resource"]
                    && value["config_sha256"] == final_value["config_sha256"],
                "canonical/journal executor scope contradicts"
            );
            earlier.executor = later.executor.clone();
        }
        if earlier.terminal.is_none() {
            earlier.terminal = later.terminal.clone();
        }
        ensure!(
            earlier == *later,
            "canonical/journal terminal evidence contradicts"
        );
    }
    Ok(())
}

pub(super) fn verify(
    store: &Store,
    assistant: &str,
    container: &AssistantContainer,
    row: &boundary::Activity,
    canonical: Option<(facts::Evidence, u64)>,
) -> Result<JournalProof> {
    let bound = container
        .state
        .generations
        .iter()
        .find(|g| g.n == row.generation)
        .and_then(|g| g.bound.as_ref())
        .context("original host binding missing")?;
    let path = &bound.session_file;
    ensure!(
        std::fs::symlink_metadata(path)
            .context("original lifecycle journal unavailable")?
            .file_type()
            .is_file(),
        "lifecycle source must be a regular canonical file"
    );
    let path = path
        .canonicalize()
        .context("resolving original lifecycle journal")?;
    let base = corral_core::agent::paths::container_sessions_dir(
        store,
        &corral_core::agent::ContainerRef::Assistant {
            session: assistant.into(),
        },
    )
    .canonicalize()
    .context("canonical source directory unavailable")?;
    ensure!(
        path.starts_with(&base) && path.extension().is_some_and(|e| e == "jsonl"),
        "source is not the original canonical assistant journal"
    );
    let file = File::open(&path).context("opening original lifecycle journal")?;
    ensure!(
        file.metadata()?.is_file(),
        "lifecycle source is no longer regular"
    );
    let mut reader = BufReader::new(file);
    let mut scanned = 0usize;
    let mut header = None;
    let mut parents = BTreeMap::new();
    let mut entries = Vec::new();
    let mut evidence = facts::Evidence {
        version: 1,
        store: container.state.binding.store.clone(),
        project: container.state.binding.project.clone(),
        assistant: assistant.into(),
        generation: row.generation,
        session_id: bound.session_id.clone(),
        admission: row.id.clone(),
        tool: row.tool.clone(),
        instances: BTreeMap::new(),
        sealed: false,
    };
    loop {
        let mut bytes = Vec::new();
        if reader
            .by_ref()
            .take(1024 * 1024 + 1)
            .read_until(b'\n', &mut bytes)?
            == 0
        {
            break;
        }
        scanned += bytes.len();
        ensure!(
            scanned <= 64 * 1024 * 1024 && bytes.len() <= 1024 * 1024 && bytes.ends_with(b"\n"),
            "lifecycle journal truncated/oversized"
        );
        let bytes = String::from_utf8(bytes).context("invalid lifecycle journal UTF-8")?;
        let value: Value =
            serde_json::from_str(&bytes).context("incomplete lifecycle journal entry")?;
        if header.is_none() {
            ensure!(
                value["type"] == "session"
                    && value["id"] == bound.session_id
                    && value["version"] == 3,
                "lifecycle source session/version mismatch"
            );
            header = Some(facts::digest(bytes.as_bytes()));
            continue;
        }
        let id = value["id"]
            .as_str()
            .context("lifecycle source entry ID missing")?
            .to_string();
        let parent = match &value["parentId"] {
            Value::Null => None,
            Value::String(p) => Some(p.clone()),
            _ => anyhow::bail!("lifecycle journal parent missing"),
        };
        ensure!(
            parents.insert(id.clone(), parent).is_none() && parents.len() <= 65_536,
            "ambiguous/oversized lifecycle parent chain"
        );
        if value["type"] != "custom"
            || value["customType"] != "corral_effect_lifecycle"
            || value["data"]["admission"] != row.id
        {
            continue;
        }
        let fact: Fact = serde_json::from_value(value["data"].clone())
            .context("unsupported retained lifecycle schema")?;
        ensure!(
            fact.version == 1
                && fact.assistant == assistant
                && fact.generation == row.generation
                && fact.session_id == bound.session_id
                && fact.admission == row.id,
            "lifecycle source/generation/admission mismatch"
        );
        facts::transition(&mut evidence, fact.event)?;
        entries.push(SourceEntry {
            id,
            sha256: facts::digest(bytes.as_bytes()),
            bytes,
        });
        ensure!(
            entries.len() <= 513
                && entries.iter().map(|e| e.bytes.len()).sum::<usize>() <= 512 * 1024,
            "selected lifecycle proof exceeds bound"
        );
    }
    let mut cursor = entries.last().map(|e| e.id.clone());
    let mut chain = Vec::new();
    while let Some(id) = cursor {
        ensure!(
            chain.len() < 16_384 && !chain.contains(&id),
            "ambiguous lifecycle parent chain"
        );
        cursor = parents
            .get(&id)
            .context("lifecycle parent unavailable")?
            .clone();
        chain.push(id);
    }
    ensure!(
        entries.iter().all(|e| chain.contains(&e.id)),
        "lifecycle evidence spans abandoned/ambiguous branches"
    );
    evidence.validate_target(store, assistant, container, row.generation, row)?;
    if let Some((canonical, _)) = &canonical {
        require_prefix(canonical, &evidence)?;
    }
    let proof = JournalProof {
        version: 1,
        source: path.strip_prefix(base)?.to_string_lossy().into_owned(),
        header_sha256: header.context("lifecycle header missing")?,
        evidence,
        canonical_version: canonical.map(|(_, v)| v),
        entries,
    };
    proof.validate()?;
    Ok(proof)
}
