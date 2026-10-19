use std::collections::BTreeSet;

use corral_core::{
    AutoProfileSelection, PROFILE_RECORD_VERSION, Profile, ProfileEligibility, ProfileMetadata,
    ProfileRole, SelectionActor, SelectionGuidance, Status, Store, Task,
};
use corral_ops::{SelectionCatalog, selection_catalog, selection_save};

fn raw_config(store: &Store) {
    let mut doc = corral_core::ConfigDoc::default();
    doc.sync_from(&store.config).unwrap();
    std::fs::write(store.dir().join("corral.toml"), doc.to_string()).unwrap();
}

fn fixture() -> (tempfile::TempDir, Store, Task) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::init(dir.path()).unwrap();
    store.config.sync.omo_agent_dir = Some(dir.path().join("agent"));
    store.config.projects.clear();
    store.config.profiles.clear();
    let profile = Profile {
        harness: "omo".into(),
        model: Some("openai/fixture-v1".into()),
        vendor: Some("openai".into()),
        effort: Some("high".into()),
        args: vec![],
        escalate_to: None,
        metadata: Some(ProfileMetadata {
            display_name: "Database".into(),
            purpose: "Persistence correctness".into(),
            guidance: SelectionGuidance {
                use_when: vec!["Transaction boundaries dominate".into()],
                avoid_when: vec!["Prose only".into()],
                positive_examples: vec!["Repair rollback".into()],
                negative_examples: vec!["Document SQL".into()],
                distinctions: vec!["General follows ordinary patterns".into()],
            },
            instructions: "Trace transaction boundaries".into(),
            eligibility: ProfileEligibility {
                enabled: true,
                automatic: true,
                roles: BTreeSet::from([ProfileRole::Implementation, ProfileRole::Planning]),
                projects: BTreeSet::new(),
            },
            revision: "database-r1".into(),
            evidence: None,
            candidates: vec![],
            fallback: None,
        }),
    };
    store
        .config
        .profiles
        .insert("database".into(), profile.clone());
    store.config.profiles.insert(
        "general".into(),
        Profile {
            metadata: None,
            ..profile
        },
    );
    store.config.pm.agent = "general".into();
    store.config.defaults.profile = Some("general".into());
    store.save_config().unwrap();
    let task = store
        .create(
            "Repair persistence",
            None,
            Status::Backlog,
            "Prove rollback is atomic",
        )
        .unwrap();
    let task = store.get(&task.id).unwrap();
    (dir, store, task)
}

fn receipt(catalog: &SelectionCatalog) -> AutoProfileSelection {
    AutoProfileSelection {
        version: PROFILE_RECORD_VERSION,
        id: "selection-1".into(),
        task: catalog.task.id.clone(),
        actor: SelectionActor::Proposer,
        selected_at: "2026-10-19T00:00:00Z".parse().unwrap(),
        role: catalog.role,
        profile: "database".into(),
        profile_revision: catalog.eligible["database"].profile_revision.clone(),
        spec_revision: catalog.spec_revision.clone(),
        catalog_revision: catalog.catalog_revision.clone(),
        reason: "Transaction correctness dominates".into(),
        closest_alternative: Some("general".into()),
        distinction: Some("Not ordinary pattern-following".into()),
        review: None,
    }
}

#[test]
fn catalog_returns_complete_custom_guidance_and_revision_identities() {
    let (_dir, store, task) = fixture();
    let before_events = std::fs::read(store.dir().join("events.jsonl")).unwrap();
    let catalog = selection_catalog(&store, &task.id, ProfileRole::Implementation).unwrap();
    assert_eq!(catalog.task, store.get(&task.id).unwrap());
    assert_eq!(
        catalog.spec_revision,
        corral_core::plan_contract_revision(&task).unwrap()
    );
    assert_eq!(
        catalog.catalog_revision,
        store.config.profile_catalog_revision().unwrap()
    );
    assert_eq!(
        catalog.config_revision,
        store.config.profile_setup_revision().unwrap()
    );
    assert_eq!(
        catalog.eligible["database"].profile,
        store.config.profiles["database"]
    );
    assert_eq!(catalog.eligible["general"].profile_revision, "legacy");
    assert_eq!(catalog.access_status, "not_checked");
    assert_eq!(
        before_events,
        std::fs::read(store.dir().join("events.jsonl")).unwrap()
    );
}

#[test]
fn locally_known_unavailable_access_is_excluded_without_probing() {
    // Given enabled models but credentials for only one connection in private fixture files.
    let (_dir, store, task) = fixture();
    let agent = store.config.sync.omo_agent_dir.as_ref().unwrap();
    std::fs::create_dir_all(agent).unwrap();
    std::fs::write(
        agent.join("settings.json"),
        r#"{"enabledModels":["openai/fixture-v1","anthropic/other"]}"#,
    )
    .unwrap();
    std::fs::write(
        agent.join("auth.json"),
        r#"{"anthropic":{"type":"oauth","access":"fixture"}}"#,
    )
    .unwrap();
    // When reading the current selector catalog from existing local observations.
    let catalog = selection_catalog(&store, &task.id, ProfileRole::Implementation).unwrap();
    // Then missing access cannot be promoted by profile prose or by a forged receipt.
    assert!(!catalog.eligible.contains_key("database"));
    assert!(catalog.excluded.contains_key("database"));
}

