use std::collections::BTreeSet;

use corral_core::{
    AvailabilityFallback, CandidateCost, ExecutionCandidate, Harness, Profile, ProfileChoice,
    ProfileEligibility, ProfileMetadata, ProfileRole, SelectionGuidance, Store,
};

use super::*;

fn candidate(id: &str, provider: &str) -> ExecutionCandidate {
    ExecutionCandidate {
        id: id.into(),
        harness: "omo".into(),
        connection: format!("owner-{provider}"),
        provider: provider.into(),
        model: format!("{provider}/{id}-fixture"),
        effort: Some("high".into()),
        args: vec![],
        evidence: "fixture-evidence".into(),
        cost: CandidateCost::Subscription,
    }
}

#[test]
fn pm_sessions_open_on_the_next_approved_candidate_when_the_preferred_is_unavailable() {
    // Given a PM profile preferring Sol, with only Opus enabled locally.
    let tmp = tempfile::tempdir().unwrap();
    let agent = tmp.path().join("agent");
    std::fs::create_dir(&agent).unwrap();
    std::fs::write(
        agent.join("settings.json"),
        r#"{"enabledModels":["anthropic/pm-opus-fixture"]}"#,
    )
    .unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.sync.omo_agent_dir = Some(agent);
    store.config.harnesses.insert(
        "omo".into(),
        Harness {
            command: "true {model_args}".into(),
            flags: Some("omo".into()),
            model_flag: None,
            effort_flag: None,
            efforts: vec![],
            prompt: None,
        },
    );
    let candidates = vec![
        candidate("pm-sol", "openai"),
        candidate("pm-opus", "anthropic"),
    ];
    let preferred = candidates[0].clone();
    store.config.profiles.insert(
        "coordinator".into(),
        Profile {
            harness: preferred.harness.clone(),
            model: Some(preferred.model.clone()),
            vendor: Some(preferred.provider.clone()),
            effort: preferred.effort.clone(),
            args: vec![],
            escalate_to: None,
            metadata: Some(ProfileMetadata {
                display_name: "Coordinator".into(),
                purpose: "Workspace PM".into(),
                guidance: SelectionGuidance::default(),
                instructions: "Coordinate".into(),
                eligibility: ProfileEligibility {
                    enabled: true,
                    automatic: true,
                    roles: BTreeSet::from([ProfileRole::Pm]),
                    projects: BTreeSet::new(),
                },
                revision: "pm-r1".into(),
                evidence: Some("fixture-evidence".into()),
                candidates,
                fallback: Some(AvailabilityFallback {
                    automatic: true,
                    approved_providers: BTreeSet::from(["openai".into(), "anthropic".into()]),
                    max_switches: 1,
                    allow_unknown_subscription_cost: true,
                    spending: None,
                    approval: "fixture-approval".into(),
                }),
            }),
        },
    );
    let ctx = Ctx {
        resource_probe: crate::resources::FixedResourceProbe::arc(),
        herdr: corral_herdr::Herdr::with_bin(tmp.path().join("unused-herdr"), None),
        store,
    };
    let preferred = ctx
        .store
        .config
        .resolve_profile(
            &ProfileChoice {
                profile: Some("coordinator".into()),
                ..ProfileChoice::default()
            },
            None,
            None,
        )
        .unwrap();
    // When the PM session resolves its launch settings.
    let resolved = resolve_before_start(&ctx, ProfileRole::Pm, preferred, None).unwrap();
    // Then the host, not the unavailable model, selects the approved alternative.
    assert_eq!(resolved.profile, "coordinator");
    assert_eq!(resolved.model.as_deref(), Some("anthropic/pm-opus-fixture"));
    let events = ctx.store.events().unwrap();
    let switched: Vec<_> = events
        .iter()
        .filter(|event| event.kind == "availability_switched")
        .collect();
    assert_eq!(switched.len(), 1);
    assert_eq!(switched[0].detail["candidate"], "pm-opus");
}
