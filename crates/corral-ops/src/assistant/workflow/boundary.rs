//! Managed-interface admission, not an OS sandbox. Lock order is the existing assistant
//! transaction, then its record unit. Starts and entry serialize on that same transaction;
//! an interrupted start remains outstanding until its actual executor proves settlement.
use anyhow::{Context, Result};

// Policy refusals are protocol rejections, not unclassified transport failures.
macro_rules! ensure {
    ($condition:expr, $reason:literal) => {
        if !$condition {
            return Err(super::bad($reason));
        }
    };
}
use corral_core::Store;
use corral_core::agent::ReceiptState;
use corral_core::assistant::AssistantContainer;
use corral_core::assistant_workflow::{ScopeOwner, WorkflowPhase};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Expected, bad, compare, prepare_entry, summary};

const LEDGER: &str = "assistant-effect-admission";
const MAX_ACTIVE: usize = 128;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Activity {
    pub id: String,
    pub generation: u32,
    pub tool: String,
    pub uncertain: bool,
}

/// Deliberately closed: this is host lifecycle bookkeeping, never a model tool or generic
/// permission endpoint. Execution consent and candidate choices are separate typed services.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum BoundaryAction {
    Enter {
        input: String,
        expected: Option<Expected>,
    },
    Drained {
        expected: Expected,
    },
    Start {
        id: String,
        tool: String,
        authority_epoch: u64,
    },
    Settle {
        id: String,
        uncertain: bool,
    },
}

pub fn ordinary(container: &AssistantContainer) -> Result<bool> {
    container.validate_protocol()?;
    Ok(container.workflow.as_ref().is_none_or(|index| {
        index.current.owner == ScopeOwner::Local
            && matches!(
                index.current.phase,
                WorkflowPhase::Ordinary | WorkflowPhase::Closed
            )
    }))
}

/// Generic owner/board effects and filesystem grants cannot consume design agreement,
/// reviews, or a scratch execution grant. Until a scoped consumer is installed, executing
/// also refuses here rather than silently treating an artifact reference as consent.
pub fn require_ordinary(
    store: &Store,
    session: &str,
    container: &AssistantContainer,
) -> Result<()> {
    if container.workflow.is_some() {
        // A closed label must not turn missing/corrupt authoritative bytes into ordinary work.
        super::read_state(store, session, container, 0, 1)?;
    }
    if !ordinary(container)? {
        return Err(crate::agent::Rejection::new(
            crate::agent::RejectCode::OpNotAllowed,
            "assistant_planning: this effect requires ordinary local authority; a plan, review, permission or old owner action cannot unlock it",
        ).with_detail(summary(container)?).into());
    }
    Ok(())
}

/// A new source host cannot reinterpret a transfer reservation or Board ownership
/// as local edit authority. Planning/local recovery may resume under the same policy.
pub fn require_resume(store: &Store, session: &str, container: &AssistantContainer) -> Result<()> {
    container.validate_protocol()?;
    if let Some(index) = &container.workflow {
        super::read_state(store, session, container, 0, 1)?;
        ensure!(
            index.current.owner == ScopeOwner::Local
                && !matches!(
                    index.current.phase,
                    WorkflowPhase::HandoffPreparing
                        | WorkflowPhase::HandoffReconciling
                        | WorkflowPhase::BoardOwned
                ),
            "assistant_scope_owned: return or reconcile this scope before resuming its source host"
        );
    }
    Ok(())
}

pub fn activities(store: &Store, session: &str) -> Result<Vec<Activity>> {
    Ok(store.record(LEDGER, session)?.unwrap_or_default())
}

pub fn projection(store: &Store, session: &str) -> Result<Value> {
    let active = activities(store, session)?;
    let mut ids: Vec<_> = active.iter().map(|a| a.id.as_str()).collect();
    ids.sort_unstable();
    Ok(
        json!({"activities": active.iter().take(4).collect::<Vec<_>>(),
        "active_count": active.len(), "uncertain": active.iter().any(|a| a.uncertain),
        "identities_digest": corral_core::blob_ref::BlobHash::of(ids.join("\0").as_bytes())}),
    )
}

