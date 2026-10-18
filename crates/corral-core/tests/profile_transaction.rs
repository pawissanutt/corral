use std::{cell::Cell, fs};

use corral_core::*;

#[path = "profile_transaction/support.rs"]
mod support;
use support::*;

#[test]
fn apply_rejects_stale_relevant_configuration() {
    // Given an approved proposal followed by a relevant edit.
    let (_dir, store, record) = fixture();
    store
        .edit_config(|doc| doc.set_str("pm.agent", "claude"))
        .unwrap();
    let calls = Cell::new(0);
    // When applying the stale proposal.
    let result = store.apply_profile_setup(|p, c| {
        calls.set(calls.get() + 1);
        valid(p, c)
    });
    // Then neither validation nor application runs.
    assert!(result.is_err());
    assert_eq!(calls.get(), 0);
    assert_eq!(store.profile_setup().unwrap(), record);
    assert!(
        !Store::open(store.root())
            .unwrap()
            .config
            .profiles
            .contains_key("custom")
    );
    assert!(store.profile_setup_diff().unwrap().stale);
}

#[test]
fn apply_preserves_unrelated_concurrent_configuration_and_comments() {
    // Given an approved proposal and an unrelated edit after the Store snapshot.
    let (_dir, store, _) = fixture();
    let path = store.dir().join("corral.toml");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        format!("# owner comment\n{text}\n# trailing owner comment\n"),
    )
    .unwrap();
    store
        .edit_config(|doc| doc.set_str("limits.running", "19"))
        .unwrap();
    // When applying, validation receives the fresh effective config.
    let applied = store
        .apply_profile_setup(|p, c| {
            assert_eq!(c.limits.running, 19);
            valid(p, c)
        })
        .unwrap();
    // Then only targeted mappings change, retaining unrelated values and comments.
    let fresh = Store::open(store.root()).unwrap();
    assert_eq!(fresh.config.limits.running, 19);
    assert_eq!(fresh.config.defaults.profile.as_deref(), Some("custom"));
    assert_eq!(
        fresh.config.routing.role_profiles[&ProfileRole::Implementation],
        "custom"
    );
    assert_eq!(applied.state, ProfileSetupState::Applied);
    assert!(!applied.validation.execution_tested);
    let text = fs::read_to_string(path).unwrap();
    assert!(text.contains("# owner comment"));
    assert!(text.contains("# trailing owner comment"));
}

#[test]
fn missing_approval_cannot_apply() {
    // Given a persisted proposal without approval.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let mut record = proposed(&store, proposal(&store.config));
    record.revision += 1;
    record.approval = None;
    store.save_profile_setup(&record).unwrap();
    // When applying without a persisted exact receipt.
    let result = store.apply_profile_setup(valid);
    // Then config is not changed or labelled Applied.
    assert!(result.is_err());
    assert_eq!(store.profile_setup().unwrap(), record);
    assert!(!store.state_dir().join("profile-setup-intent.json").exists());
}

#[test]
fn mismatched_approval_is_rejected_before_persistence() {
    // Given an approved persisted proposal.
    let (_dir, store, record) = fixture();
    let mut altered = record.clone();
    altered.revision += 1;
    altered.approval.as_mut().unwrap().proposal_digest = "different".into();
    // When saving a receipt that does not bind the exact proposal.
    let result = store.save_profile_setup(&altered);
    // Then the original approval remains the only persisted approval.
    assert!(result.is_err());
    assert_eq!(store.profile_setup().unwrap(), record);
}

#[test]
fn manual_basis_cannot_use_transactional_apply() {
    // Given a valid manual proposal and exact approval.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let mut p = proposal(&store.config);
    p.basis = SetupBasis::Manual;
    let record = proposed(&store, p);
    // When applying through the evidence-backed setup path.
    let result = store.apply_profile_setup(valid);
    // Then manual configuration is not represented as applied setup.
    assert!(result.is_err());
    assert_eq!(store.profile_setup().unwrap(), record);
}

#[test]
fn validation_failure_cannot_publish_an_intent_or_applied_record() {
    // Given an approved proposal.
    let (_dir, store, record) = fixture();
    // When the fresh validator rejects it.
    let result = store.apply_profile_setup(|_, _| Ok(SetupValidationFacets::default()));
    // Then no apply boundary has been crossed.
    assert!(result.is_err());
    assert_eq!(store.profile_setup().unwrap(), record);
    assert!(!store.state_dir().join("profile-setup-intent.json").exists());
}

