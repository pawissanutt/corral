//! Internal source proof, never owner consent. C2 already closed admission and
//! requires an empty activity ledger; the actual host must now attest its C1 drain.
use anyhow::{Context, Result, ensure};
use corral_core::agent::{AgentRole, Executor};
use corral_core::inbox::Recipient;
use corral_core::{Store, assistant::AssistantContainer};
use serde::{Deserialize, Serialize};

use super::{Expected, Journal};
use crate::Ctx;

#[cfg(test)]
#[path = "source_boundary_tests.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeProof {
    pub version: u32,
    pub transfer: String,
    pub assistant: String,
    pub generation: u32,
    pub session_id: String,
    pub pid: u32,
    pub start_time: String,
    pub expected: Expected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Proof {
    Native {
        proof: NativeProof,
    },
    /// A canonical generation-zero session has never launched a managed host.
    /// This is not a compatibility inference about an absent/unknown old host.
    Unlaunched {
        transfer: String,
        expected: Expected,
    },
}

fn reserved(container: &AssistantContainer, journal: &Journal) -> Result<Expected> {
    container.validate_protocol()?;
    let w = super::super::current(container)?;
    ensure!(
        journal.reserved
            && w.id == journal.expected.workflow
            && w.phase == corral_core::assistant_workflow::WorkflowPhase::HandoffPreparing
            && w.owner == corral_core::assistant_workflow::ScopeOwner::Local
            && w.handoff
                .as_ref()
                .is_some_and(|a| a.blob == journal.package.blob),
        "handoff_source_changed: source reservation/driver changed"
    );
    Ok(Expected {
        workflow: w.id.clone(),
        state_revision: w.state_revision,
        authority_epoch: w.authority_epoch,
        plan_revision: w.current_plan.as_ref().map(|p| p.revision),
    })
}

impl Proof {
    fn expected(&self) -> &Expected {
        match self {
            Self::Native { proof } => &proof.expected,
            Self::Unlaunched { expected, .. } => expected,
        }
    }

    fn require_binding(&self, container: &AssistantContainer, journal: &Journal) -> Result<()> {
        match self {
            Self::Unlaunched { transfer, .. } => ensure!(
                transfer == &journal.id
                    && container.state.binding.generation == 0
                    && container.state.generations.is_empty()
                    && container.pane.is_none()
                    && container.state.binding.callback.is_none(),
                "handoff_source_unsupported: source is not the attested unlaunched session"
            ),
            Self::Native { proof } => {
                let generation = container
                    .state
                    .current_generation()
                    .context("handoff_source_unsupported: generation missing")?;
                let bound = generation
                    .bound
                    .as_ref()
                    .context("handoff_source_unsupported: host was never bound")?;
                ensure!(
                    proof.version == 1
                        && proof.transfer == journal.id
                        && proof.assistant == journal.assistant
                        && proof.generation == container.state.binding.generation
                        && container.state.binding.role == AgentRole::Assistant
                        && generation.n == proof.generation
                        && proof.session_id == bound.session_id
                        && !proof.session_id.is_empty()
                        && proof.pid > 0
                        && proof.pid == bound.pid
                        && !proof.start_time.is_empty()
                        && proof.start_time == bound.start_time,
                    "handoff_source_stale: actual host/session/workflow/transfer epoch changed; no ownership published"
                );
            }
        }
        Ok(())
    }

    pub(super) fn require_current(
        &self,
        store: &Store,
        container: &AssistantContainer,
        journal: &Journal,
    ) -> Result<()> {
        ensure!(
            *self.expected() == reserved(container, journal)?,
            "handoff_source_stale: reserved workflow revision/epoch changed"
        );
        self.require_binding(container, journal)?;
        ensure!(
            super::super::boundary::activities(store, &journal.assistant)?.is_empty(),
            "handoff_source_busy: actual managed activity remains"
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
            "handoff_source_busy: owner effect remains uncertain"
        );
        Ok(())
    }

