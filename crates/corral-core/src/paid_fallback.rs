//! Paid routing is narrower than ordinary availability fallback. Generic throttling,
//! unavailability, auth and paid credit exhaustion never authorize a subscription/API switch.
use crate::{AvailabilityClass, AvailabilityEpisode, ExecutionCandidate, connection::ConnectionId};

pub fn is_api(candidate: &ExecutionCandidate) -> bool {
    ConnectionId::parse(&candidate.connection).is_ok_and(ConnectionId::api_key)
}
pub fn refusal(episode: &AvailabilityEpisode, candidate: &ExecutionCandidate) -> Option<String> {
    if episode
        .attempts
        .iter()
        .any(|a| ConnectionId::parse(&a.connection).is_ok_and(ConnectionId::api_key))
    {
        return Some("the paid connection is sticky; another fallback is not authorized".into());
    }
    if !is_api(candidate) {
        return None;
    }
    let Some(source) = episode.current() else {
        return Some("paid fallback has no subscription predecessor".into());
    };
    let source_connection = ConnectionId::parse(&source.connection)
        .ok()
        .or_else(|| ConnectionId::legacy(&source.provider));
    if !source_connection.is_some_and(|c| {
        matches!(
            c,
            ConnectionId::OpenaiSubscription | ConnectionId::AnthropicSubscription
        ) && c.vendor() == candidate.provider
    }) || !source
        .open_failure()
        .is_some_and(|f| matches!(f.class, AvailabilityClass::QuotaExhausted { .. }))
    {
        return Some(
            "paid fallback requires confirmed same-vendor subscription quota exhaustion".into(),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AttemptFailure, CandidateAttempt, CandidateCost, ProfileRole};
    #[test]
    fn only_same_vendor_subscription_quota_can_select_a_paid_route() {
        let candidate = ExecutionCandidate {
            id: "api".into(),
            harness: "corral-pi".into(),
            connection: "openai-api".into(),
            provider: "openai".into(),
            model: "same-model".into(),
            effort: None,
            args: vec![],
            evidence: "synthetic".into(),
            cost: CandidateCost::Metered {
                currency: "USD".into(),
                maximum_micros: 1,
            },
        };
        let subscription = ExecutionCandidate {
            id: "sub".into(),
            connection: "openai-subscription".into(),
            cost: CandidateCost::Subscription,
            ..candidate.clone()
        };
        let metadata: crate::ProfileMetadata = serde_json::from_value(serde_json::json!({
            "display_name":"fixture", "purpose":"fixture", "guidance":{}, "instructions":"fixture",
            "eligibility":{"enabled":true,"automatic":true,"roles":["implementation"],"projects":[]},
            "revision":"1", "evidence":"synthetic", "candidates":[subscription,candidate],
            "fallback":{"automatic":true,"approved_providers":["openai"],"max_switches":1,
                "allow_unknown_subscription_cost":true,"spending":{"currency":"USD","maximum_micros":1},"approval":"synthetic"}
        })).unwrap();
        let config = crate::Config::default_for(std::path::Path::new("/offline-fixture"));
        let mut episode = AvailabilityEpisode {
            version: 1,
            id: "fixture".into(),
            task: "T-1".into(),
            role: ProfileRole::Implementation,
            profile: "fixture".into(),
            revision: "1".into(),
            started: crate::clock::now(),
            attempts: vec![],
            exhausted: None,
            notice: None,
        };
        assert!(refusal(&episode, &candidate).is_some());
        episode.attempts.push(CandidateAttempt {
            candidate: "sub".into(),
            provider: "openai".into(),
            connection: "openai-subscription".into(),
            model: candidate.model.clone(),
            run: Some("R1".into()),
            selected_at: episode.started,
            reason: "fixture".into(),
            failures: vec![],
        });
        for class in [
            AvailabilityClass::Transient {
                retry_after_secs: None,
            },
            AvailabilityClass::Unavailable,
            AvailabilityClass::CredentialUnavailable,
        ] {
            episode.attempts[0].failures = vec![AttemptFailure {
                class,
                message: "synthetic".into(),
                retries: 0,
                at: episode.started,
                recovered_at: None,
            }];
            assert!(matches!(
                episode.select(&config, &metadata, false, None),
                crate::Selection::Exhausted { .. }
            ));
        }
        episode.attempts[0].failures[0].class = AvailabilityClass::QuotaExhausted {
            account: true,
            reset_at: None,
        };
        assert!(matches!(
            episode.select(&config, &metadata, false, None),
            crate::Selection::Next { .. }
        ));
        episode.attempts[0].connection = "anthropic-subscription".into();
        assert!(matches!(
            episode.select(&config, &metadata, false, None),
            crate::Selection::Exhausted { .. }
        ));
        episode.attempts[0].connection = "openai-subscription".into();
        episode
            .advance(&candidate, "confirmed synthetic quota", episode.started)
            .unwrap();
        let restored: AvailabilityEpisode =
            serde_json::from_slice(&serde_json::to_vec(&episode).unwrap()).unwrap();
        assert_eq!(restored.attempts.len(), 2);
        assert!(matches!(
            restored.select(&config, &metadata, false, None),
            crate::Selection::Exhausted { .. }
        ));
        assert!(refusal(&restored, &candidate).is_some());
    }
}