#[test]
fn intent_only_interruption_resumes_without_replacing_the_proposal() {
    // Given an exact approval and a deterministic config-write fault after intent persistence.
    let (_dir, store, record) = fixture();
    let fault = store.dir().join("corral.toml.tmp");
    let result = store.apply_profile_setup(|p, c| {
        fs::create_dir(&fault).unwrap();
        valid(p, c)
    });
    assert!(result.is_err());
    assert!(
        store
            .state_dir()
            .join("profile-setup-intent.json")
            .is_file()
    );
    assert_eq!(store.profile_setup().unwrap(), record);
    assert!(
        !Store::open(store.root())
            .unwrap()
            .config
            .profiles
            .contains_key("custom")
    );
    let mut cancelled = record.clone();
    cancelled.revision += 1;
    cancelled.state = ProfileSetupState::Cancelled;
    cancelled.approval = None;
    assert!(store.save_profile_setup(&cancelled).is_err());
    let mut replacement = record;
    replacement.revision += 1;
    replacement.proposal.as_mut().unwrap().id = "replacement".into();
    replacement.approval = Some(approval(replacement.proposal.as_ref().unwrap()));
    assert!(store.save_profile_setup(&replacement).is_err());
    fs::remove_dir(fault).unwrap();
    // When retrying after an unrelated intervening edit.
    store
        .edit_config(|doc| doc.set_str("limits.running", "23"))
        .unwrap();
    let applied = Store::open(store.root())
        .unwrap()
        .apply_profile_setup(valid)
        .unwrap();
    // Then the same approval finishes and the intervening edit survives.
    assert_eq!(applied.state, ProfileSetupState::Applied);
    assert_eq!(Store::open(store.root()).unwrap().config.limits.running, 23);
}

#[test]
fn config_written_interruption_finishes_without_overwriting_new_edits() {
    // Given a journal-append fault injected before config persistence.
    let (_dir, store, record) = fixture();
    let fault = store.state_dir().join("profile-setup.json.tmp");
    assert!(
        store
            .apply_profile_setup(|p, c| {
                fs::create_dir(&fault).unwrap();
                valid(p, c)
            })
            .is_err()
    );
    assert_eq!(store.profile_setup().unwrap(), record);
    assert_eq!(
        Store::open(store.root())
            .unwrap()
            .config
            .defaults
            .profile
            .as_deref(),
        Some("custom")
    );
    fs::remove_dir(fault).unwrap();
    store
        .edit_config(|doc| doc.set_str("limits.running", "29"))
        .unwrap();
    // When resuming from the after revision.
    let applied = Store::open(store.root())
        .unwrap()
        .apply_profile_setup(valid)
        .unwrap();
    // Then an Applied receipt follows actual config and preserves concurrent unrelated edits.
    assert_eq!(applied.state, ProfileSetupState::Applied);
    assert_eq!(store.profile_setup_history().unwrap().len(), 3);
    assert_eq!(Store::open(store.root()).unwrap().config.limits.running, 29);
}

#[test]
fn pending_apply_rejects_a_third_relevant_revision() {
    // Given persisted intent but no config commit.
    let (_dir, store, record) = fixture();
    let fault = store.dir().join("corral.toml.tmp");
    assert!(
        store
            .apply_profile_setup(|p, c| {
                fs::create_dir(&fault).unwrap();
                valid(p, c)
            })
            .is_err()
    );
    fs::remove_dir(fault).unwrap();
    store
        .edit_config(|doc| doc.set_str("pm.agent", "claude"))
        .unwrap();
    // When retrying after a relevant edit distinct from both intent revisions.
    let result = store.apply_profile_setup(valid);
    // Then recovery cannot overwrite that edit or claim completion.
    assert!(result.is_err());
    assert_eq!(store.profile_setup().unwrap(), record);
    assert_eq!(Store::open(store.root()).unwrap().config.pm.agent, "claude");
}