    pub(super) fn require_published(
        &self,
        container: &AssistantContainer,
        journal: &Journal,
    ) -> Result<()> {
        container.validate_protocol()?;
        let w = super::super::current(container)?;
        let pin = self.expected();
        let task = &journal.transfer.as_ref().context("transfer missing")?.task;
        ensure!(
            w.id == pin.workflow
                && w.authority_epoch
                    == pin
                        .authority_epoch
                        .checked_add(1)
                        .context("epoch overflow")?
                && w.state_revision
                    == pin
                        .state_revision
                        .checked_add(1)
                        .context("revision overflow")?
                && w.current_plan.as_ref().map(|p| p.revision) == pin.plan_revision
                && w.owner
                    == corral_core::assistant_workflow::ScopeOwner::Board { task: task.clone() }
                && w.phase == corral_core::assistant_workflow::WorkflowPhase::BoardOwned
                && w.handoff
                    .as_ref()
                    .is_some_and(|a| a.blob == journal.package.blob),
            "handoff_source_stale: published owner no longer binds its attested source/epoch"
        );
        self.require_binding(container, journal)
    }
}

pub(super) fn attest(ctx: &Ctx, id: &str) -> Result<()> {
    let result = collect(ctx, id);
    if let Err(error) = &result {
        ctx.store.records_transaction(|| {
            let mut journal =
                super::read_journal(&ctx.store, id)?.context("source journal missing")?;
            journal.waiting = Some(format!("{error:#}"));
            super::recovery::save(&ctx.store, &journal)
        })?;
    }
    result
}

fn collect(ctx: &Ctx, id: &str) -> Result<()> {
    let journal = super::read_journal(&ctx.store, id)?.context("source journal missing")?;
    let container = ctx.store.get_assistant_container(&journal.assistant)?;
    // A committed proof survives source-host death/restart of the controller, not a
    // source generation or epoch change. No future scheduling step calls that host.
    if let Some(proof) = &journal.source_boundary {
        return proof.require_current(&ctx.store, &container, &journal);
    }
    let expected = reserved(&container, &journal)?;
    let proof = if container.state.binding.generation == 0 && container.state.generations.is_empty()
    {
        Proof::Unlaunched {
            transfer: id.into(),
            expected,
        }
    } else {
        let generation = container
            .state
            .current_generation()
            .context("handoff_source_unsupported: generation missing")?;
        let bound = generation
            .bound
            .as_ref()
            .context("handoff_source_unsupported: native host identity missing")?;
        ensure!(
            generation.fenced.is_none()
                && !crate::agent::bridge::liveness::gone(&Executor {
                    pid: bound.pid,
                    start_time: bound.start_time.clone(),
                })?,
            "handoff_source_unsupported: no running host can attest this unproven source"
        );
        let mut client = crate::agent::bridge::BridgeClient::connect(
            &ctx.store,
            &Recipient::Assistant {
                session: journal.assistant.clone(),
            },
            generation.n,
        )?;
        let reply = client
            .request(
                "handoff_drain",
                serde_json::json!({
                    "version":1,"transfer":id,"expected":expected
                }),
            )
            .context(
                "handoff_source_unsupported: actual host cannot attest C1 drain; keep source held",
            )?;
        Proof::Native {
            proof: serde_json::from_value(reply)
                .context("handoff_source_unsupported: invalid native drain proof")?,
        }
    };
    ctx.store
        .settle_assistant_container(&journal.assistant, |current, _| {
            let mut journal =
                super::read_journal(&ctx.store, id)?.context("source journal disappeared")?;
            proof.require_current(&ctx.store, current, &journal)?;
            if let Some(old) = &journal.source_boundary {
                ensure!(
                    old == &proof,
                    "handoff_source_stale: another source proof committed"
                );
            } else {
                journal.source_boundary = Some(proof.clone());
                super::recovery::save(&ctx.store, &journal)?;
                ctx.store.log_event(
                    None,
                    "handoff_source_drained",
                    serde_json::json!({"transfer":id,"proof":proof}),
                )?;
            }
            Ok(())
        })
}
