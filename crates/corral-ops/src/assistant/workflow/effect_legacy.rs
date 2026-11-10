//! Narrow retained corral_command verifier. Canonical original-host files only; no supplied
//! excerpts, guessed process identities, process operations, whole-file exports or replay.
use super::{boundary, facts};
use anyhow::{Context, Result, ensure};
use corral_core::{Store, assistant::AssistantContainer};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufRead, BufReader, Read},
};

const LINE_MAX: usize = 1024 * 1024;
const SCAN_MAX: usize = 64 * 1024 * 1024;
const PROOF_MAX: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceEntry {
    pub id: String,
    pub sha256: String,
    pub bytes: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyProof {
    pub version: u32,
    pub assistant: String,
    pub store: String,
    pub generation: u32,
    pub session_id: String,
    pub source: String,
    pub admission: String,
    pub tool_call: String,
    pub job: String,
    pub exit_code: i64,
    pub group_clear: bool,
    pub consumed: bool,
    pub entries: Vec<SourceEntry>,
    pub chain: Vec<String>,
}
impl LegacyProof {
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1
                && self.group_clear
                && self.consumed
                && self.generation > 0
                && !self.tool_call.is_empty()
                && self.job.starts_with(&format!("j{}-", self.generation))
                && !self.entries.is_empty()
                && self.entries.iter().map(|e| e.bytes.len()).sum::<usize>() <= PROOF_MAX
                && self
                    .entries
                    .iter()
                    .all(|e| facts::digest(e.bytes.as_bytes()) == e.sha256),
            "incomplete/corrupt legacy command proof"
        );
        Ok(())
    }
}

fn ancestor(
    parents: &BTreeMap<String, Option<String>>,
    from: &str,
    target: &str,
) -> Result<Vec<String>> {
    let mut chain = Vec::new();
    let mut cursor = Some(from.to_string());
    while let Some(id) = cursor {
        ensure!(
            chain.len() < 16_384 && !chain.contains(&id),
            "ambiguous/cyclic retained parent chain"
        );
        chain.push(id.clone());
        if id == target {
            return Ok(chain);
        }
        cursor = parents
            .get(&id)
            .context("retained parent entry unavailable")?
            .clone();
    }
    anyhow::bail!("retained command is not on the exact model-call parent chain")
}

fn complete_ancestry(parents: &BTreeMap<String, Option<String>>, from: &str) -> Result<()> {
    let mut cursor = Some(from.to_string());
    let mut seen = BTreeSet::new();
    while let Some(id) = cursor {
        ensure!(
            seen.len() < 16_384 && seen.insert(id.clone()),
            "ambiguous/cyclic original parent chain"
        );
        cursor = parents.get(&id).context("original conversation parent chain is incomplete; excerpts cannot prove settlement")?.clone();
    }
    Ok(())
}