#[test]
fn applied_replay_retains_history_without_reapplying_later_config_edits() {
    // Given an Applied receipt followed by a relevant config edit.
    let (_dir, store, _) = fixture();
    let applied = store.apply_profile_setup(valid).unwrap();
    store
        .edit_config(|doc| doc.set_str("defaults.profile", "claude"))
        .unwrap();
    // When replaying the already completed apply.
    let replay = Store::open(store.root())
        .unwrap()
        .apply_profile_setup(|_, _| {
            panic!("Applied replay must not revalidate or rewrite");
        })
        .unwrap();
    // Then the historical receipt remains identical without overwriting the edit.
    assert_eq!(replay, applied);
    assert_eq!(store.profile_setup().unwrap(), applied);
    assert_eq!(store.profile_setup_history().unwrap().len(), 3);
    assert_eq!(
        Store::open(store.root())
            .unwrap()
            .config
            .defaults
            .profile
            .as_deref(),
        Some("claude")
    );
}

#[test]
fn journal_history_prevents_reusing_a_proposal_id_with_altered_contents() {
    // Given an old proposal followed by another proposal identity.
    let (_dir, store, record) = fixture();
    let mut next = record.clone();
    next.revision += 1;
    next.proposal.as_mut().unwrap().id = "p2".into();
    next.approval = None;
    store.save_profile_setup(&next).unwrap();
    // When trying to recycle the original ID with different contents.
    next.revision += 1;
    next.proposal = record.proposal.clone();
    next.proposal.as_mut().unwrap().procedure_version = "changed".into();
    let result = store.save_profile_setup(&next);
    // Then historical identity, not only the latest proposal, is immutable.
    assert!(result.is_err());
    assert_eq!(store.profile_setup_history().unwrap().len(), 3);
}

#[test]
fn approval_identity_cannot_be_reused_for_altered_receipt() {
    // Given a persisted exact approval.
    let (_dir, store, record) = fixture();
    let mut next = record.clone();
    next.revision += 1;
    next.approval.as_mut().unwrap().acknowledged_evidence_gap = true;
    // When reusing the approval ID for an otherwise valid changed receipt.
    let result = store.save_profile_setup(&next);
    // Then the approval identity remains immutable.
    assert!(result.is_err());
    assert_eq!(store.profile_setup().unwrap(), record);
}

#[test]
fn save_cannot_fabricate_an_applied_record_without_config() {
    // Given an approved proposal whose mapping has not been committed.
    let (_dir, store, record) = fixture();
    let mut fake = record.clone();
    fake.revision += 1;
    fake.state = ProfileSetupState::Applied;
    fake.validation.configuration_valid = true;
    fake.applied_config_revision = Some("invented".into());
    // When attempting to publish Applied through receipt saving alone.
    let result = store.save_profile_setup(&fake);
    // Then only the transactional path can attest application.
    assert!(result.is_err());
    assert_eq!(store.profile_setup().unwrap(), record);
}

#[test]
fn diff_reports_exact_targets_and_binding_side_effects_without_writes() {
    // Given an approved setup proposal.
    let (_dir, store, record) = fixture();
    let before = fs::read(store.dir().join("corral.toml")).unwrap();
    // When requesting a machine-readable diff.
    let diff = store.profile_setup_diff().unwrap();
    // Then all targeted changes and revisions are exact, without applying anything.
    assert!(!diff.stale);
    assert_eq!(diff.profiles["custom"].before, None);
    assert_eq!(diff.profiles["custom"].after, Some(profile("openai")));
    assert_eq!(diff.roles[&ProfileRole::Implementation].before, None);
    assert_eq!(
        diff.roles[&ProfileRole::Implementation].after.as_deref(),
        Some("custom")
    );
    assert_eq!(
        diff.default_profile.unwrap().after.as_deref(),
        Some("custom")
    );
    assert_eq!(
        diff.repair.unwrap().after,
        Some(RepairPolicy::approved_starter_policy())
    );
    assert_eq!(
        diff.proposed_config_revision,
        store
            .config
            .with_profile_setup(record.proposal.as_ref().unwrap())
            .unwrap()
            .profile_setup_revision()
            .unwrap()
    );
    assert_eq!(fs::read(store.dir().join("corral.toml")).unwrap(), before);
    assert_eq!(store.profile_setup().unwrap(), record);
    assert!(!store.state_dir().join("profile-setup-intent.json").exists());
}

#[path = "profile_transaction/boundaries.rs"]
mod boundaries;
#[path = "profile_transaction/policy.rs"]
mod policy;
