//! Cumulative, recovery-critical model reservations. Neither generations nor session cursors own
//! these counters: the container does. Uncertain calls keep their full reservation indefinitely.
use super::CapabilitySettings;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetLedger {
    pub requests: Vec<BudgetRequest>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetRequest {
    pub request: String,
    pub generation: u32,
    pub tokens: u64,
    pub usd_nanos: u64,
    #[serde(default)]
    pub priced: bool,
    #[serde(default)]
    pub unbounded: bool,
    /// None means reserved/uncertain, not free. A response identity settles at most once.
    pub response: Option<String>,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetLimits {
    pub max_tokens: Option<u64>,
    pub max_usd_nanos: Option<u64>,
}
impl BudgetLimits {
    pub fn from_settings(settings: &CapabilitySettings) -> Self {
        Self {
            max_tokens: settings.get("max_tokens").and_then(|v| v.as_u64()),
            max_usd_nanos: settings
                .get("max_usd")
                .and_then(|v| v.as_f64())
                .map(|v| (v * 1_000_000_000.0).ceil() as u64),
        }
    }
    fn fits(self, tokens: u64, usd: u64) -> bool {
        self.max_tokens.is_none_or(|cap| tokens <= cap)
            && self.max_usd_nanos.is_none_or(|cap| usd <= cap)
    }
}
impl BudgetLedger {
    pub fn totals(&self) -> (u64, u64) {
        self.requests.iter().fold((0u64, 0u64), |(t, u), r| {
            (t.saturating_add(r.tokens), u.saturating_add(r.usd_nanos))
        })
    }
    pub fn exhausted(&self, limits: BudgetLimits) -> bool {
        let (tokens, usd) = self.totals();
        limits.max_tokens.is_some_and(|cap| tokens >= cap)
            || limits.max_usd_nanos.is_some_and(|cap| usd >= cap)
    }
    pub fn reserve(&mut self, request: BudgetRequest, limits: BudgetLimits) -> bool {
        let (t, u) = self.totals();
        if (limits.max_tokens.is_some() || limits.max_usd_nanos.is_some())
            && (request.unbounded
                || self
                    .requests
                    .iter()
                    .any(|r| r.unbounded && r.response.is_none()))
        {
            return false;
        }
        if limits.max_usd_nanos.is_some()
            && (!request.priced || self.requests.iter().any(|r| !r.priced))
        {
            return false;
        }
        if self.requests.len() >= 16384
            || self.requests.iter().any(|r| r.request == request.request)
            || !limits.fits(
                t.saturating_add(request.tokens),
                u.saturating_add(request.usd_nanos),
            )
            || self.exhausted(limits)
        {
            return false;
        }
        self.requests.push(request);
        true
    }
    pub fn settle(
        &mut self,
        request: &str,
        generation: u32,
        response: String,
        tokens: u64,
        usd: u64,
    ) -> Result<(), String> {
        let duplicate = self
            .requests
            .iter()
            .any(|r| r.response.as_ref() == Some(&response));
        let entry = self
            .requests
            .iter_mut()
            .find(|r| r.request == request && r.generation == generation)
            .ok_or("unknown budget reservation")?;
        if entry.response.is_some() {
            return Err("budget reservation is already settled".into());
        }
        entry.tokens = if duplicate { 0 } else { tokens };
        entry.usd_nanos = if duplicate { 0 } else { usd };
        entry.response = Some(response);
        Ok(())
    }
}
pub(crate) fn budget_settings(config: &CapabilitySettings) -> Result<CapabilitySettings, String> {
    for (key, value) in config {
        match key.as_str() {
            "max_tokens"
                if value
                    .as_u64()
                    .is_some_and(|v| v > 0 && v <= 9_007_199_254_740_991) => {}
            "max_usd"
                if value
                    .as_f64()
                    .is_some_and(|v| v.is_finite() && v > 0.0 && v <= 1_000_000.0) => {}
            _ => {
                return Err(format!(
                    "{key}: budget accepts positive max_tokens (safe integer) and max_usd (<=1000000)"
                ));
            }
        }
    }
    Ok(config.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn configured_limits_enable_all_presets_roles_and_reject_invalid_values() {
        use super::super::AgentRole;
        use crate::config::agent_capabilities::{AgentCapabilities, Preset};
        for preset in [Preset::Strict, Preset::Standard, Preset::Full] {
            let mut config = AgentCapabilities::default();
            config.set_preset(preset);
            for role in [
                AgentRole::Worker,
                AgentRole::Reviewer,
                AgentRole::Pm,
                AgentRole::Planner,
                AgentRole::PlanReviewer,
            ] {
                assert!(!config.resolve(role, None).features["budget"].enabled);
                config
                    .set_settings(role, "budget", [("max_tokens".into(), json!(100))].into())
                    .unwrap();
                assert!(config.resolve(role, None).features["budget"].enabled);
                config.set(role, "budget", false).unwrap();
                assert!(!config.resolve(role, None).features["budget"].enabled);
            }
        }
        for value in [json!(0), json!(-1), json!("100"), json!(1.5)] {
            assert!(budget_settings(&[("max_tokens".into(), value)].into()).is_err());
        }
        assert!(budget_settings(&[("max_usd".into(), json!(1000001))].into()).is_err());
    }
    #[test]
    fn unbounded_subprocess_lanes_cannot_claim_a_hard_cap() {
        let mut ledger = BudgetLedger::default();
        let limits = BudgetLimits {
            max_tokens: Some(100),
            max_usd_nanos: None,
        };
        let request = BudgetRequest {
            request: "sdk-lane".into(),
            generation: 1,
            tokens: 10,
            usd_nanos: 0,
            priced: true,
            unbounded: true,
            response: None,
        };
        assert!(!ledger.reserve(request.clone(), limits));
        assert!(ledger.reserve(request.clone(), BudgetLimits::default()));
        let next = BudgetRequest {
            request: "bounded-resume".into(),
            generation: 2,
            unbounded: false,
            ..request
        };
        assert!(!ledger.reserve(next.clone(), limits));
        ledger
            .settle("sdk-lane", 1, "known-response".into(), 10, 0)
            .unwrap();
        assert!(ledger.reserve(next, limits));
    }
    #[test]
    fn cumulative_reservations_resume_dedupe_and_uncertainty() {
        let limits = BudgetLimits {
            max_tokens: Some(100),
            max_usd_nanos: Some(100),
        };
        let mut ledger = BudgetLedger::default();
        for (i, kind) in ["main", "summary", "explore", "oracle"].iter().enumerate() {
            assert!(
                ledger.reserve(
                    BudgetRequest {
                        request: kind.to_string(),
                        generation: 1,
                        tokens: 25,
                        usd_nanos: 25,
                        priced: true,
                        unbounded: false,
                        response: None
                    },
                    limits
                ),
                "{i}"
            );
        }
        assert!(ledger.exhausted(limits));
        let next = BudgetRequest {
            request: "resume".into(),
            generation: 2,
            tokens: 25,
            usd_nanos: 25,
            priced: true,
            unbounded: false,
            response: None,
        };
        let mut copy: BudgetLedger = serde_json::from_value(json!(ledger)).unwrap();
        assert!(!copy.reserve(next.clone(), limits));
        let higher = BudgetLimits {
            max_tokens: Some(125),
            max_usd_nanos: Some(125),
        };
        assert!(copy.reserve(next, higher));
        copy.settle("main", 1, "provider/model/response".into(), 10, 10)
            .unwrap();
        copy.settle("resume", 2, "provider/model/response".into(), 10, 10)
            .unwrap();
        assert_eq!(copy.totals(), (85, 85)); // the other three calls remain reserved
        assert!(copy.settle("resume", 2, "again".into(), 10, 10).is_err());
        let unpriced = BudgetRequest {
            request: "unpriced".into(),
            generation: 3,
            tokens: 1,
            usd_nanos: 0,
            priced: false,
            unbounded: false,
            response: None,
        };
        assert!(!copy.reserve(unpriced.clone(), higher));
        assert!(copy.reserve(unpriced, BudgetLimits::default()));
        assert!(!copy.reserve(
            BudgetRequest {
                request: "priced-after-gap".into(),
                generation: 4,
                tokens: 1,
                usd_nanos: 1,
                priced: true,
                unbounded: false,
                response: None
            },
            higher
        ));
        assert!(copy.exhausted(BudgetLimits {
            max_tokens: None,
            max_usd_nanos: Some(85)
        }));
    }
}
