//! What a receipt must carry to count: binding equality, plus full-tier dominance.
use anyhow::Result;
use corral_core::{ExpectedFailure, FULL_TIER, GateTierBinding};
use serde::Serialize;

/// The tier of every landing on main, `plan verify` and `plan accept`. No configuration, tier
/// policy or member-gate amendment lowers it.
pub const MAIN_LANDING_TIER: &str = FULL_TIER;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GateRequirement {
    /// The binding a receipt must carry; `None` is the full tier.
    pub binding: Option<GateTierBinding>,
    /// An unbound full receipt also satisfies it, subject to identity freshness.
    pub full_dominates: bool,
    /// A full member receipt must retain the same member selection inputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_binding: Option<GateTierBinding>,
}

impl GateRequirement {
    /// Legacy, untiered and other-base runs, and every main landing: the full tier.
    pub fn full() -> Self {
        Self {
            binding: None,
            full_dominates: true,
            full_binding: None,
        }
    }
}

pub fn satisfies(binding: Option<&GateTierBinding>, requirement: &GateRequirement) -> bool {
    binding == requirement.binding.as_ref()
        || (binding.is_none() && requirement.full_dominates)
        || (binding.is_some() && binding == requirement.full_binding.as_ref())
}

/// The binding of one selection; its digest covers every input the tier command sees.
pub(crate) fn binding(
    tier: &str,
    commands: &[String],
    tests: &[String],
    expected_failures: &[ExpectedFailure],
    source: &str,
) -> Result<GateTierBinding> {
    let selection = serde_json::to_vec(&(tier, commands, tests, expected_failures, source))?;
    Ok(GateTierBinding {
        tier: tier.to_owned(),
        selection_sha256: crate::verification::digest(&selection),
        source: source.to_owned(),
    })
}
