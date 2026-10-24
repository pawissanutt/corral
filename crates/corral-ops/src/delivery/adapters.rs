//! The delivery adapter registry: OmO for its own runs, `pi_native` (agent::delivery) for
//! pi-native runs and review children.
pub mod omo;

use corral_core::Run;

use super::DeliveryAdapter;

/// The registered adapter for a pi-native run; `None` for every other harness.
pub(crate) fn pi_native(run: &Run) -> Option<Box<dyn DeliveryAdapter>> {
    crate::agent::hooks::run_is_pi_native(run).then(crate::agent::hooks::delivery::adapter)
}
