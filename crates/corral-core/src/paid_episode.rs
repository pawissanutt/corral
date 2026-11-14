//! Canonical financial owner for a role lineage. Container ledgers are projections only;
//! replacing a host never creates another allowance or releases an uncertain request.
use crate::{
    Store,
    agent::{
        AgentRole, AgentRunState, BudgetLedger, BudgetLimits, BudgetRequest, ContainerRef,
        HostHealth, paths,
    },
    connection::ConnectionBinding,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

#[path = "paid_episode_transition.rs"]
mod transition;

pub const AREA: &str = "paid_episodes";
const ROOTS: &str = "paid_episode_roots";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaidRoot {
    pub store: String,
    pub role: AgentRole,
    pub subject: String,
    pub attempt: String,
}
impl PaidRoot {
    fn key(&self) -> Result<String> {
        ensure!(
            !self.subject.is_empty()
                && self.subject.len() <= 512
                && !self.attempt.is_empty()
                && self.attempt.len() <= 512,
            "paid root is incomplete or oversized"
        );
        Ok(paths::sha256_hex(&serde_json::to_vec(self)?))
    }
}

/// Frozen exact bindings, not provider/model aliases. Constructed only by trusted approval
/// handling, never from host payloads or the presence of a credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaidApproval {
    pub policy_digest: String,
    pub approval: String,
    pub allowance_nanos: u64,
    pub candidates: Vec<ConnectionBinding>,
}
impl PaidApproval {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.policy_digest.len() == 64
                && self.policy_digest.bytes().all(|b| b.is_ascii_hexdigit())
                && !self.approval.is_empty()
                && self.allowance_nanos > 0
                && self.allowance_nanos <= 9_007_199_254_740_991
                && !self.candidates.is_empty(),
            "paid approval needs an exact policy and finite USD allowance"
        );
        for (i, candidate) in self.candidates.iter().enumerate() {
            candidate.validate()?;
            ensure!(
                candidate.connection.api_key()
                    && candidate.candidate.is_some()
                    && !candidate.model.is_empty()
                    && !self.candidates[..i].contains(candidate),
                "invalid paid candidate"
            );
            ensure!(
                candidate.connection.vendor() == self.candidates[0].connection.vendor(),
                "cross-vendor paid episode refused"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaidBinding {
    pub container: ContainerRef,
    pub generation: u32,
    pub binding: ConnectionBinding,
}
impl PaidBinding {
    pub fn role(&self) -> AgentRole {
        match self.container {
            ContainerRef::Worker { .. } => AgentRole::Worker,
            ContainerRef::Reviewer { .. } => AgentRole::Reviewer,
            ContainerRef::Planner { .. } => AgentRole::Planner,
            ContainerRef::PlanReviewer { .. } => AgentRole::PlanReviewer,
            ContainerRef::Pm => AgentRole::Pm,
            ContainerRef::Assistant { .. } => AgentRole::Assistant,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaidLink {
    pub episode: String,
    pub epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum PaidPhase {
    Prepared {
        successor: PaidBinding,
        carry: Option<PaidCarry>,
    },
    Active {
        owner: PaidBinding,
    },
    Closed,
}
/// Existing overall/token safety is carried without treating subscription estimates as an
/// API bill. This is a safety projection, never another paid allowance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaidCarry {
    pub budget: BudgetLedger,
    pub limits: BudgetLimits,
    pub legacy_usage_gap: bool,
}
fn limits(state: &AgentRunState) -> BudgetLimits {
    state
        .current_generation()
        .and_then(|g| g.capabilities.as_ref())
        .and_then(|s| s.features.get("budget"))
        .filter(|f| f.enabled)
        .map(|f| BudgetLimits::from_settings(&f.settings))
        .unwrap_or_default()
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaidRequestOwner {
    pub request: String,
    pub owner: PaidBinding,
    pub reservation: BudgetRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaidEpisode {
    pub version: u32,
    pub id: String,
    pub root: PaidRoot,
    pub approval: PaidApproval,
    pub epoch: u64,
    pub switches: u32,
    pub phase: PaidPhase,
    pub ledger: BudgetLedger,
    pub owners: Vec<PaidRequestOwner>,
    pub legacy_usage_gap: bool,
}
impl PaidEpisode {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1 && !self.id.is_empty() && self.epoch > 0 && self.switches <= 1,
            "invalid paid episode schema/epoch"
        );
        self.root.key()?;
        self.approval.validate()?;
        let binding = match &self.phase {
            PaidPhase::Active { owner } => Some(owner),
            PaidPhase::Prepared { successor, .. } => Some(successor),
            PaidPhase::Closed => None,
        };
        if let Some(binding) = binding {
            self.approved(binding)?;
        }
        ensure!(
            self.owners.len() == self.ledger.requests.len(),
            "paid reservation owner missing"
        );
        ensure!(
            self.ledger.totals().1 <= self.approval.allowance_nanos,
            "paid ledger exceeds its finite allowance"
        );
        let owners: std::collections::BTreeMap<_, _> = self
            .owners
            .iter()
            .map(|o| (o.request.as_str(), o))
            .collect();
        ensure!(
            owners.len() == self.owners.len(),
            "duplicate paid reservation owner"
        );
        let mut seen = std::collections::BTreeSet::new();
        for request in &self.ledger.requests {
            ensure!(
                seen.insert(&request.request),
                "duplicate paid request identity"
            );
            ensure!(
                request.connection.is_some_and(|c| c.api_key())
                    && request.priced
                    && !request.unbounded
                    && owners
                        .get(request.request.as_str())
                        .is_some_and(|o| o.owner.generation == request.generation
                            && Some(o.owner.binding.connection) == request.connection
                            && o.reservation.request == request.request
                            && o.reservation.generation == request.generation
                            && o.reservation.connection == request.connection),
                "invalid paid request ownership"
            );
        }
        Ok(())
    }
    fn approved(&self, owner: &PaidBinding) -> Result<()> {
        owner.binding.validate()?;
        let subscription = matches!(
            owner.binding.connection,
            crate::connection::ConnectionId::OpenaiSubscription
                | crate::connection::ConnectionId::AnthropicSubscription
        ) && owner.binding.connection.vendor()
            == self.approval.candidates[0].connection.vendor();
        ensure!(
            owner.generation > 0
                && owner.role() == self.root.role
                && (subscription || self.approval.candidates.contains(&owner.binding)),
            "paid candidate/role is not approved"
        );
        Ok(())
    }
    pub fn owner(&self, link: &PaidLink, owner: &PaidBinding) -> Result<()> {
        self.validate()?;
        ensure!(
            link.episode == self.id
                && link.epoch == self.epoch
                && matches!(&self.phase, PaidPhase::Active { owner: active } if active == owner),
            "paid episode is fenced or not active"
        );
        Ok(())
    }
    pub fn reserve(
        &mut self,
        link: &PaidLink,
        owner: &PaidBinding,
        request: BudgetRequest,
    ) -> Result<bool> {
        self.owner(link, owner)?;
        ensure!(
            owner.binding.connection.api_key()
                && request.generation == owner.generation
                && request.connection == Some(owner.binding.connection)
                && request.response.is_none()
                && request.tokens > 0
                && request.priced
                && !request.unbounded,
            "paid request is unpriced, unbounded or has a foreign owner"
        );
        if let Some(existing) = self.owners.iter().find(|r| r.request == request.request) {
            ensure!(
                existing.owner == *owner && existing.reservation == request,
                "paid request identity reused with different bounds/owner"
            );
            return Ok(true);
        }
        let id = request.request.clone();
        let reservation = request.clone();
        let admitted = self.ledger.reserve(
            request,
            BudgetLimits {
                max_tokens: None,
                max_usd_nanos: Some(self.approval.allowance_nanos),
            },
        );
        if admitted {
            self.owners.push(PaidRequestOwner {
                request: id,
                owner: owner.clone(),
                reservation,
            });
        }
        Ok(admitted)
    }
    pub fn settle(
        &mut self,
        link: &PaidLink,
        owner: &PaidBinding,
        request: &str,
        response: String,
        tokens: u64,
        nanos: u64,
    ) -> Result<()> {
        self.owner(link, owner)?;
        ensure!(
            self.owners
                .iter()
                .any(|r| r.request == request && &r.owner == owner),
            "foreign paid reservation"
        );
        let bound = self
            .ledger
            .requests
            .iter()
            .find(|r| r.request == request)
            .context("paid reservation missing")?;
        ensure!(
            tokens <= bound.tokens && nanos <= bound.usd_nanos,
            "paid completion exceeds its conservative reservation"
        );
        self.ledger
            .settle(request, owner.generation, response, tokens, nanos)
            .map_err(anyhow::Error::msg)
    }
    /// Must run inside the ordinary role continuation claim, in the SAME commit as fencing
    /// the predecessor's native state. Ambiguous successor creation stays Prepared.
    pub fn prepare_transfer(
        &mut self,
        link: &PaidLink,
        source: &PaidBinding,
        successor: PaidBinding,
        state: &mut AgentRunState,
    ) -> Result<PaidLink> {
        self.owner(link, source)?;
        self.approved(&successor)?;
        ensure!(
            successor.container != source.container || successor.generation > source.generation,
            "retained container replacement must advance generation"
        );
        ensure!(
            state.paid_episode.as_ref() == Some(link)
                && state.binding.generation == source.generation
                && state
                    .current_generation()
                    .and_then(|g| g.connection.as_ref())
                    == Some(&source.binding)
                && successor != *source
                && successor.binding.connection.api_key(),
            "paid transfer is stale or not an API continuation"
        );
        if source.binding.connection.api_key() {
            ensure!(
                source.binding == successor.binding,
                "paid candidate is sticky within the episode"
            );
        } else {
            ensure!(
                self.switches == 0,
                "subscription-to-API switch already used"
            );
            self.switches += 1;
        }
        state.binding.health = HostHealth::Fenced;
        state
            .generation_mut(source.generation)
            .context("paid predecessor generation missing")?
            .fenced = Some(crate::agent::GenerationFence {
            reason: "paid episode transfer".into(),
            at: crate::clock::now(),
        });
        self.epoch = self.epoch.checked_add(1).context("paid epoch overflow")?;
        self.legacy_usage_gap |= state.legacy_usage_gap();
        let retained = successor.container == source.container;
        self.phase = PaidPhase::Prepared {
            successor,
            carry: Some(PaidCarry {
                budget: state.budget.clone(),
                limits: limits(state),
                legacy_usage_gap: self.legacy_usage_gap,
            }),
        };
        let next = PaidLink {
            episode: self.id.clone(),
            epoch: self.epoch,
        };
        if retained {
            state.paid_episode = Some(next.clone());
        }
        Ok(next)
    }
    pub fn activate(
        &mut self,
        link: &PaidLink,
        successor: &PaidBinding,
        state: &mut AgentRunState,
    ) -> Result<()> {
        self.validate()?;
        ensure!(
            link.episode == self.id
                && link.epoch == self.epoch
                && matches!(&self.phase, PaidPhase::Prepared { successor: pending, .. } if pending == successor)
                && state.binding.health == HostHealth::Healthy
                && state.binding.generation == successor.generation
                && state.binding.role == self.root.role
                && state.binding.store == self.root.store
                && state
                    .current_generation()
                    .is_some_and(|g| g.fenced.is_none()
                        && g.bound.is_some()
                        && g.connection.as_ref() == Some(&successor.binding))
                && state.paid_episode.as_ref().is_none_or(|old| old == link),
            "paid activation needs the exact healthy prepared successor"
        );
        if let PaidPhase::Prepared {
            carry: Some(carry), ..
        } = &self.phase
        {
            let next = limits(state);
            ensure!(
                carry
                    .limits
                    .max_tokens
                    .is_none_or(|old| next.max_tokens.is_some_and(|new| new <= old))
                    && carry
                        .limits
                        .max_usd_nanos
                        .is_none_or(|old| next.max_usd_nanos.is_some_and(|new| new <= old))
                    && (state.budget.requests.is_empty() || state.budget == carry.budget),
                "successor cannot reset or relax existing overall caps/history"
            );
            state.budget = carry.budget.clone();
        }
        self.legacy_usage_gap |= state.legacy_usage_gap();
        state.paid_episode = Some(link.clone());
        self.phase = PaidPhase::Active {
            owner: successor.clone(),
        };
        Ok(())
    }
    pub fn close(
        &mut self,
        link: &PaidLink,
        owner: &PaidBinding,
        state: &mut AgentRunState,
    ) -> Result<()> {
        self.owner(link, owner)?;
        ensure!(
            state.paid_episode.as_ref() == Some(link)
                && state.binding.generation == owner.generation,
            "foreign paid closure"
        );
        state.binding.health = HostHealth::Fenced;
        state
            .generation_mut(owner.generation)
            .context("paid generation missing")?
            .fenced = Some(crate::agent::GenerationFence {
            reason: "paid episode closed".into(),
            at: crate::clock::now(),
        });
        self.epoch = self.epoch.checked_add(1).context("paid epoch overflow")?;
        self.phase = PaidPhase::Closed;
        Ok(())
    }
}
impl Store {
    /// Reconcile creation that committed before a host received its predecessor link.
    pub fn paid_episode_for_root(&self, root: &PaidRoot) -> Result<Option<PaidEpisode>> {
        let Some(id) = self.record::<String>(ROOTS, &root.key()?)? else {
            return Ok(None);
        };
        let episode = self.paid_episode(&id)?;
        ensure!(&episode.root == root, "paid root index mismatch");
        Ok(Some(episode))
    }
    pub fn paid_episode(&self, id: &str) -> Result<PaidEpisode> {
        let episode: PaidEpisode = self
            .record(AREA, id)?
            .context("canonical paid episode missing; cannot initialize a new balance")?;
        episode.validate()?;
        ensure!(
            episode.id == id && episode.root.store == paths::store_id(self),
            "foreign paid episode"
        );
        Ok(episode)
    }
    /// Trusted setup/lifecycle seam. No CLI/host operation calls this from a key or a retry.
    /// The opaque ID is server-issued. Root insert and episode insert share the backend CAS.
    pub fn create_paid_episode(
        &self,
        root: PaidRoot,
        approval: PaidApproval,
        successor: PaidBinding,
    ) -> Result<PaidLink> {
        ensure!(root.store == paths::store_id(self), "foreign paid root");
        approval.validate()?;
        let key = root.key()?;
        self.records_transaction(|| {
            ensure!(
                self.record_row(ROOTS, &key)?.is_none(),
                "paid root already exists; recover its original episode"
            );
            let id = paths::sha256_hex(&serde_json::to_vec(&(
                &root,
                crate::clock::now(),
                std::process::id(),
            ))?);
            let episode = PaidEpisode {
                version: 1,
                id: id.clone(),
                root,
                approval,
                epoch: 1,
                switches: 0,
                phase: PaidPhase::Prepared {
                    successor,
                    carry: None,
                },
                ledger: BudgetLedger::default(),
                owners: Vec::new(),
                legacy_usage_gap: false,
            };
            episode.validate()?;
            ensure!(
                self.record_row(AREA, &id)?.is_none(),
                "paid episode ID collision"
            );
            self.save_record(ROOTS, &key, &id)?;
            self.save_paid_episode(&episode)?;
            Ok(PaidLink {
                episode: id,
                epoch: 1,
            })
        })
    }
    /// Always CAS even on refusal/state reads: a concurrent transfer must invalidate the
    /// caller's admission receipt and any existing-cap projection, not merely this row.
    pub fn save_paid_episode(&self, episode: &PaidEpisode) -> Result<()> {
        episode.validate()?;
        ensure!(
            self.has_record_unit(),
            "paid writes require an enclosing native/container transaction"
        );
        ensure!(
            episode.root.store == paths::store_id(self)
                && self.record::<String>(ROOTS, &episode.root.key()?)?.as_ref()
                    == Some(&episode.id),
            "paid episode has no canonical root index"
        );
        let before = self.record_row(AREA, &episode.id)?;
        if let Some(row) = &before {
            episode.follows(
                &serde_json::from_value(row.doc.clone())
                    .context("decoding previous paid episode")?,
            )?;
        }
        self.put_records(vec![crate::backend::RecordWrite {
            area: AREA.into(),
            key: episode.id.clone(),
            expected: before.map(|r| r.version),
            doc: Some(serde_json::to_value(episode)?),
            updated: crate::clock::now(),
        }])
    }
}

#[cfg(test)]
#[path = "paid_episode_tests.rs"]
mod tests;
