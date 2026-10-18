//! Setup validates each exact execution candidate, not just a harness's flag map.
use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use corral_core::{
    CandidateCost, Config, ExecutionCandidate, ModelEvidence, ProfileSetupProposal, SetupBasis,
    SetupValidationFacets,
};

use crate::setup::SetupProposalInput;
use crate::setup_discovery::CandidateObservation;

pub(crate) fn validate(
    input: &SetupProposalInput,
    config: &Config,
) -> Result<SetupValidationFacets> {
    let proposal = &input.proposal;
    proposal.validate(config)?;
    input.discovery.validate()?;
    ensure!(
        !matches!(proposal.basis, SetupBasis::Manual),
        "manual configuration uses config editing, not evidence-approved setup"
    );
    ensure!(
        input.discovery.observed_at <= proposal.prepared_at,
        "discovery postdates proposal"
    );
    for evidence in proposal.evidence.values() {
        check_evidence(evidence, proposal)?;
    }
    let mut checked_access = true;
    let mut execution_tested = true;
    for (key, profile) in &proposal.profiles {
        let metadata = profile
            .metadata
            .as_ref()
            .context("missing profile metadata")?;
        ensure!(
            !metadata.candidates.is_empty(),
            "profile '{key}' needs an exact execution candidate"
        );
        if let Some(previous) = config.profiles.get(key)
            && previous != profile
            && let Some(old_metadata) = &previous.metadata
        {
            ensure!(
                old_metadata.revision != metadata.revision,
                "changed profile '{key}' needs a new revision"
            );
        }
        for candidate in &metadata.candidates {
            let observation = observation(input, candidate)?;
            ensure!(
                observation.facets.installed
                    && (observation.facets.discovered || observation.facets.access_verified)
                    && (observation.facets.user_reported || observation.facets.access_verified),
                "candidate '{key}/{}' lacks installed, model and connection access observations",
                candidate.id
            );
            ensure!(
                observation.settings.iter().any(|settings| {
                    settings.effort == candidate.effort && settings.args == candidate.args
                }),
                "unsupported exact model settings for '{key}/{}'",
                candidate.id
            );
            // An extra argv override must not change the separately approved binding.
            let (_, flags) = config.harness(&candidate.harness)?;
            let binding_flags = [
                Some("--model"),
                Some("--thinking"),
                Some("--effort"),
                Some("-m"),
                flags.model_flag.as_deref(),
                flags.effort_flag.as_deref(),
            ];
            ensure!(
                !candidate.args.iter().any(|arg| {
                    binding_flags.into_iter().flatten().any(|flag| {
                        arg == flag
                            || arg.starts_with(&format!("{flag}="))
                            || (flag.starts_with('-')
                                && !flag.starts_with("--")
                                && arg.starts_with(flag))
                    })
                }),
                "candidate arguments override approved model or effort"
            );
            let access: Vec<_> = proposal
                .access
                .iter()
                .filter(|access| access.profile == *key && access.candidate == candidate.id)
                .collect();
            ensure!(
                access.len() == 1,
                "candidate needs one independent access record"
            );
            let access = access[0];
            ensure!(
                access.facets == observation.facets
                    && access.observed_at == observation.observed_at
                    && !access.evidence.is_empty(),
                "candidate access differs from discovery"
            );
            let observed_evidence: BTreeSet<_> =
                observation.evidence.iter().map(|item| &item.id).collect();
            ensure!(
                access
                    .evidence
                    .iter()
                    .all(|id| observed_evidence.contains(id))
                    && observed_evidence.contains(&candidate.evidence),
                "candidate support/access lacks its discovery evidence"
            );
            for evidence in &observation.evidence {
                check_evidence(evidence, proposal)?;
                ensure!(
                    proposal.evidence.get(&evidence.id) == Some(evidence),
                    "discovery evidence differs from exact proposal"
                );
            }
            ensure!(
                observation.observed_at <= input.discovery.observed_at
                    && observation.observed_at <= proposal.prepared_at,
                "candidate observation postdates discovery or proposal"
            );
            if !proposal.preferences.allowed_providers.is_empty() {
                ensure!(
                    proposal
                        .preferences
                        .allowed_providers
                        .contains(&candidate.provider),
                    "candidate provider is outside setup preferences"
                );
            }
            if let Some(bound) = &proposal.preferences.spending {
                match &candidate.cost {
                    CandidateCost::Metered {
                        currency,
                        maximum_micros,
                    } => ensure!(
                        currency == &bound.currency && maximum_micros <= &bound.maximum_micros,
                        "candidate exceeds setup spending bound"
                    ),
                    CandidateCost::Unknown | CandidateCost::Subscription => {
                        anyhow::bail!("unknown cost cannot satisfy setup spending bound")
                    }
                }
            }
            checked_access &= observation.facets.access_verified;
            execution_tested &= observation.facets.execution_tested;
        }
    }
    Ok(SetupValidationFacets {
        configuration_valid: true,
        access_checked: checked_access,
        execution_tested,
    })
}

fn observation<'a>(
    input: &'a SetupProposalInput,
    candidate: &ExecutionCandidate,
) -> Result<&'a CandidateObservation> {
    input
        .discovery
        .candidates
        .iter()
        .find(|observation| {
            observation.harness == candidate.harness
                && observation.connection == candidate.connection
                && observation.provider == candidate.provider
                && observation.model == candidate.model
        })
        .with_context(|| format!("no independent discovery for candidate '{}'", candidate.id))
}

fn check_evidence(evidence: &ModelEvidence, proposal: &ProfileSetupProposal) -> Result<()> {
    ensure!(
        evidence.retrieved_at <= proposal.prepared_at,
        "evidence retrieval postdates proposal"
    );
    let retrieved_on = evidence
        .retrieved_at
        .to_zoned(jiff::tz::TimeZone::UTC)
        .date();
    ensure!(
        evidence
            .published_on
            .is_none_or(|date| date <= retrieved_on)
            && evidence.measured_on.is_none_or(|date| date <= retrieved_on),
        "evidence publication/measurement postdates retrieval"
    );
    Ok(())
}