#[test]
fn catalog_separates_all_code_eligibility_and_unsupported_configuration_failures() {
    let (_dir, mut store, task) = fixture();
    let base = store.config.profiles["database"].clone();
    for key in [
        "disabled",
        "manual",
        "review-only",
        "other-project",
        "bad-effort",
        "bad-harness",
        "bad-candidate",
    ] {
        let mut profile = base.clone();
        let metadata = profile.metadata.as_mut().unwrap();
        match key {
            "disabled" => metadata.eligibility.enabled = false,
            "manual" => metadata.eligibility.automatic = false,
            "review-only" => {
                metadata.eligibility.roles = BTreeSet::from([ProfileRole::ImplementationReview])
            }
            "other-project" => metadata.eligibility.projects = BTreeSet::from(["elsewhere".into()]),
            "bad-effort" => profile.effort = Some("unsupported".into()),
            "bad-harness" => profile.harness = "missing".into(),
            "bad-candidate" => metadata.revision.clear(),
            _ => unreachable!(),
        }
        store.config.profiles.insert(key.into(), profile);
    }
    raw_config(&store);
    let catalog = selection_catalog(&store, &task.id, ProfileRole::Implementation).unwrap();
    for key in [
        "disabled",
        "manual",
        "review-only",
        "other-project",
        "bad-effort",
        "bad-harness",
        "bad-candidate",
    ] {
        assert!(!catalog.eligible.contains_key(key), "{key}");
        assert!(!catalog.excluded[key][0].is_empty(), "{key}");
        let mut input = receipt(&catalog);
        input.profile = key.into();
        input.profile_revision = base.metadata.as_ref().unwrap().revision.clone();
        assert!(selection_save(&store, &input).is_err(), "{key}");
    }
}

#[test]
fn save_preserves_proposer_manual_pin_and_authority_and_is_idempotent() {
    let (_dir, store, task) = fixture();
    store
        .update(&task.id, |task| {
            task.profile = Some("general".into());
            Ok(())
        })
        .unwrap();
    let catalog = selection_catalog(&store, &task.id, ProfileRole::Implementation).unwrap();
    let input = receipt(&catalog);
    let task_before = store.get(&task.id).unwrap();
    let config_before = std::fs::read(store.dir().join("corral.toml")).unwrap();
    assert_eq!(selection_save(&store, &input).unwrap(), input);
    selection_save(&store, &input).unwrap();
    let fresh = store.reopen_unobserved().unwrap();
    assert_eq!(
        fresh.auto_profile_selections().unwrap(),
        vec![input.clone()]
    );
    assert_eq!(fresh.get(&task.id).unwrap(), task_before);
    assert_eq!(
        std::fs::read(store.dir().join("corral.toml")).unwrap(),
        config_before
    );
    let mut conflict = input;
    conflict.actor = SelectionActor::Pm;
    assert!(selection_save(&store, &conflict).is_err());
}

#[test]
fn saves_revalidate_task_catalog_profile_and_supported_settings_after_reopen() {
    let (_dir, mut store, task) = fixture();
    let input = receipt(&selection_catalog(&store, &task.id, ProfileRole::Implementation).unwrap());
    selection_save(&store, &input).unwrap();
    store
        .update(&task.id, |task| {
            task.body.push_str("\nAlso preserve existing rows");
            Ok(())
        })
        .unwrap();
    assert!(
        selection_save(&store, &input)
            .unwrap_err()
            .to_string()
            .contains("stale task")
    );
    let current = selection_catalog(&store, &task.id, ProfileRole::Implementation).unwrap();
    let mut input = receipt(&current);
    input.id = "selection-2".into();
    store
        .config
        .profiles
        .get_mut("database")
        .unwrap()
        .metadata
        .as_mut()
        .unwrap()
        .guidance
        .use_when
        .push("New guidance".into());
    store.save_config().unwrap();
    assert!(
        selection_save(&store, &input)
            .unwrap_err()
            .to_string()
            .contains("stale profile catalog")
    );
    let current = selection_catalog(&store, &task.id, ProfileRole::Implementation).unwrap();
    let mut input = receipt(&current);
    input.profile_revision = "wrong".into();
    assert!(selection_save(&store, &input).is_err());
    // Harness edits do not change the core catalog digest; service support checks still reject.
    input.profile_revision = current.eligible["database"].profile_revision.clone();
    let mut fresh = store.reopen_unobserved().unwrap();
    fresh.config.harnesses.get_mut("omo").unwrap().efforts = vec!["low".into()];
    raw_config(&fresh);
    assert!(selection_save(&store, &input).is_err());
}

#[test]
fn planning_pm_and_review_roles_are_not_confused_and_overlap_is_validated() {
    let (_dir, store, task) = fixture();
    let catalog = selection_catalog(&store, &task.id, ProfileRole::Planning).unwrap();
    let mut input = receipt(&catalog);
    input.actor = SelectionActor::Pm;
    selection_save(&store, &input).unwrap();
    assert_eq!(
        store.auto_profile_selections().unwrap()[0].actor,
        SelectionActor::Pm
    );
    for role in [
        ProfileRole::Pm,
        ProfileRole::PlanReview,
        ProfileRole::ImplementationReview,
    ] {
        assert!(selection_catalog(&store, &task.id, role).is_err());
        input.role = role;
        assert!(selection_save(&store, &input).is_err());
    }
    input.role = ProfileRole::Planning;
    input.id = "other".into();
    input.distinction = None;
    assert!(selection_save(&store, &input).is_err());
    input.distinction = Some("Different focus".into());
    input.closest_alternative = Some("database".into());
    assert!(selection_save(&store, &input).is_err());
    input.closest_alternative = Some("unknown".into());
    assert!(selection_save(&store, &input).is_err());
    assert_eq!(store.auto_profile_selections().unwrap().len(), 1);
}
