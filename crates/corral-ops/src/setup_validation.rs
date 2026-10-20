//! Setup validates each exact execution candidate, not just a harness's flag map.
//!
//! Every structural and cross-reference problem is collected in one pass and reported with
//! the JSON path it belongs to, so an author can fix a proposal without one call per defect.
use std::collections::BTreeSet;
use std::fmt;

use anyhow::Result;
use corral_core::{
    AccessFacets, CandidateCost, Config, ExecutionCandidate, ModelEvidence, ProfileSetupProposal,
    SetupBasis, SetupValidationFacets,
};
use serde::Serialize;
use serde_json::Value;

use crate::setup::SetupProposalInput;

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

#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    Key(String),
    Index(usize),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct JsonPath(Vec<Seg>);

impl JsonPath {
    pub(crate) fn key(&self, key: &str) -> Self {
        let mut path = self.clone();
        path.0.push(Seg::Key(key.to_owned()));
        path
    }

    fn index(&self, index: usize) -> Self {
        let mut path = self.clone();
        path.0.push(Seg::Index(index));
        path
    }
}

impl fmt::Display for JsonPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("$")?;
        for seg in &self.0 {
            match seg {
                Seg::Key(key)
                    if !key.is_empty()
                        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') =>
                {
                    write!(f, ".{key}")?;
                }
                Seg::Key(key) => write!(f, "[{key:?}]")?,
                Seg::Index(index) => write!(f, "[{index}]")?,
            }
        }
        Ok(())
    }
}

/// Collects problems; a duplicate (same path and message) is reported once.
#[derive(Debug, Default)]
pub(crate) struct Problems(Vec<SetupValidationError>);

impl Problems {
    pub(crate) fn push(&mut self, path: &JsonPath, message: impl Into<String>) {
        let error = SetupValidationError {
            path: path.to_string(),
            message: message.into(),
        };
        if !self.0.contains(&error) {
            self.0.push(error);
        }
    }

    pub(crate) fn require(&mut self, ok: bool, path: &JsonPath, message: impl Into<String>) {
        if !ok {
            self.push(path, message);
        }
    }

    pub(crate) fn error(&mut self, path: &JsonPath, error: &anyhow::Error) {
        self.push(path, format!("{error:#}"));
    }

    pub(crate) fn finish<T>(self, value: T) -> Result<T> {
        if self.0.is_empty() {
            Ok(value)
        } else {
            Err(SetupValidationErrors(self.0).into())
        }
    }
}

const MAX_STRUCTURAL_PROBLEMS: usize = 64;
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

/// Decode setup input, reporting every missing, unknown or mistyped field at once.
///
/// After each serde error the offending node is repaired with a placeholder (or removed)
/// purely so decoding can continue; the input is rejected whenever any problem was found.
pub fn parse_setup_input(bytes: &[u8]) -> Result<SetupProposalInput> {
    let mut value: Value = serde_json::from_slice(bytes).map_err(|err| {
        SetupValidationErrors(vec![SetupValidationError {
            path: "$".into(),
            message: format!("invalid JSON: {err}"),
        }])
    })?;
    let mut problems = Problems::default();
    misplaced_profile_fields(&value, &mut problems);
    let mut removed: Vec<Vec<Seg>> = Vec::new();
    for _ in 0..MAX_STRUCTURAL_PROBLEMS {
        let problem = match decode(&value) {
            Ok(input) => return problems.finish(input),
            Err(problem) => problem,
        };
        problems.push(
            &JsonPath(original_path(&problem.path, &removed)),
            problem.message.clone(),
        );
        if !repair(&mut value, &problem, 0) && !discard(&mut value, &problem, &mut removed) {
            problems.push(
                &JsonPath::default(),
                "stopped checking structure after the problem above; fix it and validate again",
            );
            return Err(SetupValidationErrors(problems.0).into());
        }
    }
    problems.push(
        &JsonPath::default(),
        format!("stopped after {MAX_STRUCTURAL_PROBLEMS} structural problems"),
    );
    Err(SetupValidationErrors(problems.0).into())
}

