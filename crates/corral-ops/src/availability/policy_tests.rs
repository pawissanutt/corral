//! Pure quota evidence keeps unclassified roles on the same native admission contract.
use super::super::{continuation_tests::ROLES, policy};
use super::*;
use corral_core::availability::FailureCategory;
use corral_core::connection::ConnectionId;
use serde_json::json;
#[test]
fn quota_evidence_accepts_unclassified_roles_without_reusing_stale_quota() {
    let now = corral_core::clock::now();
    let binding = ConnectionBinding::new(
        ConnectionId::OpenaiSubscription,
        None,
        "same-model".into(),
        None,
    )
    .unwrap();
    let mut state: corral_core::agent::AgentRunState = serde_json::from_value(json!({
        "version":1,
        "binding":{"role":"assistant","store":"fixture","project":"corral","generation":2,"health":"healthy"},
        "generations":[{
            "n":2,"bind_nonce_sha256":"nonce","pane":"p1","created":now,"connection":binding,
            "bound":{"host_cap_sha256":"host","ctl_cap_sha256":"control","session_id":"session",
                "session_file":"session.jsonl","pid":1,"start_time":"start","request_id":"hello","at":now}
        }],
        "turns":[{
            "generation":2,"seq":1,"origin":{"kind":"bridge"},"started":now,"ended":now,
            "provider_failure":{"version":1,"connection":"openai-subscription",
                "category":"subscription_quota","scope":"account"}
        }]
    }))
    .unwrap();
    for role in ROLES {
        state.binding.role = role;
        assert_eq!(
            policy::quota_evidence(&state, None).is_some(),
            !matches!(role, AgentRole::Worker | AgentRole::Reviewer),
            "{role:?}"
        );
    }
    state.binding.role = AgentRole::Assistant;
    let mut latest = state.turns[0].clone();
    latest.seq = 2;
    latest.provider_failure.as_mut().unwrap().category = FailureCategory::Transient;
    state.turns.push(latest);
    assert!(policy::quota_evidence(&state, None).is_none());
    let failure = state.turns[1].provider_failure.as_mut().unwrap();
    failure.category = FailureCategory::SubscriptionQuota;
    failure.connection = ConnectionId::AnthropicSubscription;
    assert!(policy::quota_evidence(&state, None).is_none());
    state.turns.pop();
    state.turns[0].generation = 1;
    assert!(policy::quota_evidence(&state, None).is_none());
}
