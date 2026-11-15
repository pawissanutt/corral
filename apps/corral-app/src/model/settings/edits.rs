//! The daemon session's owner edits in Settings: the command identity behind each control (so
//! its truthful status shows beside it and is never re-minted after an uncertain result), the
//! device authority being written and the inline fresh confirmation for device changes, the
//! profile model being edited, the profile being added, and the autopilot grant being written.

use std::collections::BTreeMap;

use corral_protocol::rows::SettingsSnapshot;

use super::State;
use super::authority::{Draft, Flag, may_turn_on};
use super::autopilot::{Field, GrantForm, ModeVerb, Power};
use super::live::config_set;
use super::profiles::{self, ProfileField, ProfileForm};
use crate::model::daemon::Feed;
use crate::model::devices::{DeviceOp, DeviceRow};
use crate::model::effect::Effect;
use crate::model::{Act, ActStatus, Id, act};

/// A device change waiting for this device's fresh confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    Revoke { device: String, name: String },
    Authority(Draft),
}

impl Pending {
    pub fn device(&self) -> &str {
        match self {
            Pending::Revoke { device, .. } => device,
            Pending::Authority(draft) => &draft.device,
        }
    }

    /// The question the confirmation asks.
    pub fn question(&self) -> String {
        match self {
            Pending::Revoke { name, .. } => {
                format!("Revoke {name}? Its sessions end at once and it must be paired again.")
            }
            Pending::Authority(draft) => draft.question(),
        }
    }

    pub fn confirm_label(&self) -> &'static str {
        match self {
            Pending::Revoke { .. } => "Revoke",
            Pending::Authority(draft) => draft.confirm_label(),
        }
    }
}

/// The control slot of the last change sent for `device`, whose status shows on its row.
pub fn device_slot(device: &str) -> String {
    format!("device-{device}")
}

/// Owner edits in progress.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Edits {
    /// Command identity per control slug.
    pub acts: BTreeMap<String, Id>,
    pub confirming: Option<Pending>,
    /// The device authority being written, before it is reviewed.
    pub authority: Option<Draft>,
    /// The profile whose model is being edited, and the draft.
    pub model: Option<(String, String)>,
    /// The profile being added, while its form is open.
    pub adding: Option<ProfileForm>,
    pub grant: GrantForm,
    /// Whether the grant form is open.
    pub granting: bool,
}

/// The control slot of the profile add.
pub const ADD_PROFILE: &str = "profile-add";

/// The control slot of `profile`'s removal.
pub fn remove_slot(profile: &str) -> String {
    format!("remove-{profile}")
}

/// The control slot of the rollover summary writer.
pub const SUMMARY: &str = "summary-profile";

impl State {
    fn track(&mut self, slot: String, act: Act) -> Vec<Effect> {
        self.edits.acts.insert(slot, act.key.clone());
        vec![Effect::Act(act)]
    }

    /// The command `slot` last sent, while corrald has not said how it ended: asked about
    /// again under its own identity rather than sent anew.
    fn unsettled(&self, feed: &Feed, slot: &str) -> Option<Id> {
        let key = self.edits.acts.get(slot)?;
        act::status(feed, key)
            .is_some_and(|status| status.uncertain())
            .then(|| key.clone())
    }

    /// Open the add form copying the default source, or keep the form being written; after
    /// an add is done the next one starts afresh.
    pub fn open_add(&mut self, settings: &SettingsSnapshot, feed: &Feed) -> Vec<Effect> {
        let done = self
            .edits
            .acts
            .get(ADD_PROFILE)
            .and_then(|key| act::status(feed, key))
            == Some(ActStatus::Done);
        if done {
            self.edits.acts.remove(ADD_PROFILE);
        }
        if done || self.edits.adding.is_none() {
            self.edits.adding = Some(ProfileForm {
                from: profiles::default_source(settings),
                ..ProfileForm::default()
            });
        }
        Vec::new()
    }

    pub fn close_add(&mut self) -> Vec<Effect> {
        self.edits.adding = None;
        Vec::new()
    }

    /// Copy `from` instead.
    pub fn add_source(&mut self, from: String) -> Vec<Effect> {
        if let Some(form) = &mut self.edits.adding {
            form.from = from;
        }
        Vec::new()
    }

    pub fn add_field(&mut self, field: ProfileField, value: String) -> Vec<Effect> {
        if let Some(form) = &mut self.edits.adding {
            form.set(field, value);
        }
        Vec::new()
    }

    /// Preview and confirm the add, guarded by the configuration it was read from; the form
    /// stays as written until the add is done.
    pub fn submit_add(&mut self, settings: &SettingsSnapshot, feed: &Feed) -> Vec<Effect> {
        if let Some(key) = self.unsettled(feed, ADD_PROFILE) {
            return vec![Effect::Recover(key)];
        }
        match self
            .edits
            .adding
            .as_ref()
            .and_then(|form| form.act(settings))
        {
            Some(act) => self.track(ADD_PROFILE.into(), act),
            None => Vec::new(),
        }
    }

    /// Preview and confirm removing configured `profile` through corrald, guarded by the
    /// configuration it was read from.
    pub fn remove_configured_profile(
        &mut self,
        settings: &SettingsSnapshot,
        feed: &Feed,
        profile: &str,
    ) -> Vec<Effect> {
        let slot = remove_slot(profile);
        if let Some(key) = self.unsettled(feed, &slot) {
            return vec![Effect::Recover(key)];
        }
        self.track(slot, profiles::remove_act(settings, profile))
    }

