//! Availability fallback inside one profile (PRF-09): provider-error classes, a durable bounded
//! recovery episode and pure candidate selection. Availability is not difficulty: a switch keeps
//! the profile, its instructions and its role contract, and never widens the approved list.
//! Ops owns persistence, the single retry/fallback ownership claim and every transition.

use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::{
    Config, ExecutionCandidate, ProfileChoice, ProfileMetadata, ProfileRole, ResolvedProfile,
    RouteDecision,
};

pub const AVAILABILITY_EPISODE_VERSION: u32 = 1;

/// Why a provider turn failed, when the failure is about access rather than the work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AvailabilityClass {
    /// Rate limit, overload or a 5xx: the same candidate is retried within its bounded allowance.
    Transient {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after_secs: Option<u64>,
    },
    /// `account` only when the provider named an account, plan or billing scope; other
    /// candidates on the same connection are then exhausted too. Never inferred globally.
    QuotaExhausted {
        account: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reset_at: Option<Timestamp>,
    },
    Unavailable,
    /// Missing or expired credentials. Never authorizes copying credentials or another account.
    CredentialUnavailable,
}

impl AvailabilityClass {
    /// Confirmed unavailability skips the same-candidate transient retry allowance.
    pub fn advances_immediately(&self) -> bool {
        !matches!(self, Self::Transient { .. })
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Transient { .. } => "transient provider failure",
            Self::QuotaExhausted { account: true, .. } => "account quota exhausted",
            Self::QuotaExhausted { .. } => "quota exhausted",
            Self::Unavailable => "model or endpoint unavailable",
            Self::CredentialUnavailable => "credentials unavailable",
        }
    }

    /// The provider-stated instant this candidate may be usable again, if it gave one.
    pub fn reset_at(&self, at: Timestamp) -> Option<Timestamp> {
        match self {
            Self::Transient {
                retry_after_secs: Some(secs),
            } => at
                .checked_add(SignedDuration::from_secs(i64::try_from(*secs).ok()?))
                .ok(),
            Self::QuotaExhausted { reset_at, .. } => *reset_at,
            Self::Transient { .. } | Self::Unavailable | Self::CredentialUnavailable => None,
        }
    }
}

const CREDENTIAL: &[&str] = &[
    "invalid api key",
    "invalid x-api-key",
    "incorrect api key",
    "invalid_api_key",
    "authentication_error",
    "authentication failed",
    "unauthorized",
    "token has expired",
    "token expired",
    "expired token",
    "not logged in",
    "please run /login",
    "no credential",
    "credential slots",
    "missing credentials",
];
const QUOTA: &[&str] = &[
    "insufficient_quota",
    "exceeded your current quota",
    "quota exceeded",
    "quota exhausted",
    "usage limit",
    "credit balance is too low",
    "out of credits",
    "plan limit",
    "monthly limit",
    "weekly limit",
    "hit your limit",
];
const ACCOUNT: &[&str] = &[
    "account",
    "organization",
    "billing",
    "credit",
    "subscription",
    "plan",
    "usage limit",
    "your limit",
];
const UNAVAILABLE: &[&str] = &[
    "model_not_found",
    "model not found",
    "no such model",
    "unknown model",
    "model is not available",
    "model unavailable",
    "has been deprecated",
    "decommissioned",
    "does not exist or you do not have access",
    "not_found_error",
    "endpoint not found",
];
/// Request, capability and judgment failures are never routed around.
const NOT_AVAILABILITY: &[&str] = &[
    "refus",
    "content policy",
    "content_filter",
    "safety",
    "invalid_request",
    "bad request",
    "context length",
    "context window",
    "prompt is too long",
    "maximum context",
    "tool use",
    "does not support tools",
    "unsupported parameter",
    "malformed",
];
const TRANSIENT: &[&str] = &[
    "overloaded",
    "rate limit",
    "rate_limit",
    "too many requests",
    "try again later",
    "temporarily unavailable",
    "service unavailable",
    "bad gateway",
    "gateway timeout",
    "internal server error",
    "capacity",
];

