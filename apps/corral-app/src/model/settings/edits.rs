//! Device changes Settings sends in a daemon session: the one local question a revoke or a
//! Forget asks before it runs, and the identity of each device's last change, so its row shows
//! how corrald answered and a second press never re-mints it.

use std::collections::BTreeMap;

use super::State;
use crate::model::Id;
use crate::model::devices::DeviceOp;
use crate::model::effect::Effect;

/// A device change waiting for this device's confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    /// Revoke another device.
    Revoke { device: String, name: String },
    /// Forget this device's own pairing.
    Forget,
}

impl Pending {
    /// The question the confirmation asks.
    pub fn question(&self) -> String {
        match self {
            Pending::Revoke { name, .. } => format!(
                "Revoke {name}? Its sessions end at once and it must be paired again."
            ),
            Pending::Forget => "Forget this device? Its saved pairing is deleted here, so it stops connecting until it is paired or enrolled again. corrald keeps its record until another device revokes it.".into(),
        }
    }

    /// The confirming button's words.
    pub fn verb(&self) -> &'static str {
        match self {
            Pending::Revoke { .. } => "Revoke",
            Pending::Forget => "Forget",
        }
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
    /// Ask this device to confirm a revoke or a Forget.
    pub fn confirm_device(&mut self, pending: Pending) -> Vec<Effect> {
        self.edits.confirming = Some(pending);
        Vec::new()
    }

    pub fn cancel_device(&mut self) -> Vec<Effect> {
        self.edits.confirming = None;
        Vec::new()
    }

    /// Runs the confirmed change; a revoke goes under a new identity, whose status shows on
    /// the row.
    pub fn apply_device(&mut self) -> Vec<Effect> {
        match self.edits.confirming.take() {
            Some(Pending::Revoke { device, .. }) => {
                let key = self.ids.next_id();
                self.edits.acts.insert(device_slot(&device), key.clone());
                vec![Effect::Device(DeviceOp::Revoke { key, device })]
            }
            Some(Pending::Forget) => vec![Effect::Device(DeviceOp::Forget)],
            None => Vec::new(),
        }
    }

    /// Run this device's own operation (enrol, pair, push, a trusted-side pairing step): a name
    /// or code is sent trimmed, and an empty one sends nothing.
    pub fn device(&mut self, op: DeviceOp) -> Vec<Effect> {
        let op = match op {
            DeviceOp::Enrol { name } => {
                let name = name.trim();
                if name.is_empty() {
                    return Vec::new();
                }
                DeviceOp::Enrol { name: name.into() }
            }
            DeviceOp::Pair {
                code,
                name,
                endpoint,
            } => {
                let (code, name) = (code.trim(), name.trim());
                let endpoint = endpoint.as_deref().map(str::trim);
                // A typed address that is empty sends nothing; a browser has none to type.
                if code.is_empty() || name.is_empty() || endpoint == Some("") {
                    return Vec::new();
                }
                DeviceOp::Pair {
                    code: code.into(),
                    name: name.into(),
                    endpoint: endpoint.map(Into::into),
                }
            }
            op => op,
        };
        vec![Effect::Device(op)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_revoke_asks_once_and_sends_only_after_the_confirmation() {
        let mut state = State::default();
        let pending = Pending::Revoke {
            device: "phone".into(),
            name: "Phone".into(),
        };
        assert!(state.confirm_device(pending).is_empty());
        let sent = state.apply_device();
        assert!(matches!(
            sent.as_slice(),
            [Effect::Device(DeviceOp::Revoke { device, .. })] if device == "phone"
        ));
        assert!(state.apply_device().is_empty(), "the question is spent");
    }
}
