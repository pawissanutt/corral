use super::*;

#[test]
fn stale_proposal_can_be_cancelled_after_harness_support_is_removed() {
    // Given a proposal whose once-supported harness was changed by another writer.
    let (_dir, store, input) = fixture();
    let record = proposed(&store, &input);
    store
        .edit_config(|doc| doc.set_str("harnesses.omo.flags", "generic"))
        .unwrap();
    let changed = fs::read(store.config_path()).unwrap();
    assert!(setup_validate(&store, &input).is_err());
    // When cancelling the stale proposal without applying its former settings.
    let cancelled = setup_cancel(&store, record.revision).unwrap();
    // Then history remains inspectable and the current active configuration is preserved.
    assert_eq!(cancelled.state, ProfileSetupState::Cancelled);
    assert_eq!(cancelled.proposal, record.proposal);
    assert_eq!(store.profile_setup().unwrap(), cancelled);
    assert_eq!(fs::read(store.config_path()).unwrap(), changed);
}

#[test]
fn configured_binding_flags_and_short_aliases_cannot_override_exact_model_identity() {
    // Given supported extra argv that attempts to replace a separately recorded model.
    for arg in ["--engine-model=openai/unapproved", "-mother-model"] {
        let (_dir, mut store, mut input) = fixture();
        store.config.harnesses.get_mut("omo").unwrap().model_flag = Some("--engine-model".into());
        store.save_config().unwrap();
        input.proposal.based_on_config_revision = store.config.profile_setup_revision().unwrap();
        let profile = input.proposal.profiles.get_mut("database").unwrap();
        profile.args = vec![arg.into()];
        profile.metadata.as_mut().unwrap().candidates[0].args = vec![arg.into()];
        input.discovery.candidates[0].settings[0].args = vec![arg.into()];
        // When validating through the real configuration and discovery services.
        let result = setup_validate(&store, &input);
        // Then even an explicitly observed argv cannot override the approved binding.
        assert!(result.is_err(), "{arg}");
    }
}

#[test]
fn discovery_support_cannot_be_rebound_to_an_existing_proposal_digest() {
    // Given saved support facts for a proposed mapping.
    let (_dir, store, mut input) = fixture();
    let record = proposed(&store, &input);
    let before = fs::read(store.config_path()).unwrap();
    input.discovery.candidates[0]
        .settings
        .push(CandidateSettings {
            effort: Some("low".into()),
            args: vec![],
        });
    // When a writer attempts to replace the support facts without revising the proposal.
    let result = setup_propose(&store, &input, record.revision);
    // Then the input is immutable and no proposal/config revision is committed.
    assert!(result.is_err());
    assert_eq!(store.profile_setup().unwrap(), record);
    assert_eq!(fs::read(store.config_path()).unwrap(), before);
}

#[test]
fn corrupted_support_facts_cannot_publish_an_approved_mapping() {
    // Given approved setup whose persisted support facts lost the model settings.
    let (_dir, store, input) = fixture();
    let record = approved(&store, &input);
    let before = fs::read(store.config_path()).unwrap();
    let path = store
        .state_dir()
        .join("profile-proposals")
        .join(format!("{}.json", input.proposal.digest().unwrap()));
    let mut changed = input;
    changed.discovery.candidates[0].settings.clear();
    fs::write(path, serde_json::to_vec(&changed).unwrap()).unwrap();
    // When applying after restart against the real corrupted support file.
    let reopened = Store::open_unobserved(store.root()).unwrap();
    let result = setup_apply(&reopened);
    // Then approval alone cannot replace missing support or claim an applied result.
    assert!(result.is_err());
    assert_eq!(store.profile_setup().unwrap(), record);
    assert_eq!(fs::read(store.config_path()).unwrap(), before);
    assert!(!store.state_dir().join("profile-setup-intent.json").exists());
}