fn epoch(container: &AssistantContainer) -> u64 {
    container
        .workflow
        .as_ref()
        .map_or(0, |index| index.authority_epoch)
}

fn advance(container: &mut AssistantContainer, phase: WorkflowPhase) -> Result<()> {
    let index = container.workflow.as_mut().context("workflow missing")?;
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
    w.phase = phase;
    Ok(())
}

/// Called only in a generation-rechecked host event's receipted assistant transaction.
/// No PID signalling, executor replay, or inference that a fenced generation stopped work.
pub fn apply(
    store: &Store,
    session: &str,
    container: &mut AssistantContainer,
    generation: u32,
    action: BoundaryAction,
) -> Result<Value> {
    container.validate_protocol()?;
    match action {
        BoundaryAction::Enter { input, expected } => {
            if container.workflow.is_none() {
                ensure!(expected.is_none(), "new entry has no previous workflow");
                prepare_entry(store, session, container, &input)?;
            } else {
                compare(
                    container,
                    &expected.ok_or_else(|| bad("entry requires current workflow CAS"))?,
                )?;
                let w = super::current(container)?;
                ensure!(
                    w.owner == ScopeOwner::Local,
                    "board ownership must be returned explicitly"
                );
                ensure!(
                    container
                        .owner_input(&input)
                        .is_some_and(|e| e.generation == generation
                            && !e.truncated
                            && matches!(
                                e.source,
                                corral_core::assistant::OwnerInputSource::Pane
                                    | corral_core::assistant::OwnerInputSource::Dialog
                            )),
                    "entry requires genuine current-generation owner input"
                );
                advance(container, WorkflowPhase::Entering)?;
            }
        }
        BoundaryAction::Drained { expected } => {
            compare(container, &expected)?;
            ensure!(
                matches!(
                    super::current(container)?.phase,
                    WorkflowPhase::Entering | WorkflowPhase::Recovering
                ),
                "not entering planning"
            );
            ensure!(
                activities(store, session)?.is_empty(),
                "planning entry blocked: managed effects remain active or uncertain"
            );
            ensure!(
                !container
                    .state
                    .receipts
                    .iter()
                    .any(|r| r.op == "owner_action"
                        && matches!(r.state, ReceiptState::Reserved | ReceiptState::Uncertain)),
                "planning entry blocked: owner action requires reconciliation"
            );
            advance(container, WorkflowPhase::Planning)?;
        }
        BoundaryAction::Start {
            id,
            tool,
            authority_epoch,
        } => {
            require_ordinary(store, session, container)?;
            ensure!(
                authority_epoch == epoch(container),
                "stale activity authority epoch"
            );
            ensure!(
                !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control),
                "invalid activity identity"
            );
            ensure!(
                !tool.is_empty()
                    && tool.len() <= 128
                    && tool
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "_:-".contains(c)),
                "invalid activity tool"
            );
            let mut active = activities(store, session)?;
            ensure!(
                active
                    .iter()
                    .all(|a| a.generation == generation && !a.uncertain),
                "activity recovery required: prior or uncertain executor remains"
            );
            ensure!(
                active.len() < MAX_ACTIVE,
                "activity ledger full; settle existing effects"
            );
            ensure!(
                !active.iter().any(|a| a.id == id),
                "activity already admitted; never replay an effect"
            );
            active.push(Activity {
                id,
                generation,
                tool,
                uncertain: false,
            });
            store.save_record(LEDGER, session, &active)?;
        }
        BoundaryAction::Settle { id, uncertain } => {
            let mut active = activities(store, session)?;
            let row = active
                .iter()
                .position(|a| a.id == id && a.generation == generation)
                .ok_or_else(|| bad("activity not owned by this generation"))?;
            if uncertain {
                active[row].uncertain = true;
            } else {
                active.remove(row);
            }
            store.save_record(LEDGER, session, &active)?;
        }
    }
    let mut result = summary(container)?;
    result["boundary"] = projection(store, session)?;
    Ok(result)
}
