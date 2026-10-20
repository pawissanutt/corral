//! Shared setup operations. No operation here launches agents or changes operating authority.
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};

use anyhow::{Context, Result, ensure};
use corral_core::{
    PROFILE_RECORD_VERSION, Profile, ProfileRole, ProfileSetupApproval, ProfileSetupDiff,
    ProfileSetupProposal, ProfileSetupRecord, ProfileSetupState, SetupValidationFacets, Store,
};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::setup_discovery::SetupDiscovery;

/// Skeleton `SetupProposalInput`: replace every `<placeholder>` string; repeated
/// placeholders must receive the same value. docs/profile-setup.md embeds these bytes.
pub const SETUP_TEMPLATE: &str = include_str!("../assets/setup-template.json");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupProposalInput {
    pub proposal: ProfileSetupProposal,
    pub discovery: SetupDiscovery,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetupStatus {
    pub procedure_version: &'static str,
    pub config_revision: String,
    pub catalog_revision: String,
    pub record: ProfileSetupRecord,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetupCatalog {
    pub revision: String,
    pub config_revision: String,
    pub starter_keys: [&'static str; 9],
    pub profiles: BTreeMap<String, Profile>,
    pub roles: BTreeMap<ProfileRole, String>,
    pub legacy_profiles: Vec<String>,
    pub pm_profile: String,
    pub default_profile: Option<String>,
    pub reviewer: Option<String>,
    pub repair_policy: Option<corral_core::RepairPolicy>,
    pub owner_rules: Vec<corral_core::RoutingRule>,
    /// "workspace" and "project:<key>" keep a project named "workspace" unambiguous.
    pub review_policies: BTreeMap<String, corral_core::ReviewPolicy>,
}

pub fn setup_status(store: &Store) -> Result<SetupStatus> {
    let config = store.config_layers()?.config()?;
    Ok(SetupStatus {
        procedure_version: crate::skill::MODEL_SELECTION_VERSION,
        config_revision: config.profile_setup_revision()?,
        catalog_revision: config.profile_catalog_revision()?,
        record: store.profile_setup()?,
    })
}

pub fn setup_catalog(store: &Store) -> Result<SetupCatalog> {
    let config = store.config_layers()?.config()?;
    Ok(SetupCatalog {
        revision: config.profile_catalog_revision()?,
        config_revision: config.profile_setup_revision()?,
        starter_keys: corral_core::STARTER_PROFILE_KEYS,
        profiles: config.profiles.clone(),
        roles: config.routing.role_profiles.clone(),
        legacy_profiles: config.agents.keys().cloned().collect(),
        pm_profile: config.pm.agent.clone(),
        default_profile: config.defaults.profile.clone(),
        reviewer: config.review.reviewer.clone(),
        repair_policy: config.routing.repair.clone(),
        owner_rules: config.routing.rules.clone(),
        review_policies: std::iter::once(("workspace".into(), config.review_policy(None, None)))
            .chain(config.projects.keys().map(|key| {
                (
                    format!("project:{key}"),
                    config.review_policy(Some(key), None),
                )
            }))
            .collect(),
    })
}

pub fn setup_validate(store: &Store, input: &SetupProposalInput) -> Result<SetupValidationFacets> {
    let config = store.config_layers()?.config()?;
    ensure!(
        input.proposal.based_on_config_revision == config.profile_setup_revision()?,
        "stale setup proposal"
    );
    config
        .with_profile_setup(&input.proposal)?
        .validate(store.root())?;
    crate::setup_validation::validate(input, &config)
}

pub fn setup_begin(store: &Store, expected_revision: u64) -> Result<ProfileSetupRecord> {
    let previous = expected(store, expected_revision)?;
    match previous.state {
        ProfileSetupState::Gathering | ProfileSetupState::Proposed => return Ok(previous),
        ProfileSetupState::NotStarted
        | ProfileSetupState::Applied
        | ProfileSetupState::Cancelled => {}
    }
    let record = ProfileSetupRecord {
        revision: next_revision(&previous)?,
        state: ProfileSetupState::Gathering,
        answers: previous.answers,
        ..ProfileSetupRecord::default()
    };
    store.save_profile_setup(&record)?;
    Ok(record)
}

/// Merge interview answers without discarding earlier answers or approving configuration.
pub fn setup_answers(
    store: &Store,
    answers: &BTreeMap<String, String>,
    expected_revision: u64,
) -> Result<ProfileSetupRecord> {
    let mut record = expected(store, expected_revision)?;
    ensure!(
        record.state == ProfileSetupState::Gathering,
        "resume gathering before changing answers; cancel a proposal before revising its interview"
    );
    ensure!(
        answers.keys().all(|key| !key.trim().is_empty()),
        "setup answer key is empty"
    );
    record.revision = next_revision(&record)?;
    record.answers.extend(answers.clone());
    store.save_profile_setup(&record)?;
    Ok(record)
}

/// How far past the wall clock a proposal's `prepared_at` may be, to absorb clock skew.
const PREPARED_AT_SKEW: SignedDuration = SignedDuration::from_secs(60);

pub fn setup_propose(
    store: &Store,
    input: &SetupProposalInput,
    expected_revision: u64,
) -> Result<ProfileSetupRecord> {
    setup_propose_at(store, input, expected_revision, Timestamp::now())
}

/// [`setup_propose`] against an injected wall clock.
pub fn setup_propose_at(
    store: &Store,
    input: &SetupProposalInput,
    expected_revision: u64,
    now: Timestamp,
) -> Result<ProfileSetupRecord> {
    let previous = expected(store, expected_revision)?;
    let prepared_at = input.proposal.prepared_at;
    // Approval cannot predate the proposal, so a future prepared_at could never be approved.
    ensure!(
        prepared_at <= now.checked_add(PREPARED_AT_SKEW)?,
        "setup proposal prepared_at {prepared_at} is later than the current time {now} \
         (allowed clock skew {}s); correct prepared_at",
        PREPARED_AT_SKEW.as_secs()
    );
    let validation = setup_validate(store, input)?;
    // Immutable, digest-addressed support facts precede the journal that references them.
    save_input(store, input)?;
    let record = ProfileSetupRecord {
        version: PROFILE_RECORD_VERSION,
        revision: next_revision(&previous)?,
        state: ProfileSetupState::Proposed,
        answers: previous.answers,
        proposal: Some(input.proposal.clone()),
        approval: None,
        applied_config_revision: None,
        validation,
    };
    store.save_profile_setup(&record)?;
    Ok(record)
}

pub fn setup_approve(
    store: &Store,
    approval: &ProfileSetupApproval,
    expected_revision: u64,
) -> Result<ProfileSetupRecord> {
    let mut record = expected(store, expected_revision)?;
    ensure!(
        record.state == ProfileSetupState::Proposed,
        "setup is not proposed"
    );
    let proposal = record.proposal.as_ref().context("missing setup proposal")?;
    proposal.check_approval(approval)?;
    let input = load_input(store, proposal)?;
    record.validation = setup_validate(store, &input)?;
    record.revision = next_revision(&record)?;
    record.approval = Some(approval.clone());
    store.save_profile_setup(&record)?;
    Ok(record)
}

pub fn setup_cancel(store: &Store, expected_revision: u64) -> Result<ProfileSetupRecord> {
    let mut record = expected(store, expected_revision)?;
    record.revision = next_revision(&record)?;
    record.state = ProfileSetupState::Cancelled;
    record.approval = None;
    store.save_profile_setup(&record)?;
    Ok(record)
}

pub fn setup_diff(store: &Store) -> Result<ProfileSetupDiff> {
    store.profile_setup_diff()
}

pub fn setup_apply(store: &Store) -> Result<ProfileSetupRecord> {
    store.apply_profile_setup(|proposal, config| {
        let input = load_input(store, proposal)?;
        crate::setup_validation::validate(&input, config)
    })
}

fn expected(store: &Store, revision: u64) -> Result<ProfileSetupRecord> {
    let previous = store.profile_setup()?;
    ensure!(previous.revision == revision, "stale setup revision");
    Ok(previous)
}

fn next_revision(record: &ProfileSetupRecord) -> Result<u64> {
    record
        .revision
        .checked_add(1)
        .context("setup revision exhausted")
}

fn input_path(store: &Store, proposal: &ProfileSetupProposal) -> Result<std::path::PathBuf> {
    Ok(store
        .state_dir()
        .join("profile-proposals")
        .join(format!("{}.json", proposal.digest()?)))
}

fn save_input(store: &Store, input: &SetupProposalInput) -> Result<()> {
    let path = input_path(store, &input.proposal)?;
    fs::create_dir_all(path.parent().context("proposal path has no parent")?)
        .context("creating setup proposal directory")?;
    let mut file =
        tempfile::NamedTempFile::new_in(path.parent().context("missing proposal parent")?)
            .context("creating setup proposal temporary file")?;
    serde_json::to_writer(&mut file, input).context("encoding setup proposal input")?;
    file.flush().context("flushing setup proposal input")?;
    file.as_file()
        .sync_all()
        .context("syncing setup proposal input")?;
    match file.persist_noclobber(&path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            ensure!(
                load_input(store, &input.proposal)? == *input,
                "proposal support facts are immutable; prepare a new proposal"
            );
            Ok(())
        }
        Err(error) => Err(error.error).context("persisting setup proposal support facts"),
    }
}

fn load_input(store: &Store, proposal: &ProfileSetupProposal) -> Result<SetupProposalInput> {
    let path = input_path(store, proposal)?;
    ensure!(
        fs::symlink_metadata(&path)
            .context("inspecting setup proposal support facts")?
            .is_file(),
        "proposal input must be a regular file, not a symlink or special file"
    );
    let mut file = fs::File::open(&path).context("opening setup proposal support facts")?;
    ensure!(
        file.metadata()?.is_file(),
        "proposal input is not a regular file"
    );
    let max = store.config.artifacts.max_bytes;
    let mut text = String::new();
    Read::by_ref(&mut file)
        .take(max.saturating_add(1))
        .read_to_string(&mut text)
        .context("reading setup proposal support facts")?;
    ensure!(
        u64::try_from(text.len())? <= max,
        "proposal support facts exceed size limit"
    );
    let input: SetupProposalInput =
        serde_json::from_str(&text).context("parsing setup proposal support facts")?;
    ensure!(
        &input.proposal == proposal,
        "persisted proposal digest mismatch"
    );
    Ok(input)
}
