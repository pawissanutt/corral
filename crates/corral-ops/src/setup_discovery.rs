//! Sanitized local inventory, separate from explicitly observed candidate access.

use std::collections::BTreeSet;

use anyhow::Result;
use corral_core::{
    AccessFacets, Config, JsonPath, ModelEvidence, ModelEvidenceSource, PI_NATIVE_HARNESS,
    PathProblems, Read, is_pi_native,
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Ctx;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupDiscovery {
    pub observed_at: Timestamp,
    pub harnesses: Vec<DiscoveredHarness>,
    pub candidates: Vec<CandidateObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveredHarness {
    pub key: String,
    pub installed: bool,
    pub models: Vec<String>,
    pub efforts: Vec<String>,
    /// Vendors the harness accepts; reported only by harnesses that restrict them (pi-native).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vendors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateSettings {
    pub effort: Option<String>,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateObservation {
    pub harness: String,
    pub connection: String,
    pub provider: String,
    pub model: String,
    pub observed_at: Timestamp,
    pub facets: AccessFacets,
    pub settings: Vec<CandidateSettings>,
    pub evidence: Vec<ModelEvidence>,
}

/// Only existing harness probes run here. Listing a model proves neither authentication
/// nor execution, and cannot identify an exact connection or settings.
///
/// The built-in pi-native host has no command to probe: it is installed exactly when
/// `corral agent doctor` reports it ready (credentials from file metadata only).
pub fn setup_discover(ctx: &Ctx) -> SetupDiscovery {
    let config = &ctx.store.config;
    let ready = crate::agent::hooks::doctor::ready(ctx);
    discover_with(
        Timestamp::now(),
        crate::detect_harnesses(config),
        config,
        ready,
        crate::agent::hooks::preflight::supported_vendors(ctx),
    )
}

fn discover_with(
    observed_at: Timestamp,
    harnesses: Vec<crate::HarnessInfo>,
    config: &Config,
    pi_native_ready: bool,
    pi_native_vendors: Vec<String>,
) -> SetupDiscovery {
    let mut discovery = sanitized_discovery(observed_at, harnesses);
    if pi_native_ready || crate::agent::hooks::configured(config) {
        discovery.harnesses.retain(|h| !is_pi_native(&h.key));
        discovery.harnesses.push(DiscoveredHarness {
            key: PI_NATIVE_HARNESS.into(),
            installed: pi_native_ready,
            models: Vec::new(),
            efforts: config
                .harness(PI_NATIVE_HARNESS)
                .map(|(_, flags)| flags.efforts)
                .unwrap_or_default(),
            vendors: pi_native_vendors,
        });
        discovery.harnesses.sort_by(|a, b| a.key.cmp(&b.key));
    }
    discovery
}

fn sanitized_discovery(
    observed_at: Timestamp,
    harnesses: Vec<crate::HarnessInfo>,
) -> SetupDiscovery {
    SetupDiscovery {
        observed_at,
        harnesses: harnesses
            .into_iter()
            .filter(|h| safe_identity(&h.key))
            .map(|h| {
                let installed = h.installed();
                DiscoveredHarness {
                    key: h.key,
                    installed,
                    models: h.models.into_iter().filter(|s| safe_identity(s)).collect(),
                    efforts: h.efforts.into_iter().filter(|s| safe_identity(s)).collect(),
                    vendors: Vec::new(),
                }
            })
            .collect(),
        candidates: Vec::new(),
    }
}

impl SetupDiscovery {
    pub fn validate(&self) -> Result<()> {
        let mut problems = PathProblems::default();
        self.collect_problems(&JsonPath::default(), &mut problems);
        problems.first()
    }

    /// Every inventory and observation problem, each at its own path below `base`.
    pub fn collect_problems(&self, base: &JsonPath, out: &mut PathProblems) {
        let harnesses_path = base.key("harnesses");
        let mut harnesses = BTreeSet::new();
        for (index, harness) in self.harnesses.iter().enumerate() {
            let path = harnesses_path.index(index);
            out.require(
                safe_identity(&harness.key),
                &path.key("key"),
                &[],
                "invalid harness identity",
            );
            out.require_reading(
                harnesses.insert(&harness.key),
                &path.key("key"),
                (0..=index)
                    .map(|i| Read::Value(harnesses_path.index(i).key("key")))
                    .collect(),
                "duplicate harness identity",
            );
            out.require_reading(
                harness
                    .models
                    .iter()
                    .chain(&harness.efforts)
                    .chain(&harness.vendors)
                    .all(|s| safe_identity(s)),
                &path,
                vec![
                    Read::Value(path.key("models")),
                    Read::Value(path.key("efforts")),
                    Read::Value(path.key("vendors")),
                ],
                "invalid harness model, effort or vendor",
            );
        }
        let candidates_path = base.key("candidates");
        let mut candidates = BTreeSet::new();
        let identity = |index: usize| {
            let path = candidates_path.index(index);
            ["harness", "connection", "provider", "model"].map(|field| Read::Value(path.key(field)))
        };
        for (index, candidate) in self.candidates.iter().enumerate() {
            let path = candidates_path.index(index);
            let evidence_path = path.key("evidence");
            out.require_reading(
                [
                    &candidate.harness,
                    &candidate.connection,
                    &candidate.provider,
                    &candidate.model,
                ]
                .into_iter()
                .all(|s| safe_identity(s)),
                &path,
                identity(index).into(),
                "invalid candidate identity",
            );
            // Reads only harness keys and membership, so malformed model lists cannot hide it.
            out.require_reading(
                harnesses.contains(&candidate.harness),
                &path.key("harness"),
                std::iter::once(path.key("harness"))
                    .chain((0..self.harnesses.len()).map(|i| harnesses_path.index(i).key("key")))
                    .map(Read::Value)
                    .chain([Read::Members(harnesses_path.clone())])
                    .collect(),
                "candidate references unknown harness",
            );
            out.require_reading(
                candidates.insert((
                    &candidate.harness,
                    &candidate.connection,
                    &candidate.provider,
                    &candidate.model,
                )),
                &path,
                (0..=index).flat_map(identity).collect(),
                "duplicate exact candidate",
            );
            out.require(
                candidate.observed_at <= self.observed_at,
                &path.key("observed_at"),
                &[&base.key("observed_at")],
                "candidate observation postdates discovery",
            );
            let mut settings = BTreeSet::new();
            for (item, setting) in candidate.settings.iter().enumerate() {
                let path = path.key("settings").index(item);
                out.require(
                    setting.effort.as_deref().is_none_or(safe_identity),
                    &path.key("effort"),
                    &[],
                    "invalid candidate effort",
                );
                out.require(
                    setting.args.iter().all(|s| safe_arg(s)),
                    &path.key("args"),
                    &[],
                    "unsafe candidate args",
                );
                let settings_path = candidates_path.index(index).key("settings");
                out.require_reading(
                    settings.insert((&setting.effort, &setting.args)),
                    &path,
                    (0..=item)
                        .map(|i| Read::Value(settings_path.index(i)))
                        .collect(),
                    "duplicate candidate settings",
                );
            }
            let mut evidence_ids = BTreeSet::new();
            let mut local_evidence = false;
            for (item, evidence) in candidate.evidence.iter().enumerate() {
                let path = path.key("evidence").index(item);
                out.require(
                    evidence.version == 1,
                    &path.key("version"),
                    &[],
                    "unsupported evidence version",
                );
                out.require_reading(
                    safe_identity(&evidence.id) && evidence_ids.insert(&evidence.id),
                    &path.key("id"),
                    (0..=item)
                        .map(|i| Read::Value(evidence_path.index(i).key("id")))
                        .collect(),
                    "invalid or duplicate evidence identity",
                );
                out.require(
                    evidence.retrieved_at <= candidate.observed_at,
                    &path.key("retrieved_at"),
                    &[&candidates_path.index(index).key("observed_at")],
                    "evidence postdates candidate observation",
                );
                let retrieved_on = evidence
                    .retrieved_at
                    .to_zoned(jiff::tz::TimeZone::UTC)
                    .date();
                out.require_reading(
                    evidence
                        .published_on
                        .is_none_or(|date| date <= retrieved_on)
                        && evidence.measured_on.is_none_or(|date| date <= retrieved_on),
                    &path,
                    ["published_on", "measured_on", "retrieved_at"]
                        .map(|field| Read::Value(path.key(field)))
                        .into(),
                    "evidence dates postdate retrieval",
                );
                let source = match &evidence.source {
                    ModelEvidenceSource::Web { url } => {
                        out.require(
                            (url.starts_with("https://") || url.starts_with("http://"))
                                && url.split_once("://").is_some_and(|(_, rest)| {
                                    !rest
                                        .split(['/', '?', '#'])
                                        .next()
                                        .unwrap_or_default()
                                        .is_empty()
                                })
                                && !url.contains('@')
                                && !url.chars().any(char::is_whitespace),
                            &path.key("source"),
                            &[],
                            "invalid evidence URL",
                        );
                        url
                    }
                    ModelEvidenceSource::LocalDiscovery { method } => {
                        local_evidence = true;
                        method
                    }
                };
                out.require_reading(
                    [source, &evidence.claim, &evidence.finding]
                        .into_iter()
                        .all(|s| !s.trim().is_empty() && !contains_secret(s))
                        && evidence.limitations.iter().all(|s| !contains_secret(s)),
                    &path,
                    ["source", "claim", "finding", "limitations"]
                        .map(|field| Read::Value(path.key(field)))
                        .into(),
                    "empty or unsafe evidence",
                );
            }
            out.require_reading(
                !(candidate.facets.access_verified || candidate.facets.execution_tested)
                    || local_evidence,
                &path.key("facets"),
                (0..candidate.evidence.len())
                    .map(|i| Read::Value(evidence_path.index(i).key("source")))
                    .chain([
                        Read::Value(path.key("facets")),
                        Read::Members(evidence_path.clone()),
                    ])
                    .collect(),
                "verification or execution requires local evidence",
            );
            out.require_reading(
                !candidate.facets.execution_tested || !candidate.settings.is_empty(),
                &path.key("facets"),
                vec![
                    Read::Value(path.key("facets")),
                    Read::Members(path.key("settings")),
                ],
                "execution requires exact observed settings",
            );
        }
    }
}

fn safe_identity(value: &str) -> bool {
    if value.is_empty()
        || value.starts_with(['/', '\\', '.', '-'])
        || value.contains(":/")
        || (value.contains(':') && value.contains('@'))
        || contains_secret(value)
    {
        return false;
    }
    value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_-./:@+".contains(c))
}

fn contains_secret(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "bearer",
        "api_key",
        "api-key",
        "apikey",
        "access_token",
        "access-token",
        "authorization",
        "password",
        "client_secret",
        "client-secret",
        "token=",
        "token:",
        "secret=",
        "secret:",
        "credential",
        "sk-",
        "sk_",
        "ghp_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "aiza",
        "eyj",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn safe_arg(value: &str) -> bool {
    if value.trim().is_empty() || value.chars().any(char::is_control) || contains_secret(value) {
        return false;
    }
    // Settings may tune execution, but cannot replace the recorded identity or carry
    // authentication via generic header/user flags.
    let flag = value.split(['=', ' ']).next().unwrap_or_default();
    let normalized: String = flag
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect();
    if flag.starts_with('-')
        && (["auth", "credential", "password", "secret", "apikey"]
            .iter()
            .any(|marker| normalized.contains(marker))
            || normalized.ends_with("token")
            || normalized == "key"
            || normalized.starts_with("keyfile")
            || flag.starts_with("-H")
            || flag.starts_with("-u"))
    {
        return false;
    }
    !matches!(
        flag,
        "--token"
            | "--secret"
            | "--credential"
            | "--credentials"
            | "--auth"
            | "--header"
            | "--headers"
            | "-H"
            | "--user"
            | "-u"
            | "--model"
            | "--provider"
            | "--connection"
            | "--effort"
    )
}

#[cfg(test)]
#[path = "setup_discovery_tests.rs"]
mod tests;
