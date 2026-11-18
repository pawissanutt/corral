//! Device changes Settings sends in a daemon session: the one local question a revoke asks
//! before it is sent, and the identity of each device's last change, so its row shows how
//! corrald answered and a second press never re-mints it.

use std::collections::BTreeMap;

use super::State;
use crate::model::Id;
use crate::model::devices::DeviceOp;
use crate::model::effect::Effect;

/// A revoke waiting for this device's confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub device: String,
    pub name: String,
}

impl Pending {
    /// The question the confirmation asks.
    pub fn question(&self) -> String {
        format!(
            "Revoke {}? Its sessions end at once and it must be paired again.",
            self.name
        )
    }
}

/// The control slot of the last change sent for `device`, whose status shows on its row.
pub fn device_slot(device: &str) -> String {
    format!("device-{device}")
}

/// Device changes in progress.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Edits {
    /// Request identity per control slot.
    pub acts: BTreeMap<String, Id>,
    pub confirming: Option<Pending>,
}

impl State {
    /// Ask this device to confirm a revoke.
    pub fn confirm_device(&mut self, pending: Pending) -> Vec<Effect> {
        self.edits.confirming = Some(pending);
        Vec::new()
    }

    pub fn cancel_device(&mut self) -> Vec<Effect> {
        self.edits.confirming = None;
        Vec::new()
    }

    /// Send the confirmed revoke under a new identity, whose status shows on the row.
    pub fn apply_device(&mut self) -> Vec<Effect> {
        let Some(Pending { device, .. }) = self.edits.confirming.take() else {
            return Vec::new();
        };
        let key = self.ids.next_id();
        self.edits.acts.insert(device_slot(&device), key.clone());
        vec![Effect::Device(DeviceOp::Revoke { key, device })]
    }

    /// Run this device's own operation (enrol, pair, forget, push, a trusted-side pairing step):
    /// a name or code is sent trimmed, and an empty one sends nothing.
    pub fn device(&mut self, op: DeviceOp) -> Vec<Effect> {
        let op = match op {
            DeviceOp::Enrol { name } => {
                let name = name.trim();
                if name.is_empty() {
                    return Vec::new();
                }
                DeviceOp::Enrol { name: name.into() }
            }
            DeviceOp::Pair { code, name } => {
                let (code, name) = (code.trim(), name.trim());
                if code.is_empty() || name.is_empty() {
                    return Vec::new();
                }
                DeviceOp::Pair {
                    code: code.into(),
                    name: name.into(),
                }
            }
            op => op,
        };
        vec![Effect::Device(op)]
    }
}
