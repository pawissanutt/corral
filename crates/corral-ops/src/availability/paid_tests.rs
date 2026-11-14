use corral_core::{
    AvailabilityFallback, CandidateCost, Config, ExecutionCandidate, ExecutionSnapshot, Profile,
    ProfileEligibility, ProfileMetadata, ProfileRole, SelectionGuidance,
};
use std::collections::BTreeSet;

#[test]
fn same_model_candidates_match_the_frozen_connection_not_list_order() {
    let mut config = Config::default_for(std::path::Path::new("/offline-fixture"));
    let candidate = |id: &str, connection: &str, cost| ExecutionCandidate {
        id: id.into(),
        harness: "corral-pi".into(),
        connection: connection.into(),
        provider: "openai".into(),
        model: "same-model".into(),
        effort: None,
        args: vec![],
        evidence: "synthetic".into(),
        cost,
    };
    let sub = candidate("sub", "openai-subscription", CandidateCost::Subscription);
    let api = candidate(
        "api",
        "openai-api",
        CandidateCost::Metered {
            currency: "USD".into(),
            maximum_micros: 1,
        },
    );
    let metadata = ProfileMetadata {
        display_name: "fixture".into(),
        purpose: "fixture".into(),
        guidance: SelectionGuidance::default(),
        instructions: "fixture".into(),
        eligibility: ProfileEligibility {
            enabled: true,
            automatic: true,
            roles: BTreeSet::from([ProfileRole::Implementation]),
            projects: BTreeSet::new(),
        },
        revision: "fixture".into(),
        evidence: Some("synthetic".into()),
        candidates: vec![sub, api.clone()],
        fallback: Some(AvailabilityFallback {
            automatic: true,
            approved_providers: BTreeSet::from(["openai".into()]),
            max_switches: 1,
            allow_unknown_subscription_cost: true,
            spending: Some(corral_core::SpendingBound {
                currency: "USD".into(),
                maximum_micros: 1,
            }),
            approval: "synthetic".into(),
        }),
    };
    let profile: Profile = serde_json::from_value(serde_json::json!({"harness":"corral-pi", "vendor":"openai", "model":"same-model", "metadata":metadata})).unwrap();
    config.profiles.insert("fixture".into(), profile);
    let mut run = crate::agent::test_support::run("R1", "fixture");
    run.harness = Some("corral-pi".into());
    run.model = Some("same-model".into());
    run.execution = Some(ExecutionSnapshot::new(
        &config,
        config.resolve_candidate("fixture", &api).unwrap(),
    ));
    assert_eq!(super::super::matching(&metadata, &run).unwrap().id, "api");
    run.execution = None;
    assert!(super::super::matching(&metadata, &run).is_none());
}
