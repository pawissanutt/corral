//! Setup validates each exact execution candidate, not just a harness's flag map.
//!
//! Every semantic and cross-reference problem is collected in one pass and reported with
//! the JSON path it belongs to, together with any structural problems of the same input.
use std::collections::BTreeSet;
use std::fmt;

use anyhow::Result;
use corral_core::{
    AccessFacets, CandidateCost, Config, ExecutionCandidate, JsonPath, ModelEvidence, PathProblems,
    ProfileSetupProposal, SetupBasis, SetupValidationFacets,
};
use serde::Serialize;

use crate::setup::SetupProposalInput;
use crate::setup_structure::SetupInputDraft;

/// One rejected field or cross-reference, addressed by a JSONPath-style location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SetupValidationError {
    pub path: String,
    pub message: String,
}

/// All problems found in one validation pass. Non-empty whenever it is returned as an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupValidationErrors(pub Vec<SetupValidationError>);

impl fmt::Display for SetupValidationErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "setup validation found {} problem(s)", self.0.len())?;
        for error in &self.0 {
            write!(f, "; {}: {}", error.path, error.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for SetupValidationErrors {}

/// Combine structural and semantic problems. Semantic verdicts that relied on a node
/// repaired only to continue decoding are dropped; any structural problem still rejects.
pub(crate) fn finish<T>(
    semantic: PathProblems,
    draft: Option<&SetupInputDraft>,
    value: T,
) -> Result<T> {
    let structural = draft.into_iter().flat_map(|draft| &draft.notes.problems);
    let semantic = semantic
        .0
        .into_iter()
        .filter(|problem| draft.is_none_or(|draft| !draft.relies_on_repair(problem)))
        .map(|problem| {
            let path = draft.map_or_else(|| problem.path.clone(), |d| d.original(&problem.path));
            (path, problem.message)
        });
    let mut errors: Vec<SetupValidationError> = Vec::new();
    for (path, message) in structural.cloned().chain(semantic) {
        let error = SetupValidationError {
            path: path.to_string(),
            message,
        };
        if !errors.contains(&error) {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(value)
    } else {
        Err(SetupValidationErrors(errors).into())
    }
}

pub(crate) fn validate(
    input: &SetupProposalInput,
    config: &Config,
) -> Result<SetupValidationFacets> {
    let mut problems = PathProblems::default();
    let facets = collect(input, config, &mut problems);
    finish(problems, None, facets)
}

/// Record every semantic and cross-reference problem; facets are meaningful only when none.
pub(crate) fn collect(
    input: &SetupProposalInput,
    config: &Config,
    problems: &mut PathProblems,
) -> SetupValidationFacets {
    let proposal = &input.proposal;
    let proposal_path = JsonPath::default().key("proposal");
    let discovery_path = JsonPath::default().key("discovery");
    let prepared_at = proposal_path.key("prepared_at");
    proposal.collect_problems(config, &proposal_path, problems);
    input.discovery.collect_problems(&discovery_path, problems);
    problems.require(
        !matches!(proposal.basis, SetupBasis::Manual),
        &proposal_path.key("basis"),
        &[],
        "manual configuration uses config editing, not evidence-approved setup",
    );
    problems.require(
        input.discovery.observed_at <= proposal.prepared_at,
        &discovery_path.key("observed_at"),
        &[&prepared_at],
        "discovery postdates proposal",
    );
    for (id, evidence) in &proposal.evidence {
        let path = proposal_path.key("evidence").key(id);
        check_evidence(evidence, proposal, &path, problems);
    }
    let mut access_checked = true;
    let mut execution_tested = true;
    for (key, profile) in &proposal.profiles {
        let metadata_path = proposal_path.key("profiles").key(key).key("metadata");
        let Some(metadata) = profile.metadata.as_ref() else {
            continue;
        };
        problems.require(
            !metadata.candidates.is_empty(),
            &metadata_path.key("candidates"),
            &[],
            format!("profile '{key}' needs an exact execution candidate"),
        );
        if let Some(previous) = config.profiles.get(key)
            && previous != profile
            && let Some(old_metadata) = &previous.metadata
        {
            problems.require(
                old_metadata.revision != metadata.revision,
                &metadata_path.key("revision"),
                &[],
                format!("changed profile '{key}' needs a new revision"),
            );
        }
        for (index, candidate) in metadata.candidates.iter().enumerate() {
            let path = metadata_path.key("candidates").index(index);
            let facets = check_candidate(input, config, key, candidate, &path, problems);
            access_checked &= facets.access_verified;
            execution_tested &= facets.execution_tested;
        }
    }
    SetupValidationFacets {
        configuration_valid: true,
        access_checked,
        execution_tested,
    }
}

fn check_candidate(
    input: &SetupProposalInput,
    config: &Config,
    key: &str,
    candidate: &ExecutionCandidate,
    path: &JsonPath,
    problems: &mut PathProblems,
) -> AccessFacets {
    let proposal = &input.proposal;
    let name = format!("{key}/{}", candidate.id);
    let proposal_path = JsonPath::default().key("proposal");
    let discovery_path = JsonPath::default().key("discovery");
    check_arguments(config, candidate, path, problems);
    check_preferences(proposal, candidate, path, problems);

    // Each reference verdict depends on the identity fields it compares, not on the whole
    // candidate, so a malformed argument or facet cannot hide an independent failure.
    let identity = ["harness", "connection", "provider", "model"];
    let candidate_identity: Vec<_> = identity.iter().map(|field| path.key(field)).collect();
    let candidate_id = path.key("id");
    let access_path = proposal_path.key("access");
    let access_identity = |index: usize| {
        let record = access_path.index(index);
        [record.key("profile"), record.key("candidate")]
    };
    let matching: Vec<_> = proposal
        .access
        .iter()
        .enumerate()
        .filter(|(_, access)| access.profile == key && access.candidate == candidate.id)
        .collect();
    problems.require_reading(
        matching.len() == 1,
        path,
        std::iter::once(candidate_id.clone())
            .chain((0..proposal.access.len()).flat_map(access_identity))
            .collect(),
        vec![access_path.clone()],
        format!(
            "candidate '{name}' needs one independent access record; found {}",
            matching.len()
        ),
    );
    let access = match matching.as_slice() {
        [one] => Some(*one),
        _ => None,
    };
    if let Some((index, access)) = access {
        problems.require(
            !access.evidence.is_empty(),
            &access_path.index(index).key("evidence"),
            &[],
            format!("access record for '{name}' cites no discovery evidence"),
        );
    }

    let observations = discovery_path.key("candidates");
    let observation_identity = |index: usize| {
        let observation = observations.index(index);
        identity.map(|field| observation.key(field))
    };
    let found = input
        .discovery
        .candidates
        .iter()
        .enumerate()
        .find(|(_, o)| {
            o.harness == candidate.harness
                && o.connection == candidate.connection
                && o.provider == candidate.provider
                && o.model == candidate.model
        });
    let Some((observed_index, observation)) = found else {
        problems.require_reading(
            false,
            path,
            candidate_identity
                .iter()
                .cloned()
                .chain((0..input.discovery.candidates.len()).flat_map(observation_identity))
                .collect(),
            vec![observations.clone()],
            format!(
                "no independent discovery for candidate '{name}' (matching harness, connection, provider and model)"
            ),
        );
        return AccessFacets::default();
    };
    let observation_path = observations.index(observed_index);
    let matched: Vec<JsonPath> = candidate_identity
        .iter()
        .cloned()
        .chain(observation_identity(observed_index))
        .collect();
    let reading = |extra: &[&JsonPath]| -> Vec<JsonPath> {
        matched
            .iter()
            .cloned()
            .chain(extra.iter().map(|node| (*node).clone()))
            .collect()
    };
    let facets = &observation.facets;
    let facets_path = observation_path.key("facets");
    problems.require_reading(
        facets.installed
            && (facets.discovered || facets.access_verified)
            && (facets.user_reported || facets.access_verified),
        &facets_path,
        reading(&[&facets_path]),
        Vec::new(),
        format!("candidate '{name}' lacks installed, model and connection access observations"),
    );
    problems.require_reading(
        observation
            .settings
            .iter()
            .any(|settings| settings.effort == candidate.effort && settings.args == candidate.args),
        path,
        reading(&[
            &path.key("effort"),
            &path.key("args"),
            &observation_path.key("settings"),
        ]),
        Vec::new(),
        format!("unsupported exact model settings for '{name}'"),
    );
    let observed_evidence: BTreeSet<_> = observation.evidence.iter().map(|item| &item.id).collect();
    let observed_evidence_path = observation_path.key("evidence");
    let evidence_ids: Vec<_> = (0..observation.evidence.len())
        .map(|item| observed_evidence_path.index(item).key("id"))
        .collect();
    if let Some((index, access)) = access {
        let record = access_path.index(index);
        let linked: Vec<_> = reading(&[&candidate_id])
            .into_iter()
            .chain(access_identity(index))
            .collect();
        let with = |extra: &[&JsonPath]| -> Vec<JsonPath> {
            linked
                .iter()
                .cloned()
                .chain(extra.iter().map(|node| (*node).clone()))
                .collect()
        };
        let access_facets = record.key("facets");
        problems.require_reading(
            access.facets == observation.facets,
            &access_facets,
            with(&[&access_facets, &facets_path]),
            Vec::new(),
            format!("access facets for '{name}' differ from discovery {observation_path}"),
        );
        let observed_at = record.key("observed_at");
        problems.require_reading(
            access.observed_at == observation.observed_at,
            &observed_at,
            with(&[&observed_at, &observation_path.key("observed_at")]),
            Vec::new(),
            format!("access timestamp for '{name}' differs from discovery {observation_path}"),
        );
        for (item, id) in access.evidence.iter().enumerate() {
            let item_path = record.key("evidence").index(item);
            problems.require_reading(
                observed_evidence.contains(id),
                &item_path,
                with(&[&item_path])
                    .into_iter()
                    .chain(evidence_ids.iter().cloned())
                    .collect(),
                vec![observed_evidence_path.clone()],
                format!("access evidence '{id}' is not in discovery {observation_path}"),
            );
        }
    }
    let candidate_evidence = path.key("evidence");
    problems.require_reading(
        observed_evidence.contains(&candidate.evidence),
        &candidate_evidence,
        reading(&[&candidate_evidence])
            .into_iter()
            .chain(evidence_ids.iter().cloned())
            .collect(),
        vec![observed_evidence_path.clone()],
        format!(
            "candidate evidence '{}' is not in discovery {observation_path}",
            candidate.evidence
        ),
    );
    for (item, evidence) in observation.evidence.iter().enumerate() {
        let path = observed_evidence_path.index(item);
        check_evidence(evidence, proposal, &path, problems);
        problems.require(
            proposal.evidence.get(&evidence.id) == Some(evidence),
            &path,
            &[&proposal_path.key("evidence").key(&evidence.id)],
            "discovery evidence differs from exact proposal",
        );
    }
    problems.require(
        observation.observed_at <= input.discovery.observed_at
            && observation.observed_at <= proposal.prepared_at,
        &observation_path.key("observed_at"),
        &[
            &discovery_path.key("observed_at"),
            &proposal_path.key("prepared_at"),
        ],
        "candidate observation postdates discovery or proposal",
    );
    observation.facets.clone()
}

/// An extra argv override must not change the separately approved binding.
fn check_arguments(
    config: &Config,
    candidate: &ExecutionCandidate,
    path: &JsonPath,
    problems: &mut PathProblems,
) {
    let harness_path = path.key("harness");
    let flags = match config.harness(&candidate.harness) {
        Ok((_, flags)) => flags,
        Err(err) => {
            problems.error(&harness_path, &[], &err);
            return;
        }
    };
    let binding_flags = [
        Some("--model"),
        Some("--thinking"),
        Some("--effort"),
        Some("-m"),
        flags.model_flag.as_deref(),
        flags.effort_flag.as_deref(),
    ];
    problems.require(
        !candidate.args.iter().any(|arg| {
            binding_flags.into_iter().flatten().any(|flag| {
                arg == flag
                    || arg.starts_with(&format!("{flag}="))
                    || (flag.starts_with('-') && !flag.starts_with("--") && arg.starts_with(flag))
            })
        }),
        &path.key("args"),
        &[&harness_path],
        "candidate arguments override approved model or effort",
    );
}

fn check_preferences(
    proposal: &ProfileSetupProposal,
    candidate: &ExecutionCandidate,
    path: &JsonPath,
    problems: &mut PathProblems,
) {
    let preferences = &proposal.preferences;
    let preferences_path = JsonPath::default().key("proposal").key("preferences");
    problems.require(
        preferences.allowed_providers.is_empty()
            || preferences.allowed_providers.contains(&candidate.provider),
        &path.key("provider"),
        &[&preferences_path],
        "candidate provider is outside setup preferences",
    );
    if let Some(bound) = &preferences.spending {
        match &candidate.cost {
            CandidateCost::Metered {
                currency,
                maximum_micros,
            } => problems.require(
                currency == &bound.currency && maximum_micros <= &bound.maximum_micros,
                &path.key("cost"),
                &[&preferences_path],
                "candidate exceeds setup spending bound",
            ),
            CandidateCost::Unknown | CandidateCost::Subscription => problems.push(
                &path.key("cost"),
                &[&preferences_path],
                "unknown cost cannot satisfy setup spending bound",
            ),
        }
    }
}

fn check_evidence(
    evidence: &ModelEvidence,
    proposal: &ProfileSetupProposal,
    path: &JsonPath,
    problems: &mut PathProblems,
) {
    let prepared_at = JsonPath::default().key("proposal").key("prepared_at");
    problems.require(
        evidence.retrieved_at <= proposal.prepared_at,
        &path.key("retrieved_at"),
        &[&prepared_at],
        "evidence retrieval postdates proposal",
    );
    let retrieved_on = evidence
        .retrieved_at
        .to_zoned(jiff::tz::TimeZone::UTC)
        .date();
    problems.require(
        evidence
            .published_on
            .is_none_or(|date| date <= retrieved_on)
            && evidence.measured_on.is_none_or(|date| date <= retrieved_on),
        path,
        &[],
        "evidence publication/measurement postdates retrieval",
    );
}
