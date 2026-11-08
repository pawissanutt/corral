//! C2 preparation foundation. No task, dispatch, ownership transfer or native session fork
//! is created here. C3 consumes the stable journal marker in its held-task/outbox transaction.
//! Prepare slow Git/blob work outside assistant locks; reserve inside the authenticated
//! assistant transaction after drain. Until C3 exists public assistant_handoff stays closed.
use super::{Expected, compare, current, publication::load_artifact};
use anyhow::{Context as _, Result, ensure};
use corral_core::{
    Store,
    assistant::AssistantContainer,
    assistant_workflow::{DesignArtifact, PlanRevision, ScopeOwner, WorkflowPhase},
};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub(crate) mod admission;
pub(crate) mod adopt;
mod attachments;
mod code_pin;
mod content;
pub mod context;
pub mod continuation;
pub use code_pin::CodePin;
pub mod lifecycle;
pub(crate) mod plan_goal;
pub(crate) mod recovery;
mod source;
pub(crate) mod takeback;
pub mod transfer;
pub use takeback::Returned;
mod git;
pub mod owner;
mod preview;
pub(crate) mod renew;
mod restore;
mod retention;
pub use content::Content;
use context::{Context, Preview};
pub use git::{Snapshot, WorkState};
pub use preview::{PackagePreview, preview_package};
pub use retention::require_source_releasable;

const PACKAGES: &str = "assistant-handoff-packages";
const JOURNALS: &str = "assistant-handoff-journals";
/// Immutable source/intent-to-preview lookup, not ownership or another journal.
const INTENTS: &str = "assistant-handoff-intents";
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Goal {
    PlanContinue,
    PlanReviewOnly,
    ImplementationPrepare,
    ImplementationVerify,
    ImplementationShip,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub version: u32,
    pub intent: corral_core::blob_ref::BlobHash,
    pub assistant: String,
    pub expected: Expected,
    pub project: String,
    pub scope: Vec<String>,
    /// Descriptive goal, NOT execution or provider consent.
    pub goal: Goal,
    pub context: Content,
    pub original_request: DesignArtifact,
    pub plan: Option<PlanRevision>,
    pub decisions: Vec<DesignArtifact>,
    pub reviews: Vec<DesignArtifact>,
    pub execution: Option<DesignArtifact>,
    pub snapshot: Option<Snapshot>,
    pub source_path: Option<std::path::PathBuf>,
    /// Recorded base reference (normally the integration branch), distinct from its captured SHA.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_base: Option<String>,
}

/// Immutable preview descriptor, NOT a transfer journal or ownership state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Publication {
    pub version: u32,
    pub id: String,
    pub package: Content,
    pub assistant: String,
    pub expected: Expected,
    pub previous_phase: WorkflowPhase,
}

/// The single authoritative transfer journal; absent until reservation commits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    pub version: u32,
    /// Stable transfer/source marker; survives receipt-window expiry and host exit.
    pub id: String,
    pub package: Content,
    pub assistant: String,
    pub expected: Expected,
    pub previous_phase: WorkflowPhase,
    pub reserved: bool,
    /// A retained source is never cleanup authorization.
    pub source_retained: bool,
    /// C3 extends this journal in place. Old reserved C2 packages remain fenced and
    /// reconciling until actual task/ownership publication completes.
    #[serde(default)]
    pub transfer: Option<lifecycle::Transfer>,
    #[serde(default)]
    pub selection: Option<transfer::Selection>,
    #[serde(default)]
    pub previous_handoff: Option<DesignArtifact>,
    #[serde(default)]
    pub previous_handoff_known: bool,
    #[serde(default)]
    pub cancelled: bool,
    #[serde(default)]
    pub waiting: Option<String>,
}

/// Only successful preparation/restore verification can construct this value. C3 must
/// revalidate the source AFTER reservation closes admission and before ownership commits.
pub struct Prepared {
    publication: Publication,
    preview: Preview,
    package: Package,
    code_clean: Option<bool>,
}

