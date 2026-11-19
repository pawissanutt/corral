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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mixed: Option<MixedBuildApproval>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reservation_routes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MixedBuildApproval {
    pub approval: String,
    pub allowance_nanos: u64,
    pub routes: std::collections::BTreeMap<String, PaidApproval>,
    pub presets: std::collections::BTreeMap<String, String>,
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
    pub fn allowance_nanos(&self) -> u64 {
        self.mixed
            .as_ref()
            .map_or(self.approval.allowance_nanos, |mixed| mixed.allowance_nanos)
    }

    pub fn request_route(&self, digest: Option<&str>) -> Result<&ConnectionBinding> {
        if let Some(mixed) = &self.mixed {
            let digest = digest.context("mixed Build request needs an exact route")?;
            return mixed
                .routes
                .values()
                .flat_map(|p| &p.candidates)
                .find(|binding| binding.digest == digest)
                .context("Build request route was not admitted");
        }
        let route = self.route()?;
        ensure!(
            digest.is_none_or(|digest| digest == route.digest),
            "Build request changed approved route"
        );
        Ok(route)
    }

    pub fn validate(&self) -> Result<()> {
        self.approval.validate()?;
        self.route()?;
        if let Some(mixed) = &self.mixed {
            ensure!(
                !mixed.approval.trim().is_empty()
                    && mixed.routes.len() <= 17
                    && !mixed.routes.is_empty()
                    && (1..=9_007_199_254_740_991).contains(&mixed.allowance_nanos)
                    && mixed
                        .routes
                        .values()
                        .any(|approval| approval == &self.approval)
                    && self.reservation_routes.len() == self.bounds.len(),
                "invalid mixed Build approval or history"
            );
            for approval in mixed.routes.values() {
                approval.validate()?;
                ensure!(
                    approval.candidates.len() == 1,
                    "Build profile needs one exact route"
                );
            }
        } else {
            ensure!(
                self.reservation_routes.is_empty(),
                "legacy Build cannot retain mixed routes"
            );
        }
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
            self.bounds.len() == self.ledger.requests.len(),
            "Build original reservation bounds missing"
        );
        let mut ids = std::collections::BTreeSet::new();
        for (i, (request, &(tokens, nanos))) in
            self.ledger.requests.iter().zip(&self.bounds).enumerate()
        {
            let route = self.request_route(self.reservation_routes.get(i).map(String::as_str))?;
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
        self.reserve_for(request, None)
    }

    pub fn reserve_for(&mut self, request: BudgetRequest, digest: Option<&str>) -> Result<bool> {
        let route = self.request_route(digest)?;
        let pinned = self.mixed.as_ref().map(|_| route.digest.clone());
        ensure!(
            request.generation == self.generation
                && request.connection == Some(route.connection)
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
                    && self.bounds.get(i) == Some(&(request.tokens, request.usd_nanos))
                    && self.reservation_routes.get(i) == pinned.as_ref(),
                "Build reservation replay changed its bound"
            );
            return Ok(true);
        }
        let bound = (request.tokens, request.usd_nanos);
        // Spending caps are disabled: reservations are recorded for usage reporting only.
        let admitted = self.ledger.reserve(request, BudgetLimits::default());
        if admitted {
            self.bounds.push(bound);
            self.reservation_routes.extend(pinned);
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
        self.settle_for(request, response, tokens, nanos, None)
    }

    pub fn settle_for(
        &mut self,
        request: &str,
        response: String,
        tokens: u64,
        nanos: u64,
        digest: Option<&str>,
    ) -> Result<()> {
        self.request_route(digest)?;
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
        if self.mixed.is_some() {
            ensure!(
                self.reservation_routes.get(i).map(String::as_str) == digest,
                "Build settlement changed reserved route"
            );
            if let Some(previous) = &bound.response {
                ensure!(
                    previous == &response && bound.tokens == tokens && bound.usd_nanos == nanos,
                    "Build settlement replay changed completion"
                );
                return Ok(());
            }
            let entry = &mut self.ledger.requests[i];
            entry.response = Some(response);
            entry.tokens = tokens;
            entry.usd_nanos = nanos;
            return Ok(());
        }
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
                && self.approval == before.approval
                && self.mixed == before.mixed,
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
                && self.bounds.starts_with(&before.bounds)
                && self
                    .reservation_routes
                    .starts_with(&before.reservation_routes),
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
