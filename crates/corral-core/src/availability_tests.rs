use std::collections::BTreeSet;

use super::*;
use crate::{
    AvailabilityFallback, CandidateCost, Profile, ProfileEligibility, SelectionGuidance,
    SpendingBound,
};

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
}

fn t0() -> Timestamp {
    at("2026-10-21T09:00:00Z")
}

fn candidate(id: &str, harness: &str, provider: &str, connection: &str) -> ExecutionCandidate {
    ExecutionCandidate {
        id: id.into(),
        harness: harness.into(),
        connection: connection.into(),
        provider: provider.into(),
        model: format!("{provider}/{id}-fixture"),
        effort: Some(if harness == "claude" { "max" } else { "high" }.into()),
        args: vec![],
        evidence: "evidence-1".into(),
        cost: CandidateCost::Subscription,
    }
}

/// General: Sol-like preferred, a second model on the same OpenAI account, then Opus-like.
fn general() -> (tempfile::TempDir, Config, ProfileMetadata) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default_for(dir.path());
    let metadata = ProfileMetadata {
        display_name: "General".into(),
        purpose: "Everyday implementation".into(),
        guidance: SelectionGuidance::default(),
        instructions: "Follow established patterns".into(),
        eligibility: ProfileEligibility {
            enabled: true,
            automatic: true,
            roles: BTreeSet::from([ProfileRole::Implementation]),
            projects: BTreeSet::new(),
        },
        revision: "general-r1".into(),
        evidence: Some("evidence-1".into()),
        candidates: vec![
            candidate("sol", "omo", "openai", "owner-openai"),
            candidate("sol-mini", "omo", "openai", "owner-openai"),
            candidate("opus", "claude", "anthropic", "owner-anthropic"),
        ],
        fallback: Some(AvailabilityFallback {
            automatic: true,
            approved_providers: BTreeSet::from(["openai".into(), "anthropic".into()]),
            max_switches: 2,
            allow_unknown_subscription_cost: true,
            spending: None,
            approval: "proposal-1".into(),
        }),
    };
    let preferred = &metadata.candidates[0];
    let profile = Profile {
        metadata: Some(metadata.clone()),
        harness: preferred.harness.clone(),
        model: Some(preferred.model.clone()),
        vendor: Some(preferred.provider.clone()),
        effort: preferred.effort.clone(),
        args: vec![],
        escalate_to: None,
    };
    metadata.validate(&config, &profile).unwrap();
    config.profiles.insert("general".into(), profile);
    (dir, config, metadata)
}

fn running(metadata: &ProfileMetadata) -> AvailabilityEpisode {
    let mut episode = AvailabilityEpisode::new(
        "T-1-R1-implementation",
        "T-1",
        ProfileRole::Implementation,
        "general",
        metadata,
        t0(),
    );
    episode
        .seed(&metadata.candidates[0], Some("R1"), "preferred", t0())
        .unwrap();
    episode
}

fn fail(episode: &mut AvailabilityEpisode, run: &str, class: AvailabilityClass) {
    let failure = AttemptFailure {
        class,
        message: "provider error".into(),
        retries: 0,
        at: t0(),
        recovered_at: None,
    };
    assert!(episode.record_failure(run, failure).unwrap());
}

fn next_id(selection: &Selection) -> &str {
    match selection {
        Selection::Next { candidate, .. } => &candidate.id,
        other => panic!("expected a candidate, got {other:?}"),
    }
}

#[test]
fn provider_errors_are_classified_by_access_not_by_work() {
    // Given failed-turn errors from providers and from the work itself.
    let now = t0();
    let cases = [
        (
            "429 Too Many Requests; retry after 30s",
            Some(AvailabilityClass::Transient {
                retry_after_secs: Some(30),
            }),
        ),
        (
            "Codex error: Our servers are currently overloaded.",
            Some(AvailabilityClass::Transient {
                retry_after_secs: None,
            }),
        ),
        (
            "You exceeded your current quota, please check your plan and billing details",
            Some(AvailabilityClass::QuotaExhausted {
                account: true,
                reset_at: None,
            }),
        ),
        (
            "quota exceeded for model sol; resets at 2026-10-21T12:00:00Z",
            Some(AvailabilityClass::QuotaExhausted {
                account: false,
                reset_at: Some(at("2026-10-21T12:00:00Z")),
            }),
        ),
        (
            "The model `sol` does not exist or you do not have access to it (model_not_found)",
            Some(AvailabilityClass::Unavailable),
        ),
        (
            "No credential slots available",
            Some(AvailabilityClass::CredentialUnavailable),
        ),
        (
            "401 invalid x-api-key",
            Some(AvailabilityClass::CredentialUnavailable),
        ),
        ("context length exceeded (15000 tokens)", None),
        (
            "invalid_request_error: messages.0: malformed tool block",
            None,
        ),
        ("The model refused to comply with the request", None),
        ("test failed: expected 3, got 4", None),
    ];
    for (message, expected) in cases {
        // When classifying the provider's own error text.
        // Then outages, quota, rate limits and credentials differ from refusals and bad requests.
        assert_eq!(classify(message, now), expected, "{message}");
    }
    assert!(
        !AvailabilityClass::Transient {
            retry_after_secs: None
        }
        .advances_immediately()
    );
    assert!(AvailabilityClass::Unavailable.advances_immediately());
}

