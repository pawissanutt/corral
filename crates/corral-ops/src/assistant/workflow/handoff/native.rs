//! Native authenticated handoff consumer. A host-rendered selection becomes transfer
//! consent only as a journaled `Choice` input whose binding digest matches the exact
//! action payload, workflow epoch, state revision and plan. Each input is consumed once
//! (durably, surviving journal eviction) before the SAME C3 services run. Model-supplied
//! input IDs, ordinary finish confirmations and Shell fallbacks are never this evidence.
use anyhow::{Context, Result, ensure};
use corral_core::assistant::{AssistantContainer, OwnerInputSource};
use corral_core::assistant_workflow::{DecisionBinding, DecisionEvidence};
use corral_core::blob_ref::BlobHash;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{continuation, lifecycle::Instruction, takeback, transfer};
use crate::Ctx;

/// Consumed native choices, keyed by owner-input ID. Never deleted by journal eviction.
const CONSUMED: &str = "assistant-handoff-native-choices";
pub const PURPOSE: &str = "assistant_handoff";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeAction {
    Commit {
        choice: transfer::Choice,
    },
    Start {
        choice: continuation::Choice,
    },
    Save {
        task: String,
        contract_revision: String,
    },
    TakeBack {
        task: String,
        contract_revision: String,
    },
    /// Prepare, preview and commit assistant-led local work in one bound selection.
    Local {
        local: super::native_local::LocalChoice,
    },
    /// End planning and open scratch work for the current ready plan revision's scope.
    Implement {
        implement: super::native_implement::ImplementChoice,
    },
}

impl NativeAction {
    /// The stable option the owner must have selected; labels may change, this may not.
    pub fn option(&self) -> Result<String> {
        Ok(match self {
            Self::Commit { choice } if choice.save => "save_for_later".into(),
            Self::Commit { .. } => "continue_selected_goal".into(),
            Self::Start { choice } => serde_json::to_value(choice.goal)?
                .as_str()
                .context("goal name")?
                .to_string(),
            Self::Save { .. } => "save_for_later".into(),
            Self::TakeBack { .. } => "work_on_this_here".into(),
            Self::Implement { .. } => "implement_here".into(),
            Self::Local { local } if local.save => "save_for_later".into(),
            Self::Local { local } => match local.goal {
                super::Goal::PlanContinue => "plan_continue".into(),
                super::Goal::PlanReviewOnly => "plan_review_only".into(),
                _ => "continue_selected_goal".into(),
            },
        })
    }

    fn question(&self) -> String {
        match self {
            Self::Commit { choice } => format!("handoff:{}", choice.package),
            Self::Start { choice } => format!("continue:{}", choice.task),
            Self::Save { task, .. } => format!("save:{task}"),
            Self::TakeBack { task, .. } => format!("take_back:{task}"),
            Self::Local { local } => format!("local:{}", local.project),
            Self::Implement { implement } => format!("implement:{}", implement.project),
        }
    }
}

/// Read-only: what the host must show and bind the owner's selection to. Grants nothing.
pub fn binding(
    container: &AssistantContainer,
    session: &str,
    action: &NativeAction,
    shown: &str,
) -> Result<(DecisionBinding, String)> {
    container.validate_protocol()?;
    // The action and the exact state the sheet showed are one payload: neither can be widened.
    let payload = BlobHash::of(&serde_json::to_vec(&(action, shown))?);
    let entry = matches!(action, NativeAction::Local { local } if local.entry.is_some());
    let binding = match container.workflow.as_ref().map(|i| &i.current) {
        Some(w) => {
            ensure!(
                !entry,
                "handoff_source: existing workflow must be used without an entry request"
            );
            DecisionBinding {
                assistant: session.into(),
                workflow: w.id.clone(),
                question: action.question(),
                question_revision: u32::try_from(w.state_revision)
                    .context("workflow revision exceeds a question revision")?,
                purpose: PURPOSE.into(),
                plan_revision: w.current_plan.as_ref().map(|p| p.revision),
                plan_digest: w
                    .current_plan
                    .as_ref()
                    .map(|p| p.artifact.blob.hash().clone()),
                authority_epoch: w.authority_epoch,
                action_payload_digest: payload.clone(),
            }
        }
        // Only a local entry may bind before a workflow exists; it enters one when consumed.
        None => {
            ensure!(entry, "no assistant workflow");
            DecisionBinding {
                assistant: session.into(),
                workflow: "none".into(),
                question: action.question(),
                question_revision: 0,
                purpose: PURPOSE.into(),
                plan_revision: None,
                plan_digest: None,
                authority_epoch: 0,
                action_payload_digest: payload.clone(),
            }
        }
    };
    let digest = BlobHash::of(&serde_json::to_vec(&binding)?)
        .as_str()
        .to_string();
    Ok((binding, digest))
}

