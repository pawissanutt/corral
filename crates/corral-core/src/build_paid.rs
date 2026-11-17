//! Off-board Build financial lineage. A new generation fences hosts, never resets spending.
use crate::{
    Store,
    agent::{BudgetLedger, BudgetLimits, BudgetRequest, paths},
    connection::ConnectionBinding,
    paid_episode::PaidApproval,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
const AREA: &str = "build_paid_ledgers";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildPaidLedger {
    pub version: u32,
    pub store: String,
    pub session: String,
    pub profile: String,
    pub approval: PaidApproval,
    pub generation: u32,
    pub cap_sha256: String,
    pub ledger: BudgetLedger,
    /// Original (tokens, nanos), aligned with requests; settlement never rewrites a bound.
    pub bounds: Vec<(u64, u64)>,
}
impl BuildPaidLedger {
    pub fn route(&self) -> Result<&ConnectionBinding> {
        ensure!(
            self.approval.candidates.len() == 1,
            "Build needs one exact approved route"
        );
        self.approval
            .candidates
            .first()
            .context("Build route missing")
    }
    pub fn validate(&self) -> Result<()> {
        self.approval.validate()?;
        let route = self.route()?;
        ensure!(
            self.version == 1
                && self
                    .session
                    .strip_prefix("B-")
                    .is_some_and(|s| (6..=16).contains(&s.len())
                        && s.bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
                && !self.profile.is_empty()
                && self.generation > 0
                && self.cap_sha256.len() == 64
                && self.cap_sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid Build paid identity"
        );
        ensure!(
            self.ledger.totals().1 <= self.approval.allowance_nanos,
            "Build allowance exceeded"
        );
        ensure!(
            self.bounds.len() == self.ledger.requests.len(),
            "Build original reservation bounds missing"
        );
        let mut ids = std::collections::BTreeSet::new();
        for (request, &(tokens, nanos)) in self.ledger.requests.iter().zip(&self.bounds) {
            ensure!(
                request.connection == Some(route.connection)
                    && request.priced
                    && !request.unbounded
                    && request.generation > 0
                    && request.generation <= self.generation
                    && !request.request.is_empty()
                    && request.tokens <= tokens
                    && request.usd_nanos <= nanos
                    && ids.insert(request.request.as_str()),
                "invalid Build reservation"
            );
        }
        Ok(())
    }
    pub fn reserve(&mut self, request: BudgetRequest) -> Result<bool> {
        ensure!(
            request.generation == self.generation
                && request.connection == Some(self.route()?.connection)
                && request.priced
                && !request.unbounded
                && request.response.is_none()
                && request.tokens > 0
                && request.usd_nanos > 0,
            "invalid or fenced Build reservation"
        );
        if let Some((i, before)) = self
            .ledger
            .requests
            .iter()
            .enumerate()
            .find(|(_, r)| r.request == request.request)
        {
            ensure!(
                before.generation == request.generation
                    && self.bounds.get(i) == Some(&(request.tokens, request.usd_nanos)),
                "Build reservation replay changed its bound"
            );
            return Ok(true);
        }
        let bound = (request.tokens, request.usd_nanos);
        let admitted = self.ledger.reserve(
            request,
            BudgetLimits {
                max_tokens: None,
                max_usd_nanos: Some(self.approval.allowance_nanos),
            },
        );
        if admitted {
            self.bounds.push(bound);
        }
        Ok(admitted)
    }
    pub fn settle(
        &mut self,
        request: &str,
        response: String,
        tokens: u64,
        nanos: u64,
    ) -> Result<()> {
        let (i, bound) = self
            .ledger
            .requests
            .iter()
            .enumerate()
            .find(|(_, r)| r.request == request && r.generation == self.generation)
            .context("Build reservation missing or fenced")?;
        let &(max_tokens, max_nanos) = self
            .bounds
            .get(i)
            .context("Build original reservation missing")?;
        ensure!(
            tokens <= max_tokens && nanos <= max_nanos,
            "Build completion exceeds conservative reservation"
        );
        if bound.response.as_ref() == Some(&response) {
            return Ok(());
        }
        self.ledger
            .settle(request, self.generation, response, tokens, nanos)
            .map_err(anyhow::Error::msg)
    }
    fn follows(&self, before: &Self) -> Result<()> {
        before.validate()?;
        ensure!(
            self.store == before.store
                && self.session == before.session
                && self.profile == before.profile
                && self.approval == before.approval,
            "Build allowance and route are immutable"
        );
        ensure!(
            self.generation == before.generation
                || Some(self.generation) == before.generation.checked_add(1),
            "invalid Build generation transition"
        );
        if self.generation != before.generation {
            ensure!(
                self.ledger == before.ledger && self.cap_sha256 != before.cap_sha256,
                "Build recovery must retain spending and rotate capability"
            );
        } else {
            ensure!(
                self.cap_sha256 == before.cap_sha256,
                "Build capability changed without fencing"
            );
        }
        ensure!(
            self.ledger.requests.len() >= before.ledger.requests.len()
                && self.bounds.starts_with(&before.bounds),
            "Build reservation history removed or bounds rewritten"
        );
        // Same monotone request invariant as PaidEpisode; Build has no container/owner transitions.
        for (old, new) in before.ledger.requests.iter().zip(&self.ledger.requests) {
            ensure!(
                old.request == new.request
                    && old.generation == new.generation
                    && old.connection == new.connection
                    && old.priced == new.priced
                    && old.unbounded == new.unbounded,
                "Build reservation identity changed"
            );
            if old.response.is_some() || new.response.is_none() {
                ensure!(old == new, "Build reservation rewritten");
            } else {
                ensure!(
                    new.tokens <= old.tokens && new.usd_nanos <= old.usd_nanos,
                    "Build completion exceeded bound"
                );
            }
        }
        Ok(())
    }
}
impl Store {
    pub fn build_paid_ledger(&self, session: &str) -> Result<Option<BuildPaidLedger>> {
        let ledger: Option<BuildPaidLedger> = self.record(AREA, session)?;
        if let Some(ledger) = &ledger {
            ledger.validate()?;
            ensure!(
                ledger.session == session && ledger.store == paths::store_id(self),
                "foreign Build allowance"
            );
        }
        Ok(ledger)
    }
    /// Caller holds the persistent Build financial file lock across read, mutation and CAS commit.
    pub fn save_build_paid_ledger(&self, ledger: &BuildPaidLedger) -> Result<()> {
        ledger.validate()?;
        ensure!(
            self.has_record_unit() && ledger.store == paths::store_id(self),
            "Build paid write requires its store transaction"
        );
        let before = self.record_row(AREA, &ledger.session)?;
        if let Some(row) = &before {
            ledger.follows(
                &serde_json::from_value(row.doc.clone())
                    .context("decoding previous Build allowance")?,
            )?;
        } else {
            ensure!(
                ledger.generation == 1 && ledger.ledger.requests.is_empty(),
                "new Build allowance must start empty"
            );
        }
        self.put_records(vec![crate::backend::RecordWrite {
            area: AREA.into(),
            key: ledger.session.clone(),
            expected: before.map(|r| r.version),
            doc: Some(serde_json::to_value(ledger)?),
            updated: crate::clock::now(),
        }])
    }
}

#[cfg(test)]
#[path = "build_paid_tests.rs"]
mod tests;