/// Profiles ignore unknown keys in configuration files, so a metadata field written beside
/// `metadata` would vanish silently. Report it where the profile already lacks metadata.
fn misplaced_profile_fields(value: &Value, problems: &mut Problems) {
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
            problems.push(&base.key(key).key(field), message);
        }
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
    path: Vec<Seg>,
    kind: ProblemKind,
    message: String,
}

impl Problem {
    fn target(&self) -> Vec<Seg> {
        let mut target = self.path.clone();
        if let ProblemKind::Missing(field) = &self.kind {
            target.push(Seg::Key(field.clone()));
        }
        target
    }
}

fn decode(value: &Value) -> std::result::Result<SetupProposalInput, Problem> {
    serde_path_to_error::deserialize(value).map_err(|err| {
        let mut path = Vec::new();
        for seg in err.path().iter() {
            match seg {
                serde_path_to_error::Segment::Seq { index } => path.push(Seg::Index(*index)),
                serde_path_to_error::Segment::Map { key } => path.push(Seg::Key(key.clone())),
                serde_path_to_error::Segment::Enum { variant } => {
                    path.push(Seg::Key(variant.clone()));
                }
                serde_path_to_error::Segment::Unknown => break,
            }
        }
        let message = err.inner().to_string();
        let kind = if let Some(field) = quoted_after(&message, "unknown field `") {
            // Point at the stray field itself rather than its containing object.
            if node(value, &path)
                .and_then(Value::as_object)
                .is_some_and(|object| object.contains_key(&field))
            {
                path.push(Seg::Key(field.clone()));
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

fn quoted_after(message: &str, prefix: &str) -> Option<String> {
    let rest = message.strip_prefix(prefix)?;
    rest.find('`').map(|end| rest[..end].to_owned())
}

fn node<'a>(value: &'a Value, path: &[Seg]) -> Option<&'a Value> {
    path.iter().try_fold(value, |value, seg| match seg {
        Seg::Key(key) => value.as_object()?.get(key),
        Seg::Index(index) => value.as_array()?.get(*index),
    })
}

fn node_mut<'a>(value: &'a mut Value, path: &[Seg]) -> Option<&'a mut Value> {
    path.iter().try_fold(value, |value, seg| match seg {
        Seg::Key(key) => value.as_object_mut()?.get_mut(key),
        Seg::Index(index) => value.as_array_mut()?.get_mut(*index),
    })
}

fn set(value: &mut Value, path: &[Seg], replacement: Value) -> bool {
    let Some((last, parent)) = path.split_last() else {
        return false;
    };
    match (node_mut(value, parent), last) {
        (Some(Value::Object(object)), Seg::Key(key)) => {
            object.insert(key.clone(), replacement);
            true
        }
        (Some(Value::Array(items)), Seg::Index(index)) if *index < items.len() => {
            items[*index] = replacement;
            true
        }
        _ => false,
    }
}

fn remove(value: &mut Value, path: &[Seg]) -> bool {
    let Some((last, parent)) = path.split_last() else {
        return false;
    };
    match (node_mut(value, parent), last) {
        (Some(Value::Object(object)), Seg::Key(key)) => object.remove(key).is_some(),
        (Some(Value::Array(items)), Seg::Index(index)) if *index < items.len() => {
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

fn fill(value: &mut Value, target: &[Seg], depth: usize) -> bool {
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

fn settle(trial: &mut Value, target: &[Seg], depth: usize) -> bool {
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

/// Drop the nearest removable array element or map entry containing an unrepairable node.
fn discard(value: &mut Value, problem: &Problem, removed: &mut Vec<Vec<Seg>>) -> bool {
    let target = problem.target();
    for len in (1..=target.len()).rev() {
        let path = &target[..len];
        let mut trial = value.clone();
        if !remove(&mut trial, path) {
            continue;
        }
        let required = match (decode(&trial), &path[len - 1]) {
            (Err(next), Seg::Key(key)) => {
                next.path == path[..len - 1] && next.kind == ProblemKind::Missing(key.clone())
            }
            _ => false,
        };
        if required {
            continue;
        }
        if matches!(path[len - 1], Seg::Index(_)) {
            removed.push(original_path(path, removed));
        }
        *value = trial;
        return true;
    }
    false
}

/// Map a path in the repaired value back to the submitted input's array indices.
fn original_path(path: &[Seg], removed: &[Vec<Seg>]) -> Vec<Seg> {
    let mut original = Vec::with_capacity(path.len());
    for seg in path {
        match seg {
            Seg::Key(key) => original.push(Seg::Key(key.clone())),
            Seg::Index(index) => {
                let mut gone: Vec<usize> = removed
                    .iter()
                    .filter_map(|item| match item.split_last() {
                        Some((Seg::Index(at), array)) if array == original.as_slice() => Some(*at),
                        _ => None,
                    })
                    .collect();
                gone.sort_unstable();
                let mut index = *index;
                for at in gone {
                    if at <= index {
                        index += 1;
                    }
                }
                original.push(Seg::Index(index));
            }
        }
    }
    original
}

pub(crate) fn validate(
    input: &SetupProposalInput,
    config: &Config,
) -> Result<SetupValidationFacets> {
    let mut problems = Problems::default();
    let facets = collect(input, config, &mut problems);
    problems.finish(facets)
}

/// Record every semantic and cross-reference problem; facets are meaningful only when none.
pub(crate) fn collect(
    input: &SetupProposalInput,
    config: &Config,
    problems: &mut Problems,
) -> SetupValidationFacets {
    let proposal = &input.proposal;
    let proposal_path = JsonPath::default().key("proposal");
    let discovery_path = JsonPath::default().key("discovery");
    if let Err(err) = proposal.validate(config) {
        problems.error(&proposal_path, &err);
    }
    if let Err(err) = input.discovery.validate() {
        problems.error(&discovery_path, &err);
    }
    problems.require(
        !matches!(proposal.basis, SetupBasis::Manual),
        &proposal_path.key("basis"),
        "manual configuration uses config editing, not evidence-approved setup",
    );
    problems.require(
        input.discovery.observed_at <= proposal.prepared_at,
        &discovery_path.key("observed_at"),
        "discovery postdates proposal",
    );
    for (id, evidence) in &proposal.evidence {
        check_evidence(
            evidence,
            proposal,
            &proposal_path.key("evidence").key(id),
            problems,
        );
    }
    let mut access_checked = true;
    let mut execution_tested = true;
    for (key, profile) in &proposal.profiles {
        let metadata_path = proposal_path.key("profiles").key(key).key("metadata");
        let Some(metadata) = profile.metadata.as_ref() else {
            problems.push(&metadata_path, "missing profile metadata");
            continue;
        };
        problems.require(
            !metadata.candidates.is_empty(),
            &metadata_path.key("candidates"),
            format!("profile '{key}' needs an exact execution candidate"),
        );
        if let Some(previous) = config.profiles.get(key)
            && previous != profile
            && let Some(old_metadata) = &previous.metadata
        {
            problems.require(
                old_metadata.revision != metadata.revision,
                &metadata_path.key("revision"),
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
    problems: &mut Problems,
) -> AccessFacets {
    let proposal = &input.proposal;
    let name = format!("{key}/{}", candidate.id);
    check_arguments(config, candidate, path, problems);
    check_preferences(proposal, candidate, path, problems);

    let access_path = JsonPath::default().key("proposal").key("access");
    let matching: Vec<_> = proposal
        .access
        .iter()
        .enumerate()
        .filter(|(_, access)| access.profile == key && access.candidate == candidate.id)
        .collect();
    let access = match matching.as_slice() {
        [one] => Some(*one),
        _ => {
            problems.push(
                &access_path,
                format!(
                    "candidate '{name}' needs one independent access record; found {}",
                    matching.len()
                ),
            );
            None
        }
    };
    if let Some((index, access)) = access {
        problems.require(
            !access.evidence.is_empty(),
            &access_path.index(index).key("evidence"),
            format!("access record for '{name}' cites no discovery evidence"),
        );
    }

    let Some((observed_index, observation)) =
        input
            .discovery
            .candidates
            .iter()
            .enumerate()
            .find(|(_, o)| {
                o.harness == candidate.harness
                    && o.connection == candidate.connection
                    && o.provider == candidate.provider
                    && o.model == candidate.model
            })
    else {
        problems.push(
            path,
            format!(
                "no independent discovery for candidate '{name}' (matching harness, connection, provider and model)"
            ),
        );
        return AccessFacets::default();
    };
    let observation_path = JsonPath::default()
        .key("discovery")
        .key("candidates")
        .index(observed_index);
    let facets = &observation.facets;
    problems.require(
        facets.installed
            && (facets.discovered || facets.access_verified)
            && (facets.user_reported || facets.access_verified),
        &observation_path.key("facets"),
        format!("candidate '{name}' lacks installed, model and connection access observations"),
    );
    problems.require(
        observation
            .settings
            .iter()
            .any(|settings| settings.effort == candidate.effort && settings.args == candidate.args),
        path,
        format!("unsupported exact model settings for '{name}'"),
    );
    let observed_evidence: BTreeSet<_> = observation.evidence.iter().map(|item| &item.id).collect();
    if let Some((index, access)) = access {
        let path = access_path.index(index);
        problems.require(
            access.facets == observation.facets,
            &path.key("facets"),
            format!("access facets for '{name}' differ from discovery {observation_path}"),
        );
        problems.require(
            access.observed_at == observation.observed_at,
            &path.key("observed_at"),
            format!("access timestamp for '{name}' differs from discovery {observation_path}"),
        );
        for (item, id) in access.evidence.iter().enumerate() {
            problems.require(
                observed_evidence.contains(id),
                &path.key("evidence").index(item),
                format!("access evidence '{id}' is not in discovery {observation_path}"),
            );
        }
    }
    problems.require(
        observed_evidence.contains(&candidate.evidence),
        &path.key("evidence"),
        format!(
            "candidate evidence '{}' is not in discovery {observation_path}",
            candidate.evidence
        ),
    );
    for (item, evidence) in observation.evidence.iter().enumerate() {
        let path = observation_path.key("evidence").index(item);
        check_evidence(evidence, proposal, &path, problems);
        problems.require(
            proposal.evidence.get(&evidence.id) == Some(evidence),
            &path,
            "discovery evidence differs from exact proposal",
        );
    }
    problems.require(
        observation.observed_at <= input.discovery.observed_at
            && observation.observed_at <= proposal.prepared_at,
        &observation_path.key("observed_at"),
        "candidate observation postdates discovery or proposal",
    );
    observation.facets.clone()
}

/// An extra argv override must not change the separately approved binding.
fn check_arguments(
    config: &Config,
    candidate: &ExecutionCandidate,
    path: &JsonPath,
    problems: &mut Problems,
) {
    let flags = match config.harness(&candidate.harness) {
        Ok((_, flags)) => flags,
        Err(err) => {
            problems.error(&path.key("harness"), &err);
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
        "candidate arguments override approved model or effort",
    );
}

fn check_preferences(
    proposal: &ProfileSetupProposal,
    candidate: &ExecutionCandidate,
    path: &JsonPath,
    problems: &mut Problems,
) {
    let preferences = &proposal.preferences;
    problems.require(
        preferences.allowed_providers.is_empty()
            || preferences.allowed_providers.contains(&candidate.provider),
        &path.key("provider"),
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
                "candidate exceeds setup spending bound",
            ),
            CandidateCost::Unknown | CandidateCost::Subscription => problems.push(
                &path.key("cost"),
                "unknown cost cannot satisfy setup spending bound",
            ),
        }
    }
}

fn check_evidence(
    evidence: &ModelEvidence,
    proposal: &ProfileSetupProposal,
    path: &JsonPath,
    problems: &mut Problems,
) {
    problems.require(
        evidence.retrieved_at <= proposal.prepared_at,
        &path.key("retrieved_at"),
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
        "evidence publication/measurement postdates retrieval",
    );
}

#[cfg(test)]
#[path = "setup_validation_tests.rs"]
mod tests;