/// Validate a host-journaled Choice against the CURRENT binding. Pure; consumes nothing.
pub(crate) fn authenticate(
    container: &AssistantContainer,
    session: &str,
    generation: u32,
    input: &str,
    action: &NativeAction,
    shown: &str,
) -> Result<DecisionEvidence> {
    let (binding, digest) = binding(container, session, action, shown)?;
    let entry = container
        .owner_input(input)
        .context("handoff_choice: not a journaled owner input of this session")?;
    ensure!(
        entry.source == OwnerInputSource::Choice,
        "handoff_choice: only a native keyboard/pointer selection is transfer consent"
    );
    ensure!(
        entry.generation == generation,
        "handoff_choice: selection belongs to another host generation"
    );
    ensure!(
        entry.binding.as_deref() == Some(digest.as_str()),
        "handoff_stale_choice: workflow, plan, epoch or action changed since the selection; refresh"
    );
    let option = action.option()?;
    ensure!(
        !entry.truncated
            && entry.text == option
            && entry.sha256 == BlobHash::of(entry.text.as_bytes()).as_str(),
        "handoff_choice: selected option does not match the requested action"
    );
    let evidence_digest = BlobHash::of(&serde_json::to_vec(&(&binding, &option, entry))?);
    Ok(DecisionEvidence {
        binding,
        selected_option: option,
        owner_input: entry.clone(),
        evidence_digest,
        consumed_at: corral_core::clock::now(),
    })
}

/// Durable exactly-once consumption, then the shared C3 service. A duplicate or late
/// answer refuses before any effect. A consumed choice whose service refused stays
/// consumed: the owner must choose again against the refreshed state.
pub(crate) fn consume(
    ctx: &Ctx,
    evidence: DecisionEvidence,
    action: NativeAction,
    shown: &str,
) -> Result<Value> {
    if let NativeAction::Local { local } = &action {
        super::native_local::precheck(local)?;
    }
    if let NativeAction::Implement { implement } = &action {
        super::native_implement::precheck(ctx, &evidence.binding.assistant, implement)?;
    }
    let key = evidence.owner_input.id.clone();
    let session = evidence.binding.assistant.clone();
    ensure!(
        consumed(ctx, &key)?.is_none(),
        "handoff_choice_consumed: this selection was already used; no effect repeated"
    );
    // Anything shown that changed refuses here, before the selection is consumed.
    let container = ctx.store.get_assistant_container(&session)?;
    ensure!(
        super::native_view::view(ctx, &session, &container)?.digest == shown,
        super::native_view::STALE
    );
    ensure!(
        ctx.store.insert_record(CONSUMED, &key, &evidence)?,
        "handoff_choice_consumed: this selection was already used; no effect repeated"
    );
    let evidence_digest = evidence.evidence_digest.clone();
    let instruction = Instruction::Choice {
        evidence: Box::new(evidence),
        at: corral_core::clock::now(),
    };
    Ok(match action {
        NativeAction::Commit { choice } => {
            let digest = choice.digest()?;
            let task = transfer::commit(ctx, &choice, &digest, instruction)?;
            json!({"action":"commit","task":task.id,"status":task.status})
        }
        NativeAction::Start { choice } => {
            let digest = choice.digest()?;
            let task = continuation::start(ctx, &choice, &digest, instruction)?;
            json!({"action":"start","task":task.id,"status":task.status})
        }
        NativeAction::Save {
            task,
            contract_revision,
        } => {
            let task = continuation::save(ctx, &task, &contract_revision, instruction)?;
            json!({"action":"save","task":task.id,"status":task.status})
        }
        NativeAction::TakeBack {
            task,
            contract_revision,
        } => {
            let returned = takeback::apply(ctx, &task, &contract_revision, instruction)?;
            json!({"action":"take_back","returned":returned})
        }
        NativeAction::Local { local } => {
            super::native_local::run(ctx, &session, &local, shown, instruction)?
        }
        NativeAction::Implement { implement } => {
            super::native_implement::run(ctx, &session, &implement, shown, &evidence_digest)?
        }
    })
}

pub fn consumed(ctx: &Ctx, input: &str) -> Result<Option<DecisionEvidence>> {
    ctx.store.record(CONSUMED, input)
}
