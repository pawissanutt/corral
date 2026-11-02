//! The delivery adapter registry: OmO for its own runs, `corral_pi` (agent::delivery) for
//! corral-pi runs and review children.
pub mod omo;

use corral_core::Run;

use super::DeliveryAdapter;

/// The registered adapter for a corral-pi run; `None` for every other harness.
pub(crate) fn corral_pi(run: &Run) -> Option<Box<dyn DeliveryAdapter>> {
    crate::agent::hooks::run_is_corral_pi(run).then(crate::agent::hooks::delivery::adapter)
}
