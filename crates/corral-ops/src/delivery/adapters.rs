//! Native delivery adapters: OmO and Claude for their runs, `corral_pi`
//! (agent::delivery) for corral-pi runs and review children.
pub mod claude;
pub mod omo;

use corral_core::Run;

use super::DeliveryAdapter;

/// The registered adapter for a corral-pi run; `None` for every other harness.
pub(crate) fn corral_pi(run: &Run) -> Option<Box<dyn DeliveryAdapter>> {
    crate::agent::hooks::run_is_corral_pi(run).then(crate::agent::hooks::delivery::adapter)
}

/// Native adapters registered for terminal-launched coding/planning runs.
pub(crate) fn for_run(run: &Run) -> Option<Box<dyn DeliveryAdapter>> {
    if let Some(adapter) = corral_pi(run) {
        Some(adapter)
    } else if run.harness.as_deref() == Some(claude::HARNESS) {
        Some(Box::new(claude::ClaudeAdapter::from_env()))
    } else {
        None
    }
}