impl Prepared {
    pub fn id(&self) -> &str {
        &self.publication.id
    }
    pub fn preview(&self) -> &Preview {
        &self.preview
    }
    pub fn package(&self) -> &Package {
        &self.package
    }
    pub fn code_clean(&self) -> Option<bool> {
        self.code_clean
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub project: String,
    pub scope: Vec<String>,
    pub goal: Goal,
    pub context: Context,
}

fn marker(session: &str, expected: &Expected) -> String {
    format!(
        "{session}/{}/s{}-e{}",
        expected.workflow, expected.state_revision, expected.authority_epoch
    )
}

/// `source` is a service-resolved registered scratch path, never an agent-supplied path.
/// A plan/research-only package may have no code; an existing scratch may NOT be omitted.
/// The lifecycle owner is responsible for resolving that presence, not guessing from phase.
pub fn prepare(
    store: &Store,
    session: &str,
    container: &AssistantContainer,
    expected: &Expected,
    request: Request,
    source: Option<(&Path, &str, &str)>,
) -> Result<Prepared> {
    compare(container, expected)?;
    let w = current(container)?;
    ensure!(
        w.owner == ScopeOwner::Local
            && matches!(
                w.phase,
                WorkflowPhase::Ordinary
                    | WorkflowPhase::Planning
                    | WorkflowPhase::Executing
                    | WorkflowPhase::Candidate
                    | WorkflowPhase::Paused
            ),
        "handoff_unavailable: source is not settled local work"
    );
    ensure!(
        super::boundary::activities(store, session)?.is_empty(),
        "handoff_busy: active/uncertain managed effects must settle"
    );
    ensure!(
        store.config.projects.contains_key(&request.project),
        "handoff_project: unregistered project"
    );
    ensure!(
        !request.scope.is_empty() && request.scope.len() <= 64,
        "handoff_scope: requires 1-64 paths"
    );
    for path in &request.scope {
        content::safe_path(path)?;
    }
    request.context.preview()?;
    for artifact in std::iter::once(&w.original_request)
        .chain(w.current_plan.as_ref().map(|p| &p.artifact))
        .chain(
            w.current_plan
                .iter()
                .flat_map(|p| p.assumptions.iter().chain(p.execution_scope.iter())),
        )
        .chain(&w.decision_refs)
        .chain(&w.reviews)
        .chain(w.execution.iter())
    {
        load_artifact(store, artifact).context("handoff_missing: required workflow content")?;
    }
    let assistant = store.get_assistant(session)?;
    let scratches: Vec<_> = assistant
        .scratch
        .iter()
        .filter(|s| s.project == request.project)
        .collect();
    match source {
        Some((path, base, branch)) => ensure!(
            scratches
                .iter()
                .any(|s| s.path == path && s.base == base && s.branch == branch),
            "handoff_source: source must name a recorded scratch in the selected project"
        ),
        None => ensure!(
            scratches.is_empty(),
            "handoff_missing: existing project scratch cannot be omitted"
        ),
    }
    let intent =
        corral_core::blob_ref::BlobHash::of(&serde_json::to_vec(&(expected, &request, source))?);
    let intent_key = format!("{}/{}", marker(session, expected), intent);
    if let Some(id) = store.record::<String>(INTENTS, &intent_key)? {
        let prepared = recover(store, &id)?;
        ensure!(
            prepared.package.intent == intent,
            "handoff_reconciling: intent lookup differs from published package"
        );
        return Ok(prepared);
    }
    let snapshot = source
        .map(|(path, base, branch)| -> Result<Snapshot> {
            let base = String::from_utf8(git::git(
                path,
                &[
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    &format!("{base}^{{commit}}"),
                ],
                None,
            )?)?;
            Snapshot::capture(store, path, base.trim(), branch)
        })
        .transpose()?;
    let code_clean = if let Some(snapshot) = &snapshot {
        let restore = tempfile::tempdir().context("prepare restoration verification")?;
        let verified = restore.path().join("verified");
        snapshot.restore_new(store, &verified)?;
        let clean = git::git(
            &verified,
            &["status", "--porcelain", "--untracked-files=all"],
            None,
        )?
        .is_empty();
        snapshot.validate_source(source.context("source disappeared")?.0)?;
        Some(clean)
    } else {
        None
    };
    let package = Package {
        version: VERSION,
        intent,
        assistant: session.into(),
        expected: expected.clone(),
        project: request.project,
        scope: request.scope,
        goal: request.goal,
        context: Content::put(store, &serde_json::to_vec(&request.context)?)?,
        original_request: w.original_request.clone(),
        plan: w.current_plan.clone(),
        decisions: w.decision_refs.clone(),
        reviews: w.reviews.clone(),
        execution: w.execution.clone(),
        snapshot,
        source_path: source.map(|s| s.0.to_path_buf()),
        source_base: source.map(|s| s.1.to_string()),
    };
    let preview = package.context_preview(store)?;
    let bytes = serde_json::to_vec(&package)?;
    ensure!(
        bytes.len() <= corral_core::assistant_workflow::DOCUMENT_MAX,
        "handoff_oversized: package manifest exceeds 1 MiB"
    );
    let reference = Content::put(store, &bytes)?;
    crate::agent::failpoint("handoff_package_prepared");
    let publication = Publication {
        version: VERSION,
        id: format!("{}/{}", marker(session, expected), reference.blob.hash()),
        package: reference,
        assistant: session.into(),
        expected: expected.clone(),
        previous_phase: w.phase,
    };
    // A prepared preview is immutable and inspectable, but does not fence or transfer work.
    // CAS insert prevents a lost acknowledgement from selecting different bytes at this identity.
    store.records_transaction(|| {
        if !store.insert_record(PACKAGES, &publication.id, &publication)? {
            let old: Publication = store.record(PACKAGES, &publication.id)?.context("handoff package missing")?;
            ensure!(old == publication, "handoff_changed: immutable preview identity differs");
        }
        if !store.insert_record(INTENTS, &intent_key, &publication.id)? {
            ensure!(store.record::<String>(INTENTS, &intent_key)?.as_deref() == Some(publication.id.as_str()),
                "handoff_changed: this source/intent already published another checkpoint; reconcile readback");
        }
        Ok(())
    })?;
    crate::agent::failpoint("handoff_preview_published");
    Ok(Prepared {
        publication,
        preview,
        package,
        code_clean,
    })
}

/// Called ONLY inside the existing authenticated assistant-container transaction. The journal,
/// workflow fence and caller's receipt publish in one record unit. No Git/provider work here.
pub fn reserve(
    store: &Store,
    session: &str,
    container: &mut AssistantContainer,
    prepared: &Prepared,
) -> Result<Journal> {
    let j = &prepared.publication;
    ensure!(
        j.assistant == session,
        "handoff_identity: wrong source session"
    );
    if let Some(old) = read_journal(store, &j.id)? {
        ensure!(
            old.package == j.package
                && current(container)?
                    .handoff
                    .as_ref()
                    .is_some_and(|a| a.blob == j.package.blob),
            "handoff_reconciling: another reservation exists"
        );
        return Ok(old);
    }
    compare(container, &j.expected)?;
    ensure!(
        super::boundary::activities(store, session)?.is_empty(),
        "handoff_busy: managed effects remain active or uncertain"
    );
    ensure!(
        !container
            .state
            .receipts
            .iter()
            .any(|r| r.op == "owner_action"
                && matches!(
                    r.state,
                    corral_core::agent::ReceiptState::Reserved
                        | corral_core::agent::ReceiptState::Uncertain
                )),
        "handoff_busy: uncertain effect receipt requires reconciliation"
    );
    let index = container.workflow.as_mut().context("workflow missing")?;
    ensure!(
        index.current.owner == ScopeOwner::Local && index.current.phase == j.previous_phase,
        "handoff_changed: source ownership/phase changed"
    );
    index.authority_epoch = index
        .authority_epoch
        .checked_add(1)
        .context("authority epoch overflow")?;
    let w = &mut index.current;
    w.authority_epoch = index.authority_epoch;
    w.state_revision = w
        .state_revision
        .checked_add(1)
        .context("workflow revision overflow")?;
    w.phase = WorkflowPhase::HandoffPreparing;
    let previous_handoff = w.handoff.clone();
    w.handoff = Some(DesignArtifact {
        blob: j.package.blob.clone(),
        bytes: j.package.bytes as u32,
    });
    let reserved = Journal {
        version: VERSION,
        id: j.id.clone(),
        package: j.package.clone(),
        assistant: j.assistant.clone(),
        expected: j.expected.clone(),
        previous_phase: j.previous_phase,
        reserved: true,
        source_retained: true,
        transfer: None,
        selection: None,
        previous_handoff,
        previous_handoff_known: true,
        cancelled: false,
        waiting: None,
    };
    ensure!(
        store.insert_record(JOURNALS, &j.id, &reserved)?,
        "handoff_reconciling: journal already exists"
    );
    crate::agent::failpoint("handoff_reservation_staged");
    Ok(reserved)
}

pub fn read_journal(store: &Store, id: &str) -> Result<Option<Journal>> {
    let row: Option<Journal> = store
        .record(JOURNALS, id)
        .context("handoff_incompatible: upgrade/reconcile transfer journal")?;
    if let Some(j) = &row {
        ensure!(
            ((j.version == VERSION && j.transfer.is_none() && j.selection.is_none())
                || (j.version == 2 && j.selection.is_some()))
                && j.id == id
                && j.reserved
                && (j.source_retained || j.cancelled),
            "handoff_incompatible: invalid journal; upgrade/reconcile required"
        );
    }
    Ok(row)
}

/// Preview and current reservation readback reconstruct from pinned blobs, not transcripts.
pub fn read_package(store: &Store, id: &str) -> Result<(Publication, Package, Preview)> {
    let j: Publication = store
        .record(PACKAGES, id)?
        .context("handoff_missing: incomplete/unknown package publication")?;
    if let Some(journal) = read_journal(store, id)? {
        ensure!(
            journal.package == j.package
                && journal.expected == j.expected
                && journal.assistant == j.assistant,
            "handoff_reconciling: journal/package publication differs"
        );
    }
    ensure!(
        j.version == VERSION && j.id == id,
        "handoff_incompatible: unsupported/mismatched package publication"
    );
    let package: Package = serde_json::from_slice(&j.package.load(store)?)
        .context("decode immutable handoff package")?;
    ensure!(
        package.version == VERSION
            && package.assistant == j.assistant
            && package.expected == j.expected
            && j.id
                == format!(
                    "{}/{}",
                    marker(&j.assistant, &j.expected),
                    j.package.blob.hash()
                ),
        "handoff_incompatible: package/source identity mismatch"
    );
    for artifact in std::iter::once(&package.original_request)
        .chain(package.plan.as_ref().map(|p| &p.artifact))
        .chain(
            package
                .plan
                .iter()
                .flat_map(|p| p.assumptions.iter().chain(p.execution_scope.iter())),
        )
        .chain(&package.decisions)
        .chain(&package.reviews)
        .chain(package.execution.iter())
    {
        load_artifact(store, artifact).context("handoff_missing: required workflow content")?;
    }
    let preview = package.context_preview(store)?;
    Ok((j, package, preview))
}

/// Recover exact preparation after host death/lost acknowledgement. Never recapture different
/// bytes, fabricate a new task identity, or fall back to an empty checkout on missing data.
pub fn recover(store: &Store, id: &str) -> Result<Prepared> {
    checkpoint(store, id, true)
}

/// After ownership commits, restoration uses the self-contained checkpoint, not the
/// originating host/checkout. Source revalidation remains mandatory BEFORE ownership.
pub(super) fn checkpoint(store: &Store, id: &str, validate_source: bool) -> Result<Prepared> {
    let (publication, package, preview) = read_package(store, id)?;
    let code_clean = match (&package.snapshot, &package.source_path) {
        (Some(snapshot), Some(source)) => {
            let temp = tempfile::tempdir().context("create recovery verification")?;
            let verified = temp.path().join("verified");
            snapshot.restore_new(store, &verified)?;
            if validate_source {
                snapshot.validate_source(source)?;
            }
            Some(
                git::git(
                    &verified,
                    &["status", "--porcelain", "--untracked-files=all"],
                    None,
                )?
                .is_empty(),
            )
        }
        (None, None) => None,
        _ => anyhow::bail!("handoff_missing: code snapshot/source identity mismatch"),
    };
    Ok(Prepared {
        publication,
        package,
        preview,
        code_clean,
    })
}

#[cfg(test)]
mod code_journey_tests;
#[cfg(test)]
mod crash_tests;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod tests;
