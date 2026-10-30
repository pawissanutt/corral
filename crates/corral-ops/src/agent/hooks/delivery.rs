//! Delivery-mode and adapter-registry seam: pi-native runs and review children take inbox
//! delivery through the `pi_native` adapter. Owner: T-252 child inbox-adapter.
use corral_core::inbox::DeliveryMode;

use super::consulted;
use crate::delivery::DeliveryAdapter;

/// Fixed at launch: pi-native messages always go through the inbox, never typed into a pane.
pub fn launch_mode() -> DeliveryMode {
    consulted("delivery");
    DeliveryMode::Inbox
}

/// The adapter registered for pi-native recipients.
pub fn adapter() -> Box<dyn DeliveryAdapter> {
    consulted("delivery");
    Box::new(super::super::delivery::PiNativeAdapter)
}