/// A failed provider turn's error text -> its availability class; None is not availability.
/// `at` anchors relative reset hints ("retry after 30s"). Only the harness's structured error
/// field is classified, never an idle pane, a worker's prose or a failing test.
pub fn classify(message: &str, at: Timestamp) -> Option<AvailabilityClass> {
    let lower = message.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| lower.contains(needle));
    let code = |codes: &[&str]| {
        lower
            .split(|c: char| !c.is_ascii_digit())
            .any(|word| codes.contains(&word))
    };
    if has(CREDENTIAL) || code(&["401"]) {
        return Some(AvailabilityClass::CredentialUnavailable);
    }
    if has(QUOTA) {
        return Some(AvailabilityClass::QuotaExhausted {
            account: has(ACCOUNT),
            reset_at: reset_time(&lower, at),
        });
    }
    if has(UNAVAILABLE) || code(&["404"]) {
        return Some(AvailabilityClass::Unavailable);
    }
    if has(NOT_AVAILABILITY) {
        return None;
    }
    (has(TRANSIENT) || code(&["429", "500", "502", "503", "504", "529"])).then(|| {
        AvailabilityClass::Transient {
            retry_after_secs: relative_secs(&lower),
        }
    })
}

fn reset_time(lower: &str, at: Timestamp) -> Option<Timestamp> {
    let absolute = lower
        .split(|c: char| c.is_whitespace() || matches!(c, '|' | ',' | '(' | ')' | '"' | '\''))
        .map(|token| token.trim_end_matches(['.', ';']))
        .find_map(|token| {
            if let Ok(stamp) = token.to_ascii_uppercase().parse::<Timestamp>() {
                return Some(stamp);
            }
            let epoch = (token.len() >= 10 && token.bytes().all(|b| b.is_ascii_digit()))
                .then(|| token.parse::<i64>().ok())
                .flatten()?;
            Timestamp::from_second(epoch)
                .ok()
                .filter(|stamp| *stamp > at)
        });
    absolute.or_else(|| {
        let secs = i64::try_from(relative_secs(lower)?).ok()?;
        at.checked_add(SignedDuration::from_secs(secs)).ok()
    })
}

/// "retry after 30", "retry-after: 30s", "try again in 5 minutes".
fn relative_secs(lower: &str) -> Option<u64> {
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    words.windows(2).enumerate().find_map(|(index, pair)| {
        if !matches!(pair[0], "after" | "in") {
            return None;
        }
        let digits: String = pair[1].chars().take_while(char::is_ascii_digit).collect();
        let value: u64 = digits.parse().ok()?;
        let suffix = &pair[1][digits.len()..];
        let unit = if suffix.is_empty() {
            words.get(index + 2).copied().unwrap_or("s")
        } else {
            suffix
        };
        let scale = match unit {
            "ms" => return Some(value.div_ceil(1000)),
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
            _ => 1,
        };
        value.checked_mul(scale)
    })
}

