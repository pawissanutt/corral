//! Source selection shared by complete settlement and persisted app accounting.
use super::*;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

#[derive(Serialize)]
pub(crate) struct Input {
    pub session: String,
    pub model: Option<String>,
    pub closed: bool,
    pub owners: Vec<u32>,
}

#[derive(Serialize)]
pub(crate) struct Inputs {
    pub files: BTreeMap<PathBuf, Input>,
    pub bindings: BTreeMap<u32, corral_core::connection::ConnectionBinding>,
}

pub(crate) fn inputs(state: &AgentRunState, model: Option<&str>, store: &Store) -> Result<Inputs> {
    let retired = match &state.retired {
        Some(_) if state.binding.role == AgentRole::Pm => pm_state::retired_generations(store)?,
        _ => Vec::new(),
    };
    let mut bound: Vec<_> = state
        .generations
        .iter()
        .filter_map(|g| {
            let b = g.bound.as_ref()?;
            Some((
                g.n,
                b.session_file.clone(),
                b.session_id.clone(),
                g.fenced.is_some(),
            ))
        })
        .chain(
            retired
                .iter()
                .filter_map(|r| Some((r.n, r.session_file.clone()?, r.session_id.clone()?, true))),
        )
        .collect();
    bound.sort_by_key(|(n, ..)| *n);
    let mut files: BTreeMap<PathBuf, Input> = BTreeMap::new();
    let latest = bound.last().map(|(_, file, ..)| file.clone());
    for (n, file, session, closed) in bound {
        let input = files.entry(file).or_insert_with(|| Input {
            session,
            model: model.map(str::to_owned),
            closed,
            owners: Vec::new(),
        });
        input.closed &= closed;
        input.owners.push(n);
    }
    for session in &state.usage_sessions {
        if state.generation(session.generation).is_none()
            && !state.generation_retired(session.generation)
        {
            bail!(
                "usage session references absent generation {}",
                session.generation
            );
        }
        let generation_ended = state.generation_retired(session.generation)
            || state
                .generation(session.generation)
                .is_some_and(|g| g.fenced.is_some());
        let closed = matches!(
            session.outcome,
            Some(agent::AuxiliaryOutcome::Completed | agent::AuxiliaryOutcome::NotStarted)
        ) || generation_ended;
        let input = files
            .entry(session.session_file.clone())
            .or_insert_with(|| Input {
                session: session.session_id.clone(),
                model: session.model.clone(),
                closed,
                owners: Vec::new(),
            });
        input.closed &= closed;
        if !input.owners.contains(&session.generation) {
            input.owners.push(session.generation);
        }
    }
    // Launch and rewind may resume the last bound file even after its host is fenced.
    if let Some(input) = latest.and_then(|path| files.get_mut(&path)) {
        input.closed = false;
    }
    let bindings: BTreeMap<_, _> = retired
        .iter()
        .filter_map(|r| r.connection.clone().map(|b| (r.n, b)))
        .chain(
            state
                .generations
                .iter()
                .filter_map(|g| g.connection.clone().map(|b| (g.n, b))),
        )
        .collect();
    for binding in bindings.values() {
        binding.validate()?;
    }
    Ok(Inputs { files, bindings })
}

pub(crate) struct Entry {
    pub key: String,
    pub session: sha2::digest::Output<Sha256>,
    pub path: PathBuf,
    pub usage: UsageEntry,
}

/// Keep only compact response identities outside the collector. Full response rows are
/// transient, including on the one-time complete collection of closed files.
pub(crate) fn collect(
    inputs: &Inputs,
    max: Option<usize>,
) -> Result<(Vec<Entry>, UsageReadCoverage)> {
    collect_files(inputs.files.iter(), &inputs.bindings, max)
}

pub(crate) fn collect_files<'a>(
    files: impl IntoIterator<Item = (&'a PathBuf, &'a Input)>,
    bindings: &BTreeMap<u32, corral_core::connection::ConnectionBinding>,
    max: Option<usize>,
) -> Result<(Vec<Entry>, UsageReadCoverage)> {
    let mut entries = BTreeMap::new();
    let mut coverage = UsageReadCoverage::new(max);
    coverage.observational = true;
    coverage.unlimited_bytes = max.is_none();
    for (path, input) in files {
        if !path
            .try_exists()
            .with_context(|| format!("checking usage source {}", path.display()))?
        {
            coverage.partial = true;
            coverage.reason = Some("usage_source_unavailable");
            continue;
        }
        let mut found = BTreeMap::new();
        read_session_with_turns(
            path,
            &input.session,
            input.model.as_deref(),
            &mut found,
            false,
            bindings,
            &mut coverage,
        )?;
        for (key, entry) in found {
            entries.entry(key).or_insert_with(|| (path.clone(), entry));
        }
        if coverage.reason == Some("transcript_row_limit") {
            break;
        }
    }
    let mut session_hashes = BTreeMap::new();
    let entries = entries
        .into_iter()
        .map(|((session, response), (path, entry))| {
            let mut hash = Sha256::new();
            hash.update((session.len() as u64).to_le_bytes());
            hash.update(session.as_bytes());
            hash.update(response.as_bytes());
            let session = *session_hashes
                .entry(session)
                .or_insert_with_key(|session| Sha256::digest(session.as_bytes()));
            Entry {
                path,
                session,
                key: format!("{:x}", hash.finalize()),
                usage: entry.usage,
            }
        })
        .collect();
    Ok((entries, coverage))
}