#[test]
fn sol_like_quota_exhaustion_substitutes_opus_like_with_its_own_settings() {
    // Given Sol exhausted on a model-scoped quota.
    let (_dir, config, metadata) = general();
    let mut episode = running(&metadata);
    fail(
        &mut episode,
        "R1",
        AvailabilityClass::QuotaExhausted {
            account: false,
            reset_at: None,
        },
    );
    // When the host selects the next approved candidate.
    let selection = episode.select(&config, &metadata, false, None);
    // Then the next ordered candidate is chosen and resolves with its own harness and effort.
    assert_eq!(next_id(&selection), "sol-mini");
    let Selection::Next { candidate, .. } = selection else {
        unreachable!()
    };
    episode.advance(&candidate, "fallback", t0()).unwrap();
    assert_eq!(episode.pending().unwrap().candidate, "sol-mini");
    let opus = config.execution_candidate("general", "opus").unwrap();
    let resolved = config.resolve_candidate("general", opus).unwrap();
    assert_eq!(resolved.profile, "general");
    assert_eq!(resolved.harness, "claude");
    assert_eq!(resolved.vendor.as_deref(), Some("anthropic"));
    assert_eq!(resolved.effort.as_deref(), Some("max"));
    assert!(
        resolved
            .model_args
            .contains(&"anthropic/opus-fixture".to_string())
    );
}

#[test]
fn shared_account_quota_skips_candidates_on_the_same_connection() {
    // Given Sol exhausted on an account-scoped quota shared with Sol-mini.
    let (_dir, config, metadata) = general();
    let mut episode = running(&metadata);
    fail(
        &mut episode,
        "R1",
        AvailabilityClass::QuotaExhausted {
            account: true,
            reset_at: None,
        },
    );
    // When selecting.
    let selection = episode.select(&config, &metadata, false, None);
    // Then the shared-account candidate is skipped with a reason and Opus is chosen.
    assert_eq!(next_id(&selection), "opus");
    let Selection::Next { skipped, .. } = selection else {
        unreachable!()
    };
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].candidate, "sol-mini");
    assert!(
        skipped[0].reason.contains("exhausted account quota"),
        "{}",
        skipped[0].reason
    );
}

#[test]
fn exact_model_pins_and_disabled_policy_block_substitution() {
    // Given a failed preferred candidate.
    let (_dir, config, mut metadata) = general();
    let mut episode = running(&metadata);
    fail(&mut episode, "R1", AvailabilityClass::Unavailable);
    // When the run carries an exact-model pin, or automatic fallback is off.
    let pinned = episode.select(&config, &metadata, true, None);
    metadata.fallback.as_mut().unwrap().automatic = false;
    let disabled = episode.select(&config, &metadata, false, None);
    metadata.fallback = None;
    let unapproved = episode.select(&config, &metadata, false, None);
    // Then substitution is blocked with an explicit reason, never silently.
    for (selection, needle) in [
        (pinned, "exact-model pin"),
        (disabled, "automatic fallback is off"),
        (unapproved, "no approved availability fallback"),
    ] {
        let Selection::Blocked { reason } = selection else {
            panic!("expected blocked for {needle}");
        };
        assert!(reason.contains(needle), "{reason}");
    }
}

#[test]
fn cost_and_provider_policy_reject_candidates_independently() {
    // Given an approved metered bound and an over-budget, then an unapproved, fallback.
    let (_dir, config, mut metadata) = general();
    let fallback = metadata.fallback.as_mut().unwrap();
    fallback.allow_unknown_subscription_cost = false;
    fallback.spending = Some(SpendingBound {
        currency: "USD".into(),
        maximum_micros: 100,
    });
    for candidate in &mut metadata.candidates {
        candidate.cost = CandidateCost::Metered {
            currency: "USD".into(),
            maximum_micros: 50,
        };
    }
    metadata.candidates[1].cost = CandidateCost::Metered {
        currency: "USD".into(),
        maximum_micros: 500,
    };
    metadata.candidates[2].cost = CandidateCost::Unknown;
    let mut episode = running(&metadata);
    fail(&mut episode, "R1", AvailabilityClass::Unavailable);
    // When selecting under the spending policy.
    let selection = episode.select(&config, &metadata, false, None);
    // Then the list is exhausted, naming each rejection, and unknown cost is not free.
    let Selection::Exhausted {
        reason,
        waiting_until,
    } = selection
    else {
        panic!("expected exhaustion, got {selection:?}");
    };
    assert!(reason.contains("sol-mini (candidate exceeds approved spending bound)"));
    assert!(reason.contains("opus (unknown cost requires explicit permission"));
    assert_eq!(waiting_until, None);
    // And the same rule rejects the profile at validation.
    let profile = config.profiles.get("general").unwrap();
    assert!(metadata.validate(&config, profile).is_err());
}