/// Which approved candidate a run or review attempt actually used, frozen at launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateBinding {
    pub candidate: String,
    /// The recovery episode that selected a non-preferred candidate, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Candidates passed over before start; a later episode never cycles back to them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<SkippedCandidate>,
    /// An exact-model override: the run is not eligible for automatic substitution.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkippedCandidate {
    pub candidate: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptFailure {
    pub class: AvailabilityClass,
    pub message: String,
    /// Same-candidate retries consumed before the candidate's turn ended.
    pub retries: u32,
    /// The first failed provider turn; a redelivery of the same stall is a no-op.
    pub at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovered_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateAttempt {
    pub candidate: String,
    pub provider: String,
    pub connection: String,
    pub model: String,
    /// The run or review attempt that used it; None until a continuation launches.
    pub run: Option<String>,
    pub selected_at: Timestamp,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<AttemptFailure>,
}

impl CandidateAttempt {
    pub fn open_failure(&self) -> Option<&AttemptFailure> {
        self.failures
            .last()
            .filter(|failure| failure.recovered_at.is_none())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exhaustion {
    pub reason: String,
    pub at: Timestamp,
    /// A provider-stated reset for the current candidate: Waiting rather than a decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_until: Option<Timestamp>,
}

/// One bounded traversal of a profile's ordered candidates for one task role. The ordered
/// attempt history is never reset by a sync or restart, and a candidate is never revisited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AvailabilityEpisode {
    pub version: u32,
    pub id: String,
    pub task: String,
    pub role: ProfileRole,
    pub profile: String,
    pub revision: String,
    pub started: Timestamp,
    pub attempts: Vec<CandidateAttempt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exhausted: Option<Exhaustion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    Next {
        candidate: ExecutionCandidate,
        skipped: Vec<SkippedCandidate>,
    },
    /// The approved list is used up; `waiting_until` is a provider-stated reset.
    Exhausted {
        reason: String,
        waiting_until: Option<Timestamp>,
    },
    /// Policy forbids substitution (no approval, automatic off, exact pin, changed profile).
    Blocked { reason: String },
}

impl AvailabilityEpisode {
    pub fn new(
        id: &str,
        task: &str,
        role: ProfileRole,
        profile: &str,
        metadata: &ProfileMetadata,
        now: Timestamp,
    ) -> Self {
        Self {
            version: AVAILABILITY_EPISODE_VERSION,
            id: id.to_owned(),
            task: task.to_owned(),
            role,
            profile: profile.to_owned(),
            revision: metadata.revision.clone(),
            started: now,
            attempts: Vec::new(),
            exhausted: None,
        }
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == AVAILABILITY_EPISODE_VERSION,
            "unsupported availability episode version {}",
            self.version
        );
        let mut seen = BTreeSet::new();
        for attempt in &self.attempts {
            ensure!(
                seen.insert(attempt.candidate.as_str()),
                "availability episode revisits candidate '{}'",
                attempt.candidate
            );
        }
        Ok(())
    }

    pub fn seed(
        &mut self,
        candidate: &ExecutionCandidate,
        run: Option<&str>,
        reason: &str,
        now: Timestamp,
    ) -> Result<&mut CandidateAttempt> {
        ensure!(
            !self.attempts.iter().any(|a| a.candidate == candidate.id),
            "candidate '{}' is already part of this episode; episodes never cycle",
            candidate.id
        );
        self.attempts.push(CandidateAttempt {
            candidate: candidate.id.clone(),
            provider: candidate.provider.clone(),
            connection: candidate.connection.clone(),
            model: candidate.model.clone(),
            run: run.map(str::to_owned),
            selected_at: now,
            reason: reason.to_owned(),
            failures: Vec::new(),
        });
        self.attempts.last_mut().context("attempt was not recorded")
    }

    pub fn current(&self) -> Option<&CandidateAttempt> {
        self.attempts.last()
    }

    /// A selected continuation candidate that has not run or failed yet.
    pub fn pending(&self) -> Option<&CandidateAttempt> {
        self.current().filter(|attempt| {
            self.exhausted.is_none() && attempt.run.is_none() && attempt.failures.is_empty()
        })
    }

    /// Advances excluding the first candidate; bounded by the approved `max_switches`.
    pub fn switches(&self) -> usize {
        self.attempts.len().saturating_sub(1)
    }

    /// The current candidate's turn failed. Returns false for a redelivery of the same stall.
    pub fn record_failure(&mut self, run: &str, failure: AttemptFailure) -> Result<bool> {
        let current = self
            .attempts
            .last_mut()
            .context("availability episode has no candidate")?;
        if current.run.is_none() {
            current.run = Some(run.to_owned());
        }
        ensure!(
            current.run.as_deref() == Some(run),
            "failure of {run} does not belong to the episode's current attempt"
        );
        if current.failures.iter().any(|old| old.at == failure.at) {
            return Ok(false);
        }
        current.failures.push(failure);
        Ok(true)
    }

    /// The current candidate answered again: close its failure and any Waiting state.
    pub fn recovered(&mut self, at: Timestamp) -> bool {
        // A switch that never launched did not happen: the preferred candidate is back.
        if self.pending().is_some() && self.attempts.len() > 1 {
            self.attempts.pop();
        }
        let Some(current) = self.attempts.last_mut() else {
            return false;
        };
        let Some(open) = current
            .failures
            .last_mut()
            .filter(|failure| failure.recovered_at.is_none())
        else {
            return false;
        };
        open.recovered_at = Some(at);
        self.exhausted = None;
        true
    }

    /// Host policy: the next eligible approved candidate, never one already attempted.
    pub fn select(
        &self,
        config: &Config,
        metadata: &ProfileMetadata,
        exact_model_pin: bool,
        exclude_provider: Option<&str>,
    ) -> Selection {
        let blocked = |reason: &str| Selection::Blocked {
            reason: reason.to_owned(),
        };
        let Some(fallback) = &metadata.fallback else {
            return blocked("the profile has no approved availability fallback");
        };
        if !fallback.automatic {
            return blocked("automatic fallback is off for this profile");
        }
        if exact_model_pin {
            return blocked("an exact-model pin disables substitution");
        }
        if metadata.revision != self.revision {
            return blocked("the profile changed during this recovery episode");
        }
        let waiting_until = self
            .current()
            .and_then(CandidateAttempt::open_failure)
            .and_then(|failure| failure.class.reset_at(failure.at));
        if self.switches() >= usize::try_from(fallback.max_switches).unwrap_or(usize::MAX) {
            return Selection::Exhausted {
                reason: format!(
                    "the approved fallback allowance ({} switch{}) is used",
                    fallback.max_switches,
                    if fallback.max_switches == 1 { "" } else { "es" }
                ),
                waiting_until,
            };
        }
        let exhausted_accounts: BTreeSet<&str> = self
            .attempts
            .iter()
            .filter(|attempt| {
                attempt.failures.iter().any(|failure| {
                    matches!(
                        failure.class,
                        AvailabilityClass::QuotaExhausted { account: true, .. }
                    )
                })
            })
            .map(|attempt| attempt.connection.as_str())
            .collect();
        let mut skipped = Vec::new();
        for candidate in &metadata.candidates {
            if self.attempts.iter().any(|a| a.candidate == candidate.id) {
                continue;
            }
            let refusal = if exhausted_accounts.contains(candidate.connection.as_str()) {
                Some("its connection shares the exhausted account quota".to_owned())
            } else if exclude_provider == Some(candidate.provider.as_str()) {
                Some("it would violate the cross-family review policy".to_owned())
            } else if let Err(error) = fallback.admits(candidate) {
                Some(format!("{error:#}"))
            } else if let Err(error) = candidate.validate(config) {
                Some(format!("invalid settings: {error:#}"))
            } else {
                None
            };
            match refusal {
                Some(reason) => skipped.push(SkippedCandidate {
                    candidate: candidate.id.clone(),
                    reason,
                }),
                None => {
                    return Selection::Next {
                        candidate: candidate.clone(),
                        skipped,
                    };
                }
            }
        }
        let detail = skipped
            .iter()
            .map(|s| format!("{} ({})", s.candidate, s.reason))
            .collect::<Vec<_>>();
        Selection::Exhausted {
            reason: if detail.is_empty() {
                "no approved candidate remains".to_owned()
            } else {
                format!(
                    "no approved candidate remains; rejected: {}",
                    detail.join(", ")
                )
            },
            waiting_until,
        }
    }

    /// Durable intent before any external effect: the chosen continuation candidate.
    pub fn advance(
        &mut self,
        candidate: &ExecutionCandidate,
        reason: &str,
        now: Timestamp,
    ) -> Result<()> {
        ensure!(
            self.exhausted.is_none(),
            "availability episode is exhausted"
        );
        ensure!(
            self.current()
                .and_then(CandidateAttempt::open_failure)
                .is_some(),
            "the current candidate has not failed"
        );
        self.seed(candidate, None, reason, now)?;
        Ok(())
    }

    pub fn exhaust(&mut self, reason: &str, waiting_until: Option<Timestamp>, now: Timestamp) {
        self.exhausted = Some(Exhaustion {
            reason: reason.to_owned(),
            at: now,
            waiting_until,
        });
    }

    pub fn summary(&self) -> String {
        self.attempts
            .iter()
            .map(|attempt| {
                let state = match (attempt.failures.last(), &attempt.run) {
                    (Some(failure), _) if failure.recovered_at.is_none() => failure.class.label(),
                    (Some(_), _) => "recovered",
                    (None, None) => "selected",
                    (None, Some(_)) => "running",
                };
                format!(
                    "{} ({} via {}): {state}",
                    attempt.candidate, attempt.model, attempt.connection
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

impl crate::AvailabilityFallback {
    /// Provider approval and spending policy for one candidate, independent of the others.
    pub fn admits(&self, candidate: &ExecutionCandidate) -> Result<()> {
        ensure!(
            self.approved_providers.contains(&candidate.provider),
            "candidate provider '{}' is not approved",
            candidate.provider
        );
        match &candidate.cost {
            crate::CandidateCost::Unknown | crate::CandidateCost::Subscription => ensure!(
                self.allow_unknown_subscription_cost && self.spending.is_none(),
                "unknown cost requires explicit permission and cannot satisfy a numeric cap"
            ),
            crate::CandidateCost::Metered {
                currency,
                maximum_micros,
            } => {
                let bound = self
                    .spending
                    .as_ref()
                    .context("metered fallback needs a spending bound")?;
                ensure!(
                    currency == &bound.currency && maximum_micros <= &bound.maximum_micros,
                    "candidate exceeds approved spending bound"
                );
            }
        }
        Ok(())
    }
}

impl Config {
    pub fn execution_candidate(&self, profile: &str, id: &str) -> Result<&ExecutionCandidate> {
        self.profiles
            .get(profile)
            .and_then(|p| p.metadata.as_ref())
            .and_then(|metadata| metadata.candidates.iter().find(|c| c.id == id))
            .with_context(|| format!("profile '{profile}' has no execution candidate '{id}'"))
    }

    /// A candidate's own harness, model, effort and arguments; nothing is copied from another.
    pub fn resolve_candidate(
        &self,
        profile: &str,
        candidate: &ExecutionCandidate,
    ) -> Result<ResolvedProfile> {
        candidate
            .validate(self)
            .with_context(|| format!("profile '{profile}' candidate '{}'", candidate.id))?;
        let (command, flags) = self.harness(&candidate.harness)?;
        let mut model_args = flags.render(
            &candidate.harness,
            Some(&candidate.model),
            candidate.effort.as_deref(),
        )?;
        model_args.extend(candidate.args.iter().cloned());
        Ok(ResolvedProfile {
            profile: profile.to_owned(),
            harness: candidate.harness.clone(),
            command,
            model: Some(candidate.model.clone()),
            vendor: Some(candidate.provider.clone()),
            effort: candidate
                .effort
                .as_deref()
                .map(crate::profile::normalize_effort),
            model_args,
        })
    }

    pub fn resolve_route(&self, route: &RouteDecision) -> Result<ResolvedProfile> {
        match route
            .availability
            .as_ref()
            .filter(|binding| !binding.pinned)
        {
            Some(binding) => self.resolve_candidate(
                &route.profile,
                self.execution_candidate(&route.profile, &binding.candidate)?,
            ),
            None => self.resolve_profile(
                &ProfileChoice {
                    profile: Some(route.profile.clone()),
                    model: route.model.clone(),
                    effort: route.effort.clone(),
                },
                None,
                None,
            ),
        }
    }
}

#[cfg(test)]
#[path = "availability_tests.rs"]
mod tests;