    /// Ask this device to confirm a device change.
    pub fn confirm_device(&mut self, pending: Pending) -> Vec<Effect> {
        self.edits.confirming = Some(pending);
        Vec::new()
    }

    /// Drop the device change waiting for confirmation; an authority draft stays open to edit.
    pub fn cancel_device(&mut self) -> Vec<Effect> {
        self.edits.confirming = None;
        Vec::new()
    }

    /// Open the authority editor on `device`, starting from what corrald reports.
    pub fn edit_authority(&mut self, device: &DeviceRow, own: bool) -> Vec<Effect> {
        if let Some(authority) = &device.authority {
            self.edits.authority = Some(Draft {
                device: device.id.clone(),
                name: device.name.clone(),
                from: authority.clone(),
                to: authority.clone(),
                own,
            });
            self.edits.confirming = None;
        }
        Vec::new()
    }

    /// Flip one flag of the draft; a flag this device may not raise stays off.
    pub fn flip_flag(&mut self, flag: Flag) -> Vec<Effect> {
        if let Some(draft) = &mut self.edits.authority {
            let on = !flag.of(&draft.to);
            if !on || may_turn_on(draft, flag).is_ok() {
                flag.set(&mut draft.to, on);
            }
        }
        Vec::new()
    }

    pub fn close_authority(&mut self) -> Vec<Effect> {
        self.edits.authority = None;
        if matches!(self.edits.confirming, Some(Pending::Authority(_))) {
            self.edits.confirming = None;
        }
        Vec::new()
    }

    /// Review the draft: its exact changes wait for this device's confirmation.
    pub fn review_authority(&mut self) -> Vec<Effect> {
        if let Some(draft) = &self.edits.authority
            && !draft.changes().is_empty()
        {
            self.edits.confirming = Some(Pending::Authority(draft.clone()));
        }
        Vec::new()
    }

    /// Send the confirmed device change under a new identity, whose status shows on the row.
    pub fn apply_device(&mut self) -> Vec<Effect> {
        let Some(pending) = self.edits.confirming.take() else {
            return Vec::new();
        };
        let key = Id::from(crate::client::requests::new_id());
        self.edits
            .acts
            .insert(device_slot(pending.device()), key.clone());
        let op = match pending {
            Pending::Revoke { device, .. } => DeviceOp::Revoke { key, device },
            Pending::Authority(draft) => {
                self.edits.authority = None;
                DeviceOp::SetAuthority {
                    key,
                    title: draft.title(),
                    change: corral_protocol::app::DeviceSetAuthority {
                        device: draft.device,
                        authority: draft.to,
                    },
                }
            }
        };
        vec![Effect::Device(op)]
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

    /// Choose who writes rollover summaries (None: each thread's own model), guarded by the
    /// configuration it was read from.
    pub fn set_summary(
        &mut self,
        settings: &SettingsSnapshot,
        profile: Option<&str>,
    ) -> Vec<Effect> {
        self.track(SUMMARY.into(), super::live::summary_set(settings, profile))
    }

    /// Make `profile` the assistant profile, guarded by the configuration it was read from.
    pub fn use_profile(&mut self, settings: &SettingsSnapshot, profile: &str) -> Vec<Effect> {
        let act = config_set(
            settings,
            format!("Use {profile} for assistants"),
            "assistant.agent".into(),
            profile.to_owned(),
        );
        self.track(format!("use-{profile}"), act)
    }

    /// Start editing a profile's model.
    pub fn edit_model(&mut self, profile: String, model: String) -> Vec<Effect> {
        self.edits.model = Some((profile, model));
        Vec::new()
    }

    pub fn draft_model(&mut self, text: String) -> Vec<Effect> {
        if let Some((_, draft)) = &mut self.edits.model {
            *draft = text;
        }
        Vec::new()
    }

    pub fn cancel_model(&mut self) -> Vec<Effect> {
        self.edits.model = None;
        Vec::new()
    }

    /// Save the edited model through the hash-guarded configuration command.
    pub fn save_model(&mut self, settings: &SettingsSnapshot) -> Vec<Effect> {
        let Some((profile, draft)) = self.edits.model.take() else {
            return Vec::new();
        };
        let model = draft.trim().to_owned();
        if model.is_empty() {
            self.edits.model = Some((profile, draft));
            return Vec::new();
        }
        let act = config_set(
            settings,
            format!("Set {profile}'s model to {model}"),
            format!("profiles.{profile}.model"),
            model,
        );
        self.track(format!("model-{profile}"), act)
    }

    /// Change the operating mode (confirmed against corrald's preview).
    pub fn change_mode(&mut self, verb: ModeVerb) -> Vec<Effect> {
        self.track(format!("mode-{}", verb.slug()), verb.act())
    }

    pub fn open_grant(&mut self, open: bool) -> Vec<Effect> {
        self.edits.granting = open;
        Vec::new()
    }

    pub fn grant_field(&mut self, field: Field, value: String) -> Vec<Effect> {
        self.edits.grant.set(field, value);
        Vec::new()
    }

    pub fn grant_power(&mut self, power: Power) -> Vec<Effect> {
        let on = &mut self.edits.grant.powers[power as usize];
        *on = !*on;
        Vec::new()
    }

    pub fn grant_include_new(&mut self) -> Vec<Effect> {
        self.edits.grant.include_new = !self.edits.grant.include_new;
        Vec::new()
    }

    /// Preview and confirm the written grant; the form stays as written until it settles.
    pub fn submit_grant(&mut self) -> Vec<Effect> {
        match self.edits.grant.act() {
            Some(act) => self.track("grant".into(), act),
            None => Vec::new(),
        }
    }
}
