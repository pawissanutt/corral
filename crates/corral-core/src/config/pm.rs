//! PM session configuration and request-cache health policy.
use serde::{Deserialize, Serialize};

use super::{PM_DEFAULT_KINDS, attention, yes};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PmCfg {
    /// Profile key used for the workspace PM pane. Default "omo".
    pub agent: String,
    /// Opt in to actionable event delivery to the live workspace PM pane.
    #[serde(default)]
    pub push_events: bool,
    /// Allow the watch driver to relaunch the corral-pi PM.
    #[serde(default)]
    pub supervise: bool,
    /// PM-only cache-health pause and price-tier reporting.
    #[serde(default)]
    pub guard: PmGuardCfg,
    /// Opt-in provider cache-key experiment, fixed in the launch snapshot.
    #[serde(default)]
    pub cache_key: PmCacheKeyCfg,
    /// Actionable event kinds delivered to the PM when push is enabled.
    #[serde(default = "default_push_kinds")]
    pub push_kinds: Vec<String>,
    #[serde(default = "attention::seconds")]
    pub attention_secs: u64,
    #[serde(default = "attention::owner_seconds")]
    pub attention_owner_secs: u64,
    #[serde(default = "yes")]
    pub attention_notify: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PmCacheKeyCfg {
    pub mode: PmCacheKeyMode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PmCacheKeyMode {
    #[default]
    Session,
    Stable,
}

/// `[pm.guard]` observes provider usage; it is never a spend limit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PmGuardCfg {
    pub large_context_tokens: u64,
    pub min_cache_read_ratio: f64,
    pub consecutive_misses: u32,
    pub price_tier_input_tokens: u64,
    pub pause: bool,
}

impl Default for PmGuardCfg {
    fn default() -> Self {
        Self {
            large_context_tokens: 260_000,
            min_cache_read_ratio: 0.5,
            consecutive_misses: 2,
            price_tier_input_tokens: 272_000,
            pause: true,
        }
    }
}

impl PmGuardCfg {
    pub(super) fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (key, value) in [
            ("large_context_tokens", self.large_context_tokens),
            ("price_tier_input_tokens", self.price_tier_input_tokens),
        ] {
            if !(1..=9_007_199_254_740_991).contains(&value) {
                out.push(format!(
                    "pm.guard.{key} must be within 1..=9007199254740991"
                ));
            }
        }
        if !self.min_cache_read_ratio.is_finite()
            || !(0.0..=1.0).contains(&self.min_cache_read_ratio)
        {
            out.push("pm.guard.min_cache_read_ratio must be finite and within 0..=1".into());
        }
        if self.consecutive_misses == 0 {
            out.push("pm.guard.consecutive_misses must be > 0".into());
        }
        out
    }
}

fn default_push_kinds() -> Vec<String> {
    PM_DEFAULT_KINDS
        .iter()
        .map(|kind| (*kind).to_string())
        .collect()
}

impl Default for PmCfg {
    fn default() -> Self {
        Self {
            agent: "omo".into(),
            push_events: false,
            supervise: false,
            guard: PmGuardCfg::default(),
            cache_key: PmCacheKeyCfg::default(),
            push_kinds: default_push_kinds(),
            attention_secs: attention::seconds(),
            attention_owner_secs: attention::owner_seconds(),
            attention_notify: true,
        }
    }
}

#[cfg(test)]
#[path = "pm_tests.rs"]
mod tests;
