//! Delivery-mode and adapter-registry seam: corral-pi runs and review children take inbox
//! delivery through the `corral_pi` adapter. Owner: T-252 child inbox-adapter.
use corral_core::inbox::DeliveryMode;

use super::consulted;
use crate::delivery::DeliveryAdapter;

/// Fixed at launch: corral-pi messages always go through the inbox, never typed into a pane.
pub fn launch_mode() -> DeliveryMode {
    consulted("delivery");
    DeliveryMode::Inbox
}

/// The adapter registered for corral-pi recipients.
pub fn adapter() -> Box<dyn DeliveryAdapter> {
    consulted("delivery");
    Box::new(super::super::delivery::CorralPiAdapter)
}