pub(super) fn verify(
    store: &Store,
    assistant: &str,
    container: &AssistantContainer,
    row: &boundary::Activity,
) -> Result<LegacyProof> {
    ensure!(
        row.tool == "bash",
        "legacy verifier supports exact bash commands only"
    );
    let (_, call) = row
        .id
        .split_once(':')
        .context("legacy admission lacks exact tool-call identity")?;
    ensure!(!call.is_empty(), "legacy tool-call identity missing");
    let original = container
        .state
        .generations
        .iter()
        .find(|g| g.n == row.generation)
        .and_then(|g| g.bound.as_ref())
        .context("original host binding unavailable")?;
    let base = corral_core::agent::paths::container_sessions_dir(
        store,
        &corral_core::agent::ContainerRef::Assistant {
            session: assistant.into(),
        },
    );
    let meta = std::fs::symlink_metadata(&original.session_file)
        .context("canonical original conversation unavailable")?;
    ensure!(
        meta.file_type().is_file(),
        "legacy evidence must be a regular canonical file (no symlinks/devices)"
    );
    let canonical = original
        .session_file
        .canonicalize()
        .context("resolving canonical original conversation")?;
    let base = base
        .canonicalize()
        .context("canonical assistant session directory unavailable")?;
    ensure!(
        canonical.starts_with(&base) && canonical.extension().is_some_and(|ext| ext == "jsonl"),
        "source is not the recorded canonical assistant conversation"
    );
    let file = File::open(&canonical).context("opening retained original conversation")?;
    ensure!(
        file.metadata()?.is_file(),
        "legacy source changed to a nonregular file"
    );
    let mut reader = BufReader::new(file);
    let mut scanned = 0usize;
    let mut parents = BTreeMap::new();
    let mut ids = BTreeSet::new();
    let mut header = None;
    let mut model = None;
    let mut candidates: BTreeMap<String, Vec<(SourceEntry, Value)>> = BTreeMap::new();
    let mut results = Vec::new();
    loop {
        let mut line = Vec::new();
        let count = reader
            .by_ref()
            .take((LINE_MAX + 1) as u64)
            .read_until(b'\n', &mut line)
            .context("streaming bounded legacy entry")?;
        if count == 0 {
            break;
        }
        scanned += count;
        ensure!(
            scanned <= SCAN_MAX && line.len() <= LINE_MAX && line.ends_with(b"\n"),
            "legacy evidence scan/entry truncated or incomplete"
        );
        let bytes = String::from_utf8(line).context("invalid UTF-8 legacy evidence")?;
        let value: Value =
            serde_json::from_str(&bytes).context("incomplete/corrupt retained journal entry")?;
        let kind = value["type"]
            .as_str()
            .context("journal entry type missing")?;
        if header.is_none() {
            ensure!(
                kind == "session" && value["id"] == original.session_id && value["version"] == 3,
                "legacy source session/version mismatch"
            );
            header = Some(SourceEntry {
                id: original.session_id.clone(),
                sha256: facts::digest(bytes.as_bytes()),
                bytes,
            });
            continue;
        }
        let id = value["id"]
            .as_str()
            .context("journal entry ID missing")?
            .to_string();
        ensure!(
            ids.insert(id.clone()) && ids.len() <= 65_536,
            "duplicate/oversized journal identity set"
        );
        let parent = match &value["parentId"] {
            Value::Null => None,
            Value::String(p) => Some(p.clone()),
            _ => anyhow::bail!("journal parent missing"),
        };
        parents.insert(id.clone(), parent);
        let source = || SourceEntry {
            id: id.clone(),
            sha256: facts::digest(bytes.as_bytes()),
            bytes: bytes.clone(),
        };
        if kind == "message" && value["message"]["role"] == "assistant" {
            let calls: Vec<_> = value["message"]["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|v| v["type"] == "toolCall")
                .collect();
            if calls.iter().any(|v| v["id"] == call) {
                // A legacy corral_command has no invocation ID. Without an unambiguous direct
                // single-call source, a summary or sibling result cannot manufacture linkage.
                ensure!(
                    model.is_none() && calls.len() == 1 && calls[0]["name"] == "bash",
                    "ambiguous/nested legacy executor linkage requires authoritative metadata"
                );
                model = Some(source());
            }
        }
        if kind == "custom"
            && value["customType"] == "corral_command"
            && value["data"]["generation"] == row.generation
            && model.is_some()
            && results.is_empty()
        {
            let job = value["data"]["job"]
                .as_str()
                .context("command journal job missing")?;
            if job.starts_with(&format!("j{}-", row.generation)) {
                candidates
                    .entry(job.into())
                    .or_default()
                    .push((source(), value["data"].clone()));
            }
        }
        if kind == "message"
            && value["message"]["role"] == "toolResult"
            && value["message"]["toolCallId"] == call
        {
            ensure!(
                value["message"]["toolName"] == "bash",
                "legacy tool result contradicts the exact bash call identity"
            );
            results.push(source());
        }
        ensure!(
            candidates.len() <= 128
                && candidates.values().map(Vec::len).sum::<usize>() <= 1024
                && candidates
                    .values()
                    .flatten()
                    .map(|(e, _)| e.bytes.len())
                    .sum::<usize>()
                    <= PROOF_MAX,
            "legacy command evidence bound exceeded; select an immutable retained proof instead"
        );
    }
    let model =
        model.context("exact model bash call unavailable (nested/legacy identity unknown)")?;
    complete_ancestry(&parents, &model.id)?;
    ensure!(
        results.len() == 1,
        "exact retained tool result linkage missing/ambiguous"
    );
    let chain = ancestor(&parents, &results[0].id, &model.id)?;
    let mut linked = Vec::new();
    for (job, entries) in candidates {
        let own: Vec<_> = entries
            .into_iter()
            .filter(|(e, _)| chain.contains(&e.id) && ancestor(&parents, &e.id, &model.id).is_ok())
            .collect();
        if !own.is_empty() {
            linked.push((job, own));
        }
    }
    ensure!(
        linked.len() == 1,
        "missing/ambiguous legacy job-to-call linkage"
    );
    let (job, entries) = linked.pop().context("legacy linked job missing")?;
    let mut snapshots = vec![header.context("session header missing")?, model];
    let mut terminal = None;
    let mut consumed = false;
    for (entry, fact) in entries {
        ensure!(
            chain.contains(&entry.id),
            "command journal belongs to an abandoned/ambiguous branch"
        );
        let result = &fact["result"];
        if result.is_object() {
            ensure!(
                result["job"] == job
                    && result["state"] == "exited"
                    && result["group_clear"] == true,
                "legacy executor result unknown/incomplete/contradictory"
            );
            let code = result["exit_code"]
                .as_i64()
                .context("legacy command terminal exit missing")?;
            ensure!(
                terminal.is_none_or(|prior| prior == code),
                "contradictory legacy command exit"
            );
            terminal = Some(code);
        }
        if fact["consumed"] == true {
            ensure!(
                terminal.is_some()
                    && matches!(fact["delivery"].as_str(), Some("result" | "notice")),
                "consumed entry lacks terminal delivery linkage"
            );
            consumed = true;
        }
        snapshots.push(entry);
    }
    snapshots.extend(results);
    let proof = LegacyProof {
        version: 1,
        assistant: assistant.into(),
        store: container.state.binding.store.clone(),
        generation: row.generation,
        session_id: original.session_id.clone(),
        source: canonical
            .strip_prefix(&base)?
            .to_string_lossy()
            .into_owned(),
        admission: row.id.clone(),
        tool_call: call.into(),
        job,
        exit_code: terminal.context("legacy exit/group-clear evidence missing")?,
        group_clear: true,
        consumed,
        entries: snapshots,
        chain,
    };
    proof.validate()?;
    Ok(proof)
}
