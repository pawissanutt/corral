use super::*;

#[test]
fn failed_intent_publication_never_reaches_configuration() {
    // Given persisted approval but a deterministic failure of the atomic intent write.
    let (_dir, store, record) = fixture();
    let (store, fault) = fail_records(&store, "profile-setup-intent", "current");
    let before = fs::read(store.dir().join("corral.toml")).unwrap();
    // When the intent cannot be published.
    let result = store.apply_profile_setup(valid);
    // Then config and the journal remain unapplied; fixing storage permits a fresh retry.
    assert!(result.is_err());
    assert_eq!(fs::read(store.dir().join("corral.toml")).unwrap(), before);
    assert_eq!(store.profile_setup().unwrap(), record);
    assert!(
        store
            .record_row("profile-setup-intent", "current")
            .unwrap()
            .is_none()
    );
    fault.store(false, Ordering::SeqCst);
    assert_eq!(
        store.apply_profile_setup(valid).unwrap().state,
        ProfileSetupState::Applied
    );
}

#[test]
fn config_written_recovery_rejects_relevant_edits_without_reporting_applied() {
    // Given a committed mapping whose Applied append was interrupted.
    let (_dir, store, record) = fixture();
    let (store, fault) = fail_records(&store, "profile-setup", "current");
    assert!(store.apply_profile_setup(valid).is_err());
    fault.store(false, Ordering::SeqCst);
    store
        .edit_config(|doc| doc.set_str("defaults.profile", "claude"))
        .unwrap();
    // When retrying after a third relevant revision.
    let result = store.apply_profile_setup(valid);
    // Then recovery preserves the new mapping without inventing an Applied receipt.
    assert!(result.is_err());
    assert_eq!(store.profile_setup().unwrap(), record);
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
fn identical_proposal_reintroduced_later_is_historical_replay_not_a_new_write() {
    // Given a completed proposal, a fresh setup cycle and a later owner binding edit.
    let (_dir, store, record) = fixture();
    let applied = store.apply_profile_setup(valid).unwrap();
    store
        .save_profile_setup(&ProfileSetupRecord {
            revision: 4,
            state: ProfileSetupState::Gathering,
            ..Default::default()
        })
        .unwrap();
    store
        .save_profile_setup(&ProfileSetupRecord {
            revision: 5,
            ..record
        })
        .unwrap();
    store
        .edit_config(|doc| doc.set_str("defaults.profile", "claude"))
        .unwrap();
    // When the same proposal identity is applied again.
    let replay = store
        .apply_profile_setup(|_, _| panic!("historical replay must not validate"))
        .unwrap();
    // Then it returns the durable historical receipt without restoring old config.
    assert_eq!(replay, applied);
    assert_eq!(
        Store::open(store.root())
            .unwrap()
            .config
            .defaults
            .profile
            .as_deref(),
        Some("claude")
    );
    assert_eq!(store.profile_setup_history().unwrap().len(), 5);
}

#[test]
fn invalid_project_root_is_rejected_before_intent_publication() {
    // Given a valid approved project whose directory subsequently disappears.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    fs::create_dir(dir.path().join("project")).unwrap();
    let config = store
        .edit_config(|doc| doc.set_str("projects.app.path", "project"))
        .unwrap();
    let record = proposed(&store, proposal(&config));
    fs::remove_dir(dir.path().join("project")).unwrap();
    // When attempting apply against the workspace's actual root.
    let result = store.apply_profile_setup(valid);
    // Then invalid project paths do not create an unfinishable intent.
    assert!(result.is_err());
    assert_eq!(store.profile_setup().unwrap(), record);
    assert!(
        store
            .record_row("profile-setup-intent", "current")
            .unwrap()
            .is_none()
    );
}

#[test]
fn raw_relevant_edit_rejection_has_no_observation_side_effects() {
    // Given an approved proposal followed by an unobserved relevant file edit.
    let (_dir, store, record) = fixture();
    let path = store.dir().join("corral.toml");
    let mut doc = ConfigDoc::load(&path).unwrap();
    doc.set_str("pm.agent", "claude").unwrap();
    fs::write(path, doc.to_string()).unwrap();
    fn snapshot(path: &std::path::Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
        let mut files = std::collections::BTreeMap::new();
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(snapshot(&path));
            } else if path.file_name().is_some_and(|name| name == "corral.db-shm") {
                // Opening a read view can move shared-memory read marks without committing.
                // The database, journal bytes and committed sequence remain checked.
            } else {
                files.insert(path.clone(), fs::read(path).unwrap());
            }
        }
        files
    }
    let before = snapshot(store.dir());
    let sequence = store.view().unwrap().seq();
    // When rejecting the stale proposal before config observation.
    let result = store.apply_profile_setup(valid);
    // Then rejection leaves every persisted byte and the setup receipt unchanged.
    assert!(result.is_err());
    assert_eq!(snapshot(store.dir()), before);
    assert_eq!(store.view().unwrap().seq(), sequence);
    assert_eq!(store.profile_setup().unwrap(), record);
}

#[test]
fn resume_revalidates_current_support_and_never_trusts_old_facets() {
    // Given an intent-only interruption with previously valid support.
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
    // When current support validation now rejects the proposal.
    let result = store.apply_profile_setup(|_, _| anyhow::bail!("support withdrawn"));
    // Then old validation facets cannot authorize a resumed write.
    assert!(result.is_err());
    assert_eq!(store.profile_setup().unwrap(), record);
    assert!(
        !Store::open(store.root())
            .unwrap()
            .config
            .profiles
            .contains_key("custom")
    );
}