#[test]
fn episodes_are_bounded_never_cycle_and_ignore_redelivered_failures() {
    // Given Sol failed, Opus selected and launched, then Opus fails as well.
    let (_dir, config, metadata) = general();
    let mut episode = running(&metadata);
    fail(
        &mut episode,
        "R1",
        AvailabilityClass::QuotaExhausted {
            account: true,
            reset_at: Some(at("2026-10-21T13:00:00Z")),
        },
    );
    let duplicate = AttemptFailure {
        class: AvailabilityClass::Unavailable,
        message: "redelivered".into(),
        retries: 3,
        at: t0(),
        recovered_at: None,
    };
    assert!(!episode.record_failure("R1", duplicate).unwrap());
    let opus = metadata.candidates[2].clone();
    episode.advance(&opus, "fallback", t0()).unwrap();
    assert!(episode.advance(&opus, "again", t0()).is_err());
    fail(
        &mut episode,
        "R2",
        AvailabilityClass::QuotaExhausted {
            account: false,
            reset_at: Some(at("2026-10-21T14:00:00Z")),
        },
    );
    assert_eq!(episode.current().unwrap().run.as_deref(), Some("R2"));
    // When selecting again.
    let selection = episode.select(&config, &metadata, false, None);
    // Then the list is exhausted without returning to Sol, and the current reset is reported.
    let Selection::Exhausted { waiting_until, .. } = selection else {
        panic!("expected exhaustion, got {selection:?}");
    };
    assert_eq!(waiting_until, Some(at("2026-10-21T14:00:00Z")));
    assert!(
        episode
            .seed(&metadata.candidates[0], None, "cycle", t0())
            .is_err()
    );
    let summary = episode.summary();
    assert!(summary.contains("sol (openai/sol-fixture via owner-openai): account quota exhausted"));
    assert!(summary.contains("opus (anthropic/opus-fixture via owner-anthropic): quota exhausted"));
    // And recovery of the current candidate clears a Waiting exhaustion without forgetting history.
    episode.exhaust("all used", waiting_until, t0());
    assert!(episode.recovered(at("2026-10-21T14:05:00Z")));
    assert_eq!(episode.exhausted, None);
    assert_eq!(episode.attempts.len(), 2);
    let restored: AvailabilityEpisode =
        serde_json::from_str(&serde_json::to_string(&episode).unwrap()).unwrap();
    assert_eq!(restored, episode);
    restored.validate().unwrap();
}

#[test]
fn cross_family_reviewer_fallback_skips_the_implementer_provider() {
    // Given a reviewer profile whose preferred candidate failed while reviewing OpenAI work.
    let (_dir, config, metadata) = general();
    let mut episode = running(&metadata);
    fail(&mut episode, "R1", AvailabilityClass::Unavailable);
    // When the next reviewer candidate must differ from the implementer's family.
    let selection = episode.select(&config, &metadata, false, Some("openai"));
    // Then the same-family candidate is skipped rather than silently downgrading independence.
    assert_eq!(next_id(&selection), "opus");
}

#[test]
fn route_bindings_resolve_their_candidate_and_pins_keep_profile_settings() {
    // Given a route bound to Opus and an exact-model pinned route.
    let (_dir, config, _metadata) = general();
    let mut route = RouteDecision {
        profile: "general".into(),
        harness: "claude".into(),
        model: Some("anthropic/opus-fixture".into()),
        effort: Some("max".into()),
        route: crate::RouteSource::Defaults,
        warnings: vec![],
        availability: Some(CandidateBinding {
            candidate: "opus".into(),
            episode: Some("T-1-R1-implementation".into()),
            reason: Some("fallback".into()),
            skipped: vec![],
            pinned: false,
        }),
    };
    // When resolving frozen execution settings.
    let bound = config.resolve_route(&route).unwrap();
    route.availability = Some(CandidateBinding {
        candidate: "sol".into(),
        episode: None,
        reason: None,
        skipped: vec![],
        pinned: true,
    });
    route.harness = "omo".into();
    route.model = Some("openai/pinned-fixture".into());
    route.effort = None;
    let pinned = config.resolve_route(&route).unwrap();
    // Then the candidate supplies its own harness, and a pin keeps the explicit model.
    assert_eq!(bound.harness, "claude");
    assert_eq!(bound.vendor.as_deref(), Some("anthropic"));
    assert_eq!(pinned.harness, "omo");
    assert_eq!(pinned.model.as_deref(), Some("openai/pinned-fixture"));
}
